mod support;

use support::{author, key, sign, signature};

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::SigningKey;
use stockfoil::{
    BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, DiffError, QueryError,
    ResolveError, Signature, SignatureError, ViewPoint, Vis, WritePermission,
};

fn commit(key: &SigningKey, value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents: parents.to_vec(),
        author: author(key),
        payload: BlobHash::of_contents(&[value]),
    })
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
fn remote_signature_arrival_changes_only_new_view_points() {
    let alice = key(1);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    let c = commit(&alice, 1, &[]);
    let hash = *c.hash();
    graph.ingest_commit(c, 1);
    let before = graph.view_point();
    assert!(!graph.resolve(&before).unwrap().visible(&hash).unwrap());
    // Incoming unsigned data is forwardable, but has no optimistic allowance.
    assert_eq!(published(&graph), vec![1]);

    sign(&mut graph, &alice, hash);
    let after = graph.view_point();
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

    // Duplicate evidence and commits do not change the viewpoint.
    sign(&mut graph, &alice, hash);
    graph.ingest_commit(commit(&alice, 1, &[]), 99);
    assert_eq!(graph.view_point(), after);
}

#[test]
fn signatures_can_arrive_before_commits_and_causal_delivery() {
    let alice = key(1);
    let root = commit(&alice, 1, &[]);
    let tip = commit(&alice, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice]);
    sign(&mut graph, &alice, *tip.hash());
    graph.ingest_commit(tip.clone(), 2);
    let detached = graph.view_point();
    assert!(detached.delivered_heads().is_empty());
    assert!(detached.authentication_frontiers().is_empty());
    assert_eq!(
        graph.resolve(&detached).unwrap().visible(tip.hash()),
        Err(QueryError::UnknownCommit(*tip.hash()))
    );

    // Queued incoming data and its signature can be forwarded before the root.
    let mut peer = CommitGraph::new();
    grant(&mut peer, &[&alice]);
    peer.merge(&graph);
    peer.ingest_commit(root.clone(), 1);
    graph.ingest_commit(root.clone(), 1);
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
    graph.ingest_commit(c.clone(), 1);
    sign(&mut graph, &alice, *c.hash());
    let before = graph.view_point();
    assert!(!graph.resolve(&before).unwrap().visible(c.hash()).unwrap());

    graph.ingest_commit(frontier.clone(), 3);
    assert_eq!(graph.view_point(), before); // Merely reading its parent link isn't enough.
    graph.ingest_commit(missing, 2);
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
fn newer_evidence_resolves_an_old_view_point_without_expanding_it() {
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

    // A fresh process has only the later signature, never the original one.
    let mut peer = CommitGraph::new();
    assert!(matches!(
        peer.resolve(&old),
        Err(ResolveError::MissingCommit(_))
    ));
    assert!(matches!(
        peer.diff(&old, &old),
        Err(DiffError::MissingCommit(_))
    ));
    peer.ingest_commit(root.clone(), 1);
    assert!(matches!(
        peer.resolve(&old),
        Err(ResolveError::MissingSignature { .. })
    ));
    // Diff is just calculation; it does not claim to verify a received viewpoint.
    assert_eq!(
        peer.diff(&old, &old)
            .unwrap()
            .visibility(root.hash())
            .unwrap(),
        Vis::Both
    );
    sign(&mut peer, &alice, *tip.hash());
    assert!(peer.resolve(&old).is_err()); // Still missing the ancestry evidence.
    peer.ingest_commit(tip.clone(), 2);
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
fn invalid_or_other_authors_signatures_cannot_authenticate_a_commit() {
    let alice = key(1);
    let bob = key(2);
    let root = commit(&alice, 1, &[]);
    let child = commit(&bob, 2, &[*root.hash()]);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice, &bob]);
    graph.ingest_commit(root.clone(), 1);
    graph.ingest_commit(child.clone(), 2);
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
        graph
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
    // A duplicate received commit cannot be promoted to optimistic local trust.
    graph.ingest_local_commit(root.clone(), 42);
    assert_eq!(
        graph
            .diff(&graph.view_point(), &graph.view_point())
            .unwrap()
            .visibility(root.hash())
            .unwrap(),
        Vis::Neither
    );
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
        graph.ingest_commit(meta, value as u8);
    }
    sign(&mut graph, &alice, *left.hash());
    sign(&mut graph, &alice, *right.hash());
    sign(&mut graph, &alice, *extra.hash());
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
    source.ingest_commit(tip.clone(), 4);
    source.ingest_commit(queued.clone(), 3);
    let mut peer = CommitGraph::new();
    peer.merge(&source);
    // If the queued descendants leaked, these two commits would release them.
    peer.ingest_commit(root.clone(), 1);
    peer.ingest_commit(missing.clone(), 2);
    assert_eq!(peer.delivered_heads().count(), 2);

    sign(&mut source, &alice, *root.hash());
    peer.merge(&source);
    assert_eq!(
        peer.delivered_heads().collect::<Vec<_>>(),
        vec![*tip.hash()]
    );
    assert!(peer.is_ancestor(root.hash(), tip.hash()));
}

#[test]
fn received_commits_need_their_own_authors_signature_coverage() {
    let alice = key(1);
    let bob = key(2);
    let mut graph = CommitGraph::new();
    grant(&mut graph, &[&alice, &bob]);
    let unsigned = graph.commit_by(author(&bob), &[1]);
    graph.ingest_commit(unsigned.clone(), 1);
    let signed = graph.commit_by(author(&alice), &[2]);
    graph.ingest_local_commit(signed.clone(), 2);
    sign(&mut graph, &alice, *signed.hash());
    let view_point = graph.signed_view_point();
    assert!(
        !graph
            .resolve(&view_point)
            .unwrap()
            .visible(unsigned.hash())
            .unwrap()
    );
    assert!(
        graph
            .resolve(&view_point)
            .unwrap()
            .visible(signed.hash())
            .unwrap()
    );
    assert_eq!(published(&graph), vec![1, 2]);

    sign(&mut graph, &bob, *unsigned.hash());
    assert!(
        graph
            .resolve(&graph.signed_view_point())
            .unwrap()
            .visible(unsigned.hash())
            .unwrap()
    );
    assert!(
        !graph
            .resolve(&view_point)
            .unwrap()
            .visible(unsigned.hash())
            .unwrap()
    );
}
