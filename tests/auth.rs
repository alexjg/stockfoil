//! Authentication, permission snapshots, and verified receipt boundaries.
//! Received history is authenticated before buffering and waits for admitted
//! dependencies; local work must arrive in causal order and can wait for authentication.
mod support;

use support::{author, key, sign, signature};

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
    panic::{AssertUnwindSafe, catch_unwind},
};

use ed25519_dalek::SigningKey;
use stockfoil::{
    BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, DiffError, ItemId, QueryError,
    Signature, SignatureError, SignatureEvidence, UnverifiedItem, VerificationError, ViewPoint,
    Vis, WritePermission,
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

#[test]
fn missing_local_parents_panic_without_retaining_work_and_allow_caller_retry() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let missing = commit(&alice, 2, &[]);
    let child = commit(&alice, 3, &[*root.hash(), *missing.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    graph.ingest_local_commit(root.clone(), 1);
    let before = graph.view_point();
    let signed_before = graph.signed_view_point();

    // Even with the first parent delivered, every parent must exist before
    // insertion mutates topology, payload storage, or pending-local tracking.
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            graph.ingest_local_commit(child.clone(), 3);
        }))
        .is_err()
    );
    assert_eq!(graph.view_point(), before);
    assert_eq!(graph.signed_view_point(), signed_before);
    assert!(!graph.has_commit(child.hash()));
    assert!(graph.publishable_commit(child.hash()).is_none());
    assert!(graph.export(&ItemId::Commit(*child.hash())).is_err());
    assert!(graph.minimal_cover().loose_commits.is_empty());
    assert!(graph.minimal_cover().fragments.is_empty());
    let rejected = ViewPoint::from_parts(vec![*child.hash()], HashMap::new());
    assert_eq!(
        graph.diff(&before, &rejected).unwrap_err(),
        DiffError::MissingCommit(*child.hash())
    );
    assert_eq!(
        graph.ingest_signature(
            author(&alice),
            *child.hash(),
            signature(&alice, *child.hash())
        ),
        Err(SignatureError::NotPendingLocalCommit(*child.hash()))
    );
    let mut values = Vec::new();
    graph.traverse(|_, value| {
        values.push(*value);
        ControlFlow::Continue(())
    });
    assert_eq!(values, vec![1]);

    graph.ingest_local_commit(missing.clone(), 2);
    sign(&mut graph, &alice, *root.hash());
    sign(&mut graph, &alice, *missing.hash());
    // Filling the gap never releases rejected local work or its signature.
    assert_eq!(
        graph.heads().collect::<BTreeSet<_>>(),
        BTreeSet::from([*root.hash(), *missing.hash()])
    );
    assert_eq!(
        graph.ingest_signature(
            author(&alice),
            *child.hash(),
            signature(&alice, *child.hash())
        ),
        Err(SignatureError::NotPendingLocalCommit(*child.hash()))
    );
    let mut peer = CommitGraph::new();
    peer.merge(&graph);
    assert_eq!(peer.heads().count(), 2);

    // The caller must retry, and the retry supplies the payload actually kept.
    graph.ingest_local_commit(child.clone(), 99);
    assert!(!graph.has_commit(child.hash()));
    sign(&mut graph, &alice, *child.hash());
    values.clear();
    graph.traverse(|_, value| {
        values.push(*value);
        ControlFlow::Continue(())
    });
    assert_eq!(values, vec![1, 2, 99]);
    peer.merge(&graph);
    assert!(peer.has_commit(child.hash()));
    assert_eq!(peer.heads().collect::<Vec<_>>(), vec![*child.hash()]);
}

