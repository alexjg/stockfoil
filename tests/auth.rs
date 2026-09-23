//! Authentication, permission snapshots, and verified receipt boundaries.
//! Received history is authenticated before admission; only local work can wait
//! inside the graph for signatures or causal delivery.
mod support;

use support::{author, key, sign, signature};

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::SigningKey;
use stockfoil::{
    BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, DiffError, IngestError, ItemId,
    QueryError, ResolveError, Signature, SignatureError, SignatureEvidence, UnverifiedItem,
    VerificationError, ViewPoint, Vis, WritePermission,
};

fn commit(key: &SigningKey, value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents: parents.to_vec(),
        author: author(key),
        payload: BlobHash::of_contents(&[value]),
    })
}

fn receipt(meta: CommitMeta, key: &SigningKey) -> UnverifiedItem {
    let evidence = SignatureEvidence {
        author: author(key),
        commit: *meta.hash(),
        signature: signature(key, *meta.hash()),
    };
    UnverifiedItem::Commit {
        meta,
        signatures: vec![evidence],
    }
}

fn grant(graph: &mut CommitGraph<u8>, keys: &[&SigningKey]) {
    graph.set_write_permissions(
        &keys
            .iter()
            .map(|key| (author(key), WritePermission::Unbounded))
            .collect(),
    );
}

fn published(graph: &CommitGraph<u8>) -> Vec<u8> {
    let mut result = Vec::new();
    graph.traverse_publishable(|_, value| {
        result.push(*value);
        ControlFlow::Continue(())
    });
    result
}

#[test]
fn local_signature_arrival_changes_only_new_signed_view_points() {
    let alice = key(1);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    let c = commit(&alice, 1, &[]);
    let hash = *c.hash();
    graph.ingest_local_commit(c, 1);
    let working = graph.view_point();
    assert!(matches!(
        graph.resolve(&working),
        Err(ResolveError::MissingSignature { .. })
    ));
    let before = graph.signed_view_point();
    assert!(!graph.resolve(&before).unwrap().visible(&hash).unwrap());
    assert!(published(&graph).is_empty());

    sign(&mut graph, &alice, hash);
    let after = graph.signed_view_point();
    assert!(graph.resolve(&working).unwrap().visible(&hash).unwrap());
    assert_eq!(published(&graph), vec![1]);
    assert_eq!(before.delivered_heads(), after.delivered_heads());
    assert_eq!(
        graph
            .diff(&before, &after)
            .unwrap()
            .visibility(&hash)
            .unwrap(),
        Vis::After
    );
    assert!(!graph.resolve(&before).unwrap().visible(&hash).unwrap());
    assert!(graph.resolve(&after).unwrap().visible(&hash).unwrap());
    let mut count = 0;
    graph.traverse_delivered_since(before.delivered_heads(), |_, _| {
        count += 1;
        ControlFlow::Continue(())
    });
    assert_eq!(count, 0);

    // Historical signatures are rejected; duplicate receipts remain idempotent.
    assert_eq!(
        graph.ingest_signature(author(&alice), hash, signature(&alice, hash)),
        Err(SignatureError::NotPendingLocalCommit(hash))
    );
    support::receive_commit(&mut graph, commit(&alice, 1, &[]), 99);
    assert_eq!(graph.view_point(), after);
}

#[test]
fn signatures_require_known_local_work_but_can_precede_causal_delivery() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let tip = commit(&alice, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    assert_eq!(
        graph.ingest_signature(author(&alice), *tip.hash(), signature(&alice, *tip.hash())),
        Err(SignatureError::NotPendingLocalCommit(*tip.hash()))
    );
    graph.ingest_local_commit(tip.clone(), 2);
    sign(&mut graph, &alice, *tip.hash());
    let detached = graph.view_point();
    assert!(detached.delivered_heads().is_empty());
    assert!(detached.authentication_frontiers().is_empty());
    assert_eq!(
        graph.resolve(&detached).unwrap().visible(tip.hash()),
        Err(QueryError::UnknownCommit(*tip.hash()))
    );

    // Queued local data and its signature are not forwarded before the root.
    let mut peer = CommitGraph::new();
    grant(&mut peer, &[&alice]);
    peer.merge(&graph);
    assert!(peer.delivered_heads().next().is_none());
    assert!(
        peer.signed_view_point()
            .authentication_frontiers()
            .is_empty()
    );
    graph.ingest_local_commit(root.clone(), 1);
    assert!(!graph.has_admitted_commit(root.hash()));
    assert!(graph.publishable_commit(root.hash()).is_none());
    assert!(graph.export(&ItemId::Commit(*root.hash())).is_err());
    peer.merge(&graph);
    assert_eq!(peer.delivered_heads().count(), 0);
    // Local authentication by the tip cannot synthesize a root receipt.
    sign(&mut graph, &alice, *root.hash());
    peer.merge(&graph);
    for doc in [&graph, &peer] {
        let view_point = doc.view_point();
        assert_eq!(
            view_point.authentication_frontiers()[&author(&alice)],
            vec![*tip.hash()]
        );
        let resolved = doc.resolve(&view_point).unwrap();
        assert!(resolved.visible(root.hash()).unwrap());
        assert!(resolved.visible(tip.hash()).unwrap());
        assert!(
            !doc.resolve(&detached)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );
    }
}

