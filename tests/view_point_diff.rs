//! Graph participation and evidence contracts, independent of any CRDT.

mod support;

use std::{collections::HashMap, ops::ControlFlow};
use support::{author, key, sign, signature};

use ed25519_dalek::SigningKey;
use stockfoil::{CommitGraph, CommitHash, ViewPointDiff, Vis, VisDiff, WritePermission};

fn prepare(graph: &mut CommitGraph<u8>, key: &SigningKey, value: u8) -> CommitHash {
    let meta = graph.commit_by(author(key), &[value]);
    let hash = *meta.hash();
    graph.ingest_local_commit(meta, value);
    hash
}

fn append(graph: &mut CommitGraph<u8>, key: &SigningKey, value: u8) -> CommitHash {
    let hash = prepare(graph, key, value);
    sign(graph, key, hash);
    hash
}

fn assert_changes(diff: &ViewPointDiff<'_>, removed: &[CommitHash], added: &[CommitHash]) {
    let changes = diff.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), removed.len() + added.len());
    let actual = changes
        .into_iter()
        .map(|c| (c.commit_id, c.visibility_delta))
        .collect::<HashMap<_, _>>();
    let expected = removed
        .iter()
        .map(|hash| (*hash, VisDiff::Before))
        .chain(added.iter().map(|hash| (*hash, VisDiff::After)))
        .collect::<HashMap<_, _>>();
    assert_eq!(actual, expected);
}

#[test]
fn revocation_diffs_are_bidirectional_and_do_not_retract_delivery() {
    let alice_key = key(1);
    let bob_key = key(2);
    let alice_id = author(&alice_key);
    let bob_id = author(&bob_key);
    let allowed = HashMap::from([
        (alice_id, WritePermission::Unbounded),
        (bob_id, WritePermission::Unbounded),
    ]);
    let mut alice = CommitGraph::new();
    alice.set_write_permissions(&allowed);
    let original = append(&mut alice, &alice_key, 0);
    let mut bob = CommitGraph::new();
    bob.set_write_permissions(&allowed);
    bob.merge(&alice);

    let bound = append(&mut alice, &alice_key, 1);
    let alice_branch = alice.view_point();
    let concurrent = append(&mut bob, &bob_key, 2);
    let bob_branch = bob.view_point();
    let revoked_title = append(&mut alice, &alice_key, 3);
    let revoked_key = append(&mut alice, &alice_key, 4);
    bob.merge(&alice);
    let descendant = append(&mut bob, &bob_key, 5);
    alice.merge(&bob);
    assert!(alice.is_ancestor(&revoked_key, &descendant));

    let branches = alice.diff(&alice_branch, &bob_branch).unwrap();
    assert_eq!(branches.visibility(&original).unwrap(), Vis::Both);
    assert_eq!(branches.visibility(&bound).unwrap(), Vis::Before);
    assert_eq!(branches.visibility(&concurrent).unwrap(), Vis::After);
    assert_eq!(branches.visibility(&revoked_title).unwrap(), Vis::Neither);
    assert_changes(&branches, &[bound], &[concurrent]);

    let before = alice.view_point();
    alice.set_write_permissions(&HashMap::from([
        (alice_id, WritePermission::Bounded(vec![bound])),
        (bob_id, WritePermission::Unbounded),
    ]));
    let after = alice.view_point();
    assert_eq!(before.delivered_heads(), after.delivered_heads());
    let mut redelivered = 0;
    alice.traverse_since(before.delivered_heads(), |_, _| {
        redelivered += 1;
        ControlFlow::Continue(())
    });
    assert_eq!(redelivered, 0);

    let diff = alice.diff(&before, &after).unwrap();
    assert_eq!(diff.visibility(&bound).unwrap(), Vis::Both);
    assert_eq!(diff.visibility(&descendant).unwrap(), Vis::Both);
    assert_eq!(diff.visibility(&revoked_title).unwrap(), Vis::Before);
    assert_changes(&diff, &[revoked_title, revoked_key], &[]);
    assert_changes(
        &alice.diff(&after, &before).unwrap(),
        &[],
        &[revoked_title, revoked_key],
    );
    assert_changes(&alice.diff(&after, &after).unwrap(), &[], &[]);
    assert!(
        alice
            .resolve(&before)
            .unwrap()
            .visible(&revoked_key)
            .unwrap()
    );
    assert!(!bob.resolve(&after).unwrap().visible(&revoked_key).unwrap());
    assert!(bob.resolve(&after).unwrap().visible(&descendant).unwrap());

    // Default denial, explicit denial, and empty bounds have equal participation.
    alice.set_write_permissions(&HashMap::from([(bob_id, WritePermission::Unbounded)]));
    let omitted = alice.view_point();
    for permission in [WritePermission::Denied, WritePermission::Bounded(vec![])] {
        alice.set_write_permissions(&HashMap::from([
            (alice_id, permission),
            (bob_id, WritePermission::Unbounded),
        ]));
        assert_changes(
            &alice.diff(&omitted, &alice.view_point()).unwrap(),
            &[],
            &[],
        );
    }
    alice.set_write_permissions(&allowed);
    assert_changes(&alice.diff(&before, &alice.view_point()).unwrap(), &[], &[]);
    assert_eq!(
        alice.view_point().delivered_heads(),
        before.delivered_heads()
    );
}