#[test]
fn signatures_require_delivered_local_work_but_can_precede_parent_admission() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let tip = commit(&alice, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    let empty = graph.signed_view_point();
    assert_eq!(
        graph.ingest_signature(author(&alice), *tip.hash(), signature(&alice, *tip.hash())),
        Err(SignatureError::NotPendingLocalCommit(*tip.hash()))
    );
    graph.ingest_local_commit(root.clone(), 1);
    graph.ingest_local_commit(tip.clone(), 2);
    let working = graph.view_point();
    assert_eq!(working.heads(), &[*tip.hash()]);
    sign(&mut graph, &alice, *tip.hash());
    assert_eq!(graph.view_point(), working);
    assert_eq!(graph.signed_view_point(), empty);
    for meta in [&root, &tip] {
        assert!(!graph.has_commit(meta.hash()));
        assert!(graph.publishable_commit(meta.hash()).is_none());
        assert!(graph.export(&ItemId::Commit(*meta.hash())).is_err());
    }

    let mut peer = CommitGraph::new();
    grant(&mut peer, &[&alice]);
    peer.merge(&graph);
    assert!(peer.heads().next().is_none());
    // Signing the delivered tip cannot synthesize an unsigned root receipt.
    sign(&mut graph, &alice, *root.hash());
    assert_eq!(graph.view_point(), working);
    peer.merge(&graph);
    for doc in [&graph, &peer] {
        assert_eq!(doc.signed_view_point(), working);
    }
    assert_eq!(
        graph.ingest_signature(author(&alice), *tip.hash(), signature(&alice, *tip.hash())),
        Err(SignatureError::NotPendingLocalCommit(*tip.hash()))
    );
}

#[test]
fn admission_excludes_unknown_and_pending_local_commits() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let child = commit(&alice, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    assert!(!graph.has_commit(root.hash()));

    // Local drafts require delivered, not necessarily admitted, parents.
    graph.ingest_local_commit(root.clone(), 1);
    graph.ingest_local_commit(child.clone(), 2);
    let child_receipt = receipt(child.clone(), &alice).verify().unwrap();
    assert!(!graph.has_commit(child.hash()));
    assert_eq!(graph.heads().collect::<Vec<_>>(), vec![*child.hash()]);

    // Delivery alone cannot discharge a received item's dependencies.
    let mut graph = graph.clone();
    assert!(!graph.has_commit(root.hash()));
    assert!(!graph.has_commit(child.hash()));
    let report = graph
        .ingest(child_receipt, |_| {
            panic!("preserve delivered local payload")
        })
        .unwrap();
    assert!(report.admitted.is_empty());
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 1);
    assert_eq!(graph.pending_items(), 1);
    assert_eq!(
        graph
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*root.hash()])
    );
    assert!(graph.minimal_cover().loose_commits.is_empty());
    assert!(graph.minimal_cover().fragments.is_empty());

    // Received evidence promotes local copies without replacing their payloads.
    let report = graph
        .ingest(receipt(root.clone(), &alice).verify().unwrap(), |_| {
            panic!("preserve local payload")
        })
        .unwrap();
    assert_eq!(
        report.admitted.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())])
    );
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 0);
    assert_eq!(graph.pending_items(), 0);
    assert!(graph.missing_dependencies().next().is_none());
    assert!(graph.has_commit(root.hash()));
    assert!(graph.has_commit(child.hash()));

    let duplicate = graph
        .ingest(receipt(child.clone(), &alice).verify().unwrap(), |_| {
            panic!("duplicate receipt must preserve its payload")
        })
        .unwrap();
    assert_eq!(duplicate.admitted, [ItemId::Commit(*child.hash())]);
    assert!(duplicate.rejected.is_empty());
    assert_eq!(duplicate.pending, 0);

    // Duplicate local insertion must not demote admitted history.
    graph.ingest_local_commit(root.clone(), 99);
    assert!(graph.has_commit(root.hash()));
    let mut values = Vec::new();
    graph.traverse(|_, value| {
        values.push(*value);
        ControlFlow::Continue(())
    });
    assert_eq!(values, vec![1, 2]);
    let mut peer = CommitGraph::new();
    peer.merge(&graph);
    assert!(peer.has_commit(root.hash()));
    assert!(peer.has_commit(child.hash()));
}