#[test]
fn a_write_frontier_supplies_no_proof_until_fully_delivered() {
    let alice = key(1);
    let bob = key(2);
    let c = commit(&alice, 1, &[]);
    let missing = commit(&bob, 2, &[]);
    let frontier = commit(&bob, 3, &[*c.hash(), *missing.hash()]);
    let mut graph = CommitGraph::new();
    graph.set_write_permissions(&HashMap::from([(
        author(&alice),
        WritePermission::Bounded(vec![*frontier.hash()]),
    )]));
    support::receive_commit(&mut graph, c.clone(), 1);
    let before = graph.view_point();
    assert!(!graph.resolve(&before).unwrap().visible(c.hash()).unwrap());

    // Buffer the unverified envelope outside the graph until all parents exist.
    let pending = receipt(frontier.clone(), &bob);
    let pending = pending.verify().unwrap();
    assert!(matches!(
        graph.ingest(pending.clone(), |_| panic!("missing dependency must precede payload lookup")),
        Err(IngestError::MissingDependency(hash)) if hash == *missing.hash()
    ));
    assert_eq!(graph.view_point(), before); // Reading its parent link isn't proof.
    support::receive_commit(&mut graph, missing, 2);
    graph.ingest(pending, |_| 3).unwrap();
    let after = graph.view_point();
    assert!(graph.resolve(&after).unwrap().visible(c.hash()).unwrap());
    assert!(!graph.resolve(&before).unwrap().visible(c.hash()).unwrap());
    assert_eq!(
        graph
            .diff(&before, &after)
            .unwrap()
            .visibility(c.hash())
            .unwrap(),
        Vis::After
    );
    assert!(
        !graph
            .resolve(&after)
            .unwrap()
            .visible(frontier.hash())
            .unwrap()
    );
    let mut newly_delivered = Vec::new();
    graph.traverse_delivered_since(before.delivered_heads(), |_, value| {
        newly_delivered.push(*value);
        ControlFlow::Continue(())
    });
    assert_eq!(newly_delivered, vec![2, 3]);
}