#[test]
fn signing_a_frontier_changes_signed_participation_not_captured_working_view_points() {
    let key = key(42);
    let mut graph = CommitGraph::new();
    graph.set_write_permissions(&HashMap::from([(author(&key), WritePermission::Unbounded)]));
    let base = prepare(&mut graph, &key, 0);
    assert!(graph.resolve(&graph.view_point()).is_err());
    sign(&mut graph, &key, base);
    let initial = graph.view_point();
    assert!(graph.resolve(&initial).is_ok());

    let first = prepare(&mut graph, &key, 1);
    let first_working = graph.view_point();
    let first_signed = graph.signed_view_point();
    assert_eq!(
        first_working.delivered_heads(),
        first_signed.delivered_heads()
    );
    assert!(graph.resolve(&first_working).is_err());
    assert!(graph.resolve(&first_signed).is_ok());
    let pending = graph.diff(&first_signed, &first_working).unwrap();
    assert_eq!(pending.visibility(&base).unwrap(), Vis::Both);
    assert_eq!(pending.visibility(&first).unwrap(), Vis::After);
    assert_changes(&pending, &[], &[first]);

    assert!(
        graph
            .ingest_signature(author(&key), first, signature(&key, base))
            .is_err()
    );
    assert!(graph.resolve(&first_working).is_err());
    assert_changes(
        &graph
            .diff(&first_signed, &graph.signed_view_point())
            .unwrap(),
        &[],
        &[],
    );

    let tip = prepare(&mut graph, &key, 2);
    let batch_working = graph.view_point();
    let batch_signed = graph.signed_view_point();
    assert!(graph.resolve(&batch_working).is_err());
    assert_changes(
        &graph.diff(&batch_signed, &batch_working).unwrap(),
        &[],
        &[first, tip],
    );
    let mut peer = CommitGraph::new();
    peer.merge(&graph);
    assert_eq!(peer.delivered_heads().collect::<Vec<_>>(), vec![base]);

    sign(&mut graph, &key, tip);
    assert!(!graph.has_admitted_commit(&tip));
    assert!(graph.resolve(&batch_working).is_err());
    sign(&mut graph, &key, first);
    let published_signed = graph.signed_view_point();
    assert_changes(
        &graph.diff(&batch_working, &graph.view_point()).unwrap(),
        &[],
        &[],
    );
    assert_changes(
        &graph.diff(&published_signed, &graph.view_point()).unwrap(),
        &[],
        &[],
    );
    assert_eq!(
        batch_working.delivered_heads(),
        graph.view_point().delivered_heads()
    );
    assert!(graph.resolve(&first_working).is_ok());
    assert!(graph.resolve(&batch_working).is_ok());
    let old_signed = graph.resolve(&batch_signed).unwrap();
    assert!(!old_signed.visible(&first).unwrap());
    assert!(!old_signed.visible(&tip).unwrap());
    assert!(old_signed.visible(&base).unwrap());
    peer.merge(&graph);
    assert!(peer.resolve(&batch_working).is_ok());

    assert_eq!(
        graph.ingest_signature(author(&key), tip, signature(&key, tip)),
        Err(stockfoil::SignatureError::NotPendingLocalCommit(tip))
    );
    peer.merge(&graph);
    assert_changes(
        &graph
            .diff(&published_signed, &graph.signed_view_point())
            .unwrap(),
        &[],
        &[],
    );
}