#[test]
fn signing_or_merging_a_local_parent_drains_received_descendants() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let child = commit(&alice, 2, &[*root.hash()]);
    let tip = commit(&alice, 3, &[*child.hash()]);
    for via_merge in [false, true] {
        let mut graph = CommitGraph::new();
        grant(&mut graph, &[&alice]);
        let report = graph.ingest_local_commit(root.clone(), 1);
        assert!(report.admitted.is_empty());
        assert!(report.rejected.is_empty());
        assert_eq!(report.pending, 0);
        let working = graph.view_point();
        for (meta, value, pending) in [(&tip, 3, 1), (&child, 2, 2)] {
            let mut callbacks = 0;
            let report = graph
                .ingest(receipt(meta.clone(), &alice).verify().unwrap(), |_| {
                    callbacks += 1;
                    value
                })
                .unwrap();
            assert_eq!(callbacks, 1);
            assert!(report.admitted.is_empty());
            assert!(report.rejected.is_empty());
            assert_eq!(report.pending, pending);
            assert_eq!(graph.view_point(), working);
        }
        assert_eq!(graph.pending_items(), 2);
        assert_eq!(
            graph
                .missing_dependencies()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([*root.hash(), *child.hash()])
        );
        let report = if via_merge {
            let mut source = CommitGraph::new();
            source
                .ingest(receipt(root.clone(), &alice).verify().unwrap(), |_| 99)
                .unwrap();
            graph.merge(&source)
        } else {
            graph
                .ingest_signature(
                    author(&alice),
                    *root.hash(),
                    signature(&alice, *root.hash()),
                )
                .unwrap()
        };
        assert_eq!(
            report.admitted.into_iter().collect::<BTreeSet<_>>(),
            [&root, &child, &tip]
                .map(|meta| ItemId::Commit(*meta.hash()))
                .into()
        );
        assert!(report.rejected.is_empty());
        assert_eq!(report.pending, 0);
        assert_eq!(graph.pending_items(), 0);
        assert!(graph.missing_dependencies().next().is_none());
        assert_eq!(graph.signed_view_point().heads(), &[*tip.hash()]);
        let mut values = Vec::new();
        graph.traverse(|_, value| {
            values.push(*value);
            ControlFlow::Continue(())
        });
        assert_eq!(values, [1, 2, 3]); // Never replace the delivered local payload.
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
    assert_eq!(
        graph.diff(&before, &before).unwrap().visibility(c.hash()),
        Ok(Vis::Neither)
    );

    // Verification and payload capture happen now, but the buffered frontier
    // cannot supply permission evidence until its dependencies are admitted.
    let pending = receipt(frontier.clone(), &bob).verify().unwrap();
    let mut callbacks = 0;
    let report = graph
        .ingest(pending, |_| {
            callbacks += 1;
            3
        })
        .unwrap();
    assert_eq!(callbacks, 1);
    assert!(report.admitted.is_empty());
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 1);
    assert_eq!(
        graph
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([*missing.hash()])
    );
    assert_eq!(graph.view_point(), before); // Reading its parent link isn't proof.
    assert_eq!(
        graph.diff(&before, &before).unwrap().visibility(c.hash()),
        Ok(Vis::Neither)
    );
    assert!(!graph.has_commit(frontier.hash()));
    assert!(graph.publishable_commit(frontier.hash()).is_none());
    assert!(graph.export(&ItemId::Commit(*frontier.hash())).is_err());
    let mut delivered = Vec::new();
    graph.traverse(|meta, _| {
        delivered.push(*meta.hash());
        ControlFlow::Continue(())
    });
    assert_eq!(delivered, vec![*c.hash()]);
    let unavailable = ViewPoint::from_parts(vec![*frontier.hash()], HashMap::new());
    assert_eq!(
        graph.diff(&before, &unavailable).unwrap_err(),
        DiffError::MissingCommit(*frontier.hash())
    );
    support::receive_commit(&mut graph, missing, 2);
    assert_eq!(graph.pending_items(), 0);
    let after = graph.view_point();
    let diff = graph.diff(&before, &after).unwrap();
    // The newly delivered frontier reveals an older commit, not its own denied
    // author's writes, and cannot retroactively grant access in the old view.
    assert_eq!(diff.visibility(c.hash()), Ok(Vis::After));
    assert_eq!(diff.visibility(frontier.hash()), Ok(Vis::Neither));
    assert_eq!(
        graph.diff(&before, &before).unwrap().visibility(c.hash()),
        Ok(Vis::Neither)
    );
    let changes = diff.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].commit_id, *c.hash());
    assert_eq!(changes[0].visibility_delta, stockfoil::VisDiff::After);
    let mut newly_delivered = Vec::new();
    graph.traverse_since(before.heads(), |_, value| {
        newly_delivered.push(*value);
        ControlFlow::Continue(())
    });
    assert_eq!(newly_delivered, vec![2, 3]);
}