#[test]
fn pending_evidence_waits_for_admission_before_resolving_old_views() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let tip = commit(&alice, 2, &[*root.hash()]);
    let mut source = CommitGraph::new();
    grant(&mut source, &[&alice]);
    source.ingest_local_commit(root.clone(), 1);
    sign(&mut source, &alice, *root.hash());
    let captured = source.signed_view_point();
    let old = ViewPoint::from_parts(
        captured.delivered_heads().to_vec(),
        captured.write_permissions().clone(),
        captured.authentication_frontiers().clone(),
    );
    source.ingest_local_commit(tip.clone(), 2);
    sign(&mut source, &alice, *tip.hash());

    // A fresh process cannot resolve an assumed dependency from a pending tip.
    let mut peer = CommitGraph::new();
    assert!(matches!(
        peer.resolve(&old),
        Err(ResolveError::MissingCommit(_))
    ));
    assert!(matches!(
        peer.diff(&old, &old),
        Err(DiffError::MissingCommit(_))
    ));
    // Later evidence still resolves old snapshots over actual local history.
    // Neither node is a fragment checkpoint, so there is no batch envelope.
    assert_ne!(root.hash().as_bytes()[0], 0);
    assert_ne!(tip.hash().as_bytes()[0], 0);
    peer.ingest_local_commit(root.clone(), 1);
    peer.ingest_local_commit(tip.clone(), 2);
    sign(&mut peer, &alice, *tip.hash());
    assert!(matches!(
        peer.resolve(&old),
        Err(ResolveError::MissingSignature { .. })
    ));
    assert!(peer.minimal_cover().loose_commits.is_empty());
    sign(&mut peer, &alice, *root.hash());
    assert_eq!(peer.resolve(&old).unwrap().visible(root.hash()), Ok(true));
    assert_eq!(peer.resolve(&old).unwrap().visible(tip.hash()), Ok(false));
    assert_eq!(published(&peer), vec![1, 2]);
    assert!(peer.publishable_commit(root.hash()).is_some());
    assert!(peer.export(&ItemId::Commit(*root.hash())).is_ok());
    assert!(peer.export(&ItemId::Commit(*tip.hash())).is_ok());
    let inventory = peer.minimal_cover();
    assert!(inventory.fragments.is_empty());
    assert_eq!(inventory.loose_commits.len(), 2);
    assert_eq!(peer.as_sedimentree().summary().loose_commits.len(), 2);
    // A directly signed root remains exportable after a newer signature arrives.
    assert!(source.export(&ItemId::Commit(*root.hash())).is_ok());
    assert_eq!(
        peer.diff(&old, &old)
            .unwrap()
            .visibility(root.hash())
            .unwrap(),
        Vis::Both
    );
    let mut forwarded = CommitGraph::new();
    forwarded.merge(&peer);
    assert!(
        forwarded
            .resolve(&old)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert!(
        !forwarded
            .resolve(&old)
            .unwrap()
            .visible(tip.hash())
            .unwrap()
    );
    assert_eq!(published(&forwarded), vec![1, 2]);
    let resolved = peer.resolve(&old).unwrap();
    assert!(resolved.visible(root.hash()).unwrap());
    assert!(!resolved.visible(tip.hash()).unwrap());
    assert_eq!(
        resolved.visible(&CommitHash::from([99; 32])),
        Err(QueryError::UnknownCommit(CommitHash::from([99; 32])))
    );
    // Resolving somebody else's permissions doesn't adopt them as our own.
    assert!(peer.view_point().write_permissions().is_empty());
    assert!(
        !peer
            .resolve(&peer.signed_view_point())
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
}

#[test]
fn unverified_receipts_reject_missing_invalid_or_other_authors_signatures() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let graph = CommitGraph::<u8>::new();
    let unsigned = UnverifiedItem::Commit {
        meta: root.clone(),
        signatures: vec![],
    };
    assert!(matches!(
        unsigned.verify(),
        Err(VerificationError::MissingSignature { author: signer, commit: hash })
            if signer == author(&alice) && hash == *root.hash()
    ));
    // A valid signature by Bob is not coverage for Alice's received metadata.
    assert!(matches!(
        receipt(root.clone(), &bob).verify(),
        Err(VerificationError::MissingSignature { author: signer, commit: hash })
            if signer == author(&alice) && hash == *root.hash()
    ));
    let wrong_signer = signature(&bob, *root.hash());
    assert!(matches!(
        (UnverifiedItem::Commit {
            meta: root.clone(),
            signatures: vec![SignatureEvidence {
                author: author(&alice),
                commit: *root.hash(),
                signature: wrong_signer,
            }],
        })
        .verify(),
        Err(VerificationError::InvalidSignature(_))
    ));
    assert!(graph.delivered_heads().next().is_none());
}