#[test]
fn historical_views_use_delivered_history_independently_of_admission() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let tip = commit(&alice, 2, &[*root.hash()]);
    let mut source = CommitGraph::new();
    grant(&mut source, &[&alice]);
    source.ingest_local_commit(root.clone(), 1);
    sign(&mut source, &alice, *root.hash());
    let captured = source.signed_view_point();
    let old = ViewPoint::from_parts(
        captured.heads().to_vec(),
        captured.write_permissions().clone(),
    );
    source.ingest_local_commit(tip.clone(), 2);
    sign(&mut source, &alice, *tip.hash());

    // A fresh process cannot compare a snapshot whose head is missing.
    let mut peer = CommitGraph::new();
    assert!(matches!(
        peer.diff(&old, &old),
        Err(DiffError::MissingCommit(_))
    ));
    // Delivered drafts suffice for diffing, even before admission.
    // Neither node is a fragment checkpoint, so there is no batch envelope.
    assert_ne!(root.hash().as_bytes()[0], 0);
    assert_ne!(tip.hash().as_bytes()[0], 0);
    peer.ingest_local_commit(root.clone(), 1);
    peer.ingest_local_commit(tip.clone(), 2);
    sign(&mut peer, &alice, *tip.hash());
    let membership = peer.diff(&old, &old).unwrap();
    assert_eq!(membership.visibility(root.hash()), Ok(Vis::Both));
    assert_eq!(membership.visibility(tip.hash()), Ok(Vis::Neither));
    assert!(!peer.has_commit(root.hash()));
    assert!(peer.minimal_cover().loose_commits.is_empty());
    sign(&mut peer, &alice, *root.hash());
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
    for graph in [&peer, &forwarded] {
        let membership = graph.diff(&old, &old).unwrap();
        assert_eq!(membership.visibility(root.hash()), Ok(Vis::Both));
        assert_eq!(membership.visibility(tip.hash()), Ok(Vis::Neither));
        let unknown = CommitHash::from([99; 32]);
        assert_eq!(
            membership.visibility(&unknown),
            Err(QueryError::UnknownCommit(unknown))
        );
        // Querying a transported snapshot must not adopt its permissions.
        let current = graph.view_point();
        assert!(current.write_permissions().is_empty());
        assert_eq!(
            graph
                .diff(&current, &current)
                .unwrap()
                .visibility(root.hash()),
            Ok(Vis::Neither)
        );
    }
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
    // Local optimism does not make the signed snapshot or publication authentic.
    assert_eq!(
        graph
            .diff(&view_point, &graph.view_point())
            .unwrap()
            .visibility(root.hash())
            .unwrap(),
        Vis::After
    );
    assert!(!graph.has_commit(root.hash()));
    assert!(!graph.has_commit(child.hash()));
    sign(&mut graph, &alice, *root.hash());
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
    let working = source.view_point();
    let mut peer = CommitGraph::new();
    peer.merge(&source);
    assert_eq!(peer.heads().count(), 0);
    assert!(peer.view_point().heads().is_empty());

    // Revoking Alice while signing is pending doesn't stop the graph syncing
    // once signed, nor let the signature grant her write permission again.
    source.set_write_permissions(&HashMap::from([(author(&bob), WritePermission::Unbounded)]));
    sign(&mut source, &alice, *root.hash());
    let current = source.view_point();
    let diff = source.diff(&working, &current).unwrap();
    assert_eq!(diff.visibility(root.hash()), Ok(Vis::Before));
    assert_eq!(diff.visibility(child.hash()), Ok(Vis::Both));
    peer.merge(&source);
    assert_eq!(peer.heads().collect::<Vec<_>>(), vec![*child.hash()]);
    assert!(peer.view_point().write_permissions().is_empty());
}