#[test]
fn invalid_or_other_authors_signatures_cannot_authenticate_local_work() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let child = commit(&bob, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice, &bob]);
    // Independently authored local work can exist while its signature is pending.
    graph.ingest_local_commit(root.clone(), 1);
    graph.ingest_local_commit(child.clone(), 2);
    let before = graph.view_point();
    let wrong_signer = signature(&bob, *root.hash());
    assert!(matches!(
        graph.ingest_signature(author(&alice), *root.hash(), wrong_signer),
        Err(SignatureError::InvalidSignature { .. })
    ));
    let mut corrupt: [u8; 64] = signature(&alice, *root.hash()).into();
    corrupt[0] ^= 1;
    assert!(
        graph
            .ingest_signature(author(&alice), *root.hash(), Signature::from(corrupt))
            .is_err()
    );
    assert_eq!(graph.view_point(), before);

    // Even a genuine signature by Bob over Alice's hash isn't proof by Alice.
    graph
        .ingest_signature(author(&bob), *root.hash(), wrong_signer)
        .unwrap();
    sign(&mut graph, &bob, *child.hash());
    let view_point = graph.signed_view_point();
    assert!(
        !graph
            .resolve(&view_point)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert!(
        !graph
            .resolve(&view_point)
            .unwrap()
            .visible(child.hash())
            .unwrap()
    );
    let forged = ViewPoint::from_parts(
        view_point.delivered_heads().to_vec(),
        view_point.write_permissions().clone(),
        HashMap::from([(author(&alice), vec![*child.hash()])]),
    );
    assert!(matches!(
        graph.resolve(&forged),
        Err(ResolveError::MissingSignature { .. })
    ));
    // Local optimism does not make the signed snapshot or publication authentic.
    assert_eq!(
        graph
            .diff(&view_point, &graph.view_point())
            .unwrap()
            .visibility(root.hash())
            .unwrap(),
        Vis::After
    );
    assert!(published(&graph).is_empty());
    sign(&mut graph, &alice, *root.hash());
    assert!(
        graph
            .resolve(&graph.signed_view_point())
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
}

#[test]
fn signed_descendants_do_not_export_unsigned_local_ancestors() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let child = commit(&bob, 2, &[*root.hash()]);
    let mut source = CommitGraph::new();
    grant(&mut source, &[&alice, &bob]);
    source.ingest_local_commit(root.clone(), 1);
    source.ingest_local_commit(child.clone(), 2);
    sign(&mut source, &bob, *child.hash());
    assert!(published(&source).is_empty());
    let working = source.view_point();
    assert!(source.resolve(&working).is_err());
    let mut peer = CommitGraph::new();
    peer.merge(&source);
    assert_eq!(peer.delivered_heads().count(), 0);
    assert!(peer.view_point().authentication_frontiers().is_empty());

    // Revoking Alice while signing is pending doesn't stop the graph syncing
    // once signed, nor let the signature grant her write permission again.
    source.set_write_permissions(&HashMap::from([(author(&bob), WritePermission::Unbounded)]));
    sign(&mut source, &alice, *root.hash());
    assert_eq!(published(&source), vec![1, 2]);
    let current = source.view_point();
    assert!(
        !source
            .resolve(&current)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert!(
        source
            .resolve(&current)
            .unwrap()
            .visible(child.hash())
            .unwrap()
    );
    assert!(
        source
            .resolve(&working)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    peer.merge(&source);
    assert_eq!(
        peer.delivered_heads().collect::<Vec<_>>(),
        vec![*child.hash()]
    );
    assert!(peer.resolve(&working).is_ok());
    assert!(peer.view_point().write_permissions().is_empty());

    let mut visits = 0;
    source.traverse_publishable(|_, _| {
        visits += 1;
        ControlFlow::Break(())
    });
    assert_eq!(visits, 1);
}

#[test]
fn authentication_and_write_frontiers_can_have_multiple_heads() {
    let alice = key(1);
    let root = commit(&alice, 0, &[]);
    let left = commit(&alice, 1, &[*root.hash()]);
    let right = commit(&alice, 2, &[*root.hash()]);
    let extra = commit(&alice, 3, &[]);
    let mut graph = CommitGraph::new();
    for (value, meta) in [root.clone(), left.clone(), right.clone(), extra.clone()]
        .into_iter()
        .enumerate()
    {
        support::receive_commit(&mut graph, meta, value as u8);
    }
    let mut permissions = HashMap::from([(
        author(&alice),
        WritePermission::Bounded(vec![*left.hash(), *right.hash()]),
    )]);
    graph.set_write_permissions(&permissions);
    let bounded = graph.view_point();
    permissions.clear(); // Setter and viewpoint both own snapshots.
    assert_eq!(bounded.authentication_frontiers()[&author(&alice)].len(), 3);
    let resolved = graph.resolve(&bounded).unwrap();
    for meta in [&root, &left, &right] {
        assert!(resolved.visible(meta.hash()).unwrap());
    }
    assert!(!resolved.visible(extra.hash()).unwrap());
    graph.set_write_permissions(&permissions);
    assert!(
        !graph
            .resolve(&graph.view_point())
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert!(
        graph
            .resolve(&bounded)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );

    let mut peer = CommitGraph::new();
    peer.merge(&graph);
    assert!(
        peer.resolve(&bounded)
            .unwrap()
            .visible(left.hash())
            .unwrap()
    );
    assert!(
        peer.resolve(&bounded)
            .unwrap()
            .visible(right.hash())
            .unwrap()
    );
    let added = peer
        .diff(&peer.view_point(), &bounded)
        .unwrap()
        .changed_commits()
        .map(|c| c.commit_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        added,
        BTreeSet::from([*root.hash(), *left.hash(), *right.hash()])
    );
}

#[test]
fn merge_withholds_queued_descendants_of_local_work() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let missing = commit(&bob, 2, &[]);
    let queued = commit(&bob, 3, &[*root.hash(), *missing.hash()]);
    let tip = commit(&bob, 4, &[*queued.hash()]);
    let mut source = CommitGraph::new();
    source.ingest_local_commit(root.clone(), 1);
    source.ingest_local_commit(tip.clone(), 4);
    source.ingest_local_commit(queued.clone(), 3);
    sign(&mut source, &bob, *tip.hash());
    let mut peer = CommitGraph::new();
    peer.merge(&source);
    // If queued local descendants leaked, these receipts would release them.
    support::receive_commit(&mut peer, root.clone(), 1);
    support::receive_commit(&mut peer, missing.clone(), 2);
    assert_eq!(peer.delivered_heads().count(), 2);

    sign(&mut source, &alice, *root.hash());
    peer.merge(&source);
    assert_eq!(peer.delivered_heads().count(), 2);
    // The source cannot export the queued branch until its own gap is filled.
    source.ingest_local_commit(missing.clone(), 2);
    peer.merge(&source);
    // Delivery and tip coverage alone do not export the unsigned middle item.
    assert_eq!(peer.delivered_heads().count(), 2);
    sign(&mut source, &bob, *missing.hash());
    sign(&mut source, &bob, *queued.hash());
    peer.merge(&source);
    assert_eq!(
        peer.delivered_heads().collect::<Vec<_>>(),
        vec![*tip.hash()]
    );
    assert!(peer.is_ancestor(root.hash(), tip.hash()));
}

#[test]
fn received_children_require_verified_parents_not_local_optimism() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let child = commit(&bob, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice, &bob]);
    graph.ingest_local_commit(root.clone(), 1);
    let before = graph.view_point();
    let pending = receipt(child.clone(), &bob).verify().unwrap();
    assert!(!graph.has_admitted_commit(root.hash()));
    assert!(matches!(
        graph.ingest(pending.clone(), |_| panic!("missing dependency must precede payload lookup")),
        Err(IngestError::MissingDependency(hash)) if hash == *root.hash()
    ));
    assert_eq!(graph.view_point(), before);
    assert!(published(&graph).is_empty());

    sign(&mut graph, &alice, *root.hash());
    assert!(graph.has_admitted_commit(root.hash()));
    graph.ingest(pending, |_| 2).unwrap();
    assert_eq!(published(&graph), vec![1, 2]);
    assert!(
        graph
            .resolve(&graph.signed_view_point())
            .unwrap()
            .visible(child.hash())
            .unwrap()
    );
}