#[test]
fn history_and_write_frontiers_can_have_multiple_heads() {
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
    assert_eq!(bounded.heads().len(), 3);
    graph.set_write_permissions(&permissions);

    let mut peer = CommitGraph::new();
    peer.merge(&graph);
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
fn merge_withholds_delivered_descendants_of_unsigned_local_work() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let other_root = commit(&bob, 2, &[]);
    let middle = commit(&bob, 3, &[*root.hash(), *other_root.hash()]);
    let tip = commit(&bob, 4, &[*middle.hash()]);
    let mut source = CommitGraph::new();
    source.ingest_local_commit(root.clone(), 1);
    source.ingest_local_commit(other_root.clone(), 2);
    source.ingest_local_commit(middle.clone(), 3);
    source.ingest_local_commit(tip.clone(), 4);
    assert_eq!(source.heads().collect::<Vec<_>>(), vec![*tip.hash()]);
    sign(&mut source, &bob, *tip.hash());
    let mut peer = CommitGraph::new();
    peer.merge(&source);
    assert_eq!(peer.heads().count(), 0);
    for meta in [&root, &other_root, &middle, &tip] {
        assert!(!source.has_commit(meta.hash()));
        assert!(!peer.has_commit(meta.hash()));
    }
    // The receiver having admitted parents cannot make source drafts exportable.
    support::receive_commit(&mut peer, root.clone(), 1);
    support::receive_commit(&mut peer, other_root.clone(), 2);
    assert_eq!(peer.heads().count(), 2);

    sign(&mut source, &alice, *root.hash());
    peer.merge(&source);
    assert_eq!(peer.heads().count(), 2);
    sign(&mut source, &bob, *other_root.hash());
    peer.merge(&source);
    // Delivered topology and a signed tip do not authenticate the middle item.
    assert_eq!(peer.heads().count(), 2);
    assert!(!source.has_commit(middle.hash()));
    assert!(!source.has_commit(tip.hash()));
    sign(&mut source, &bob, *middle.hash());
    peer.merge(&source);
    assert_eq!(peer.heads().collect::<Vec<_>>(), vec![*tip.hash()]);
    assert!(peer.is_ancestor(root.hash(), tip.hash()));
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
    let report = graph.ingest(pending, |_| 2).unwrap();
    assert!(report.admitted.is_empty());
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 1);
    assert_eq!(graph.signed_view_point(), before);
    assert_eq!(graph.view_point(), before);

    support::receive_commit(&mut graph, root.clone(), 1);
    assert_eq!(graph.pending_items(), 0);
    assert!(graph.has_commit(root.hash()));
    assert!(graph.has_commit(child.hash()));
    let admitted = graph.signed_view_point();
    let diff = graph.diff(&before, &admitted).unwrap();
    for meta in [&root, &child] {
        assert_eq!(diff.visibility(meta.hash()), Ok(Vis::After));
    }

    // Neither a duplicate local insertion nor signatures can undo revocation.
    grant(&mut graph, &[&alice]);
    graph.ingest_local_commit(root.clone(), 42);
    assert_eq!(
        graph.ingest_signature(author(&bob), *root.hash(), signature(&bob, *root.hash())),
        Err(SignatureError::NotPendingLocalCommit(*root.hash()))
    );
    let current = graph.view_point();
    let diff = graph.diff(&admitted, &current).unwrap();
    assert_eq!(diff.visibility(root.hash()), Ok(Vis::Before));
    assert_eq!(diff.visibility(child.hash()), Ok(Vis::Both));
}