#[test]
fn received_commits_need_their_own_authors_signature_coverage() {
    let alice = key(1);
    let bob = key(2);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice, &bob]);
    let root = commit(&bob, 1, &[]);
    let child = commit(&alice, 2, &[*root.hash()]);
    let before = graph.signed_view_point();
    let alice_evidence = SignatureEvidence {
        author: author(&alice),
        commit: *child.hash(),
        signature: signature(&alice, *child.hash()),
    };
    assert!(matches!(
        (UnverifiedItem::Commit {
            meta: root.clone(),
            signatures: vec![alice_evidence],
        })
        .verify(),
        Err(VerificationError::InvalidItem) // Signature target is not an item member.
    ));
    // Even a directly signed child cannot enter before its verified parent.
    let pending = receipt(child.clone(), &alice).verify().unwrap();
    assert!(matches!(
        graph.ingest(pending.clone(), |_| panic!("missing dependency must precede payload lookup")),
        Err(IngestError::MissingDependency(hash)) if hash == *root.hash()
    ));
    assert_eq!(graph.signed_view_point(), before);
    assert!(published(&graph).is_empty());

    support::receive_commit(&mut graph, root.clone(), 1);
    assert!(graph.has_admitted_commit(root.hash()));
    graph.ingest(pending, |_| 2).unwrap();
    let view_point = graph.signed_view_point();
    for meta in [&root, &child] {
        assert!(
            graph
                .resolve(&view_point)
                .unwrap()
                .visible(meta.hash())
                .unwrap()
        );
        assert!(
            !graph
                .resolve(&before)
                .unwrap()
                .visible(meta.hash())
                .unwrap()
        );
    }
    assert_eq!(published(&graph), vec![1, 2]);

    // Neither a duplicate local insertion nor signatures can undo revocation.
    grant(&mut graph, &[&alice]);
    graph.ingest_local_commit(root.clone(), 42);
    assert_eq!(
        graph.ingest_signature(author(&bob), *root.hash(), signature(&bob, *root.hash())),
        Err(SignatureError::NotPendingLocalCommit(*root.hash()))
    );
    assert!(
        !graph
            .resolve(&graph.view_point())
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert!(
        graph
            .resolve(&view_point)
            .unwrap()
            .visible(root.hash())
            .unwrap()
    );
    assert_eq!(published(&graph), vec![1, 2]);
}
