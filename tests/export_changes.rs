//! Contract tests for incremental publication of the certified export cover.
mod support;

use std::collections::BTreeSet;
use stockfoil::collections::HashMap;

use stockfoil::{
    CommitGraph, ExportChanges, ExportCursor, FragmentRange, ItemId, UnverifiedItem,
    WritePermission,
};
use support::{at_depth, author, key, proof, sign, signed_commit, signed_fragment};

fn receive(graph: &mut CommitGraph<()>, item: UnverifiedItem) {
    let report = graph.ingest(item.verify().unwrap(), |_| ()).unwrap();
    assert!(report.rejected.is_empty());
}

fn assert_changes(changes: &ExportChanges, added: &[ItemId], retired: &[ItemId]) {
    let ids: BTreeSet<_> = changes.added.iter().map(UnverifiedItem::id).collect();
    assert_eq!(ids, added.iter().cloned().collect());
    assert_eq!(changes.added.len(), ids.len(), "duplicate additions");
    let ids: BTreeSet<_> = changes.retired.iter().cloned().collect();
    assert_eq!(ids, retired.iter().cloned().collect());
    assert_eq!(changes.retired.len(), ids.len(), "duplicate retirements");
    for item in &changes.added {
        item.clone()
            .verify()
            .expect("added items carry sufficient certificates");
    }
}

#[test]
fn empty_cursor_exports_the_full_cover_and_advancing_makes_it_a_noop() {
    let mut graph = CommitGraph::new();
    let cursor = ExportCursor::default();
    let (empty, empty_cursor) = graph.export_changes(&cursor);
    assert_changes(&empty, &[], &[]);

    let root = at_depth(1, [], 0);
    let child = at_depth(1, [*root.hash()], 0);
    receive(&mut graph, signed_commit(&root, 1));
    receive(&mut graph, signed_commit(&child, 1));
    let expected = [ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())];
    let (changes, published) = graph.export_changes(&empty_cursor);
    assert_changes(&changes, &expected, &[]);

    // Not advancing (or cloning the old cursor) retries the publication.
    let (retry, _) = graph.export_changes(&empty_cursor.clone());
    assert_changes(&retry, &expected, &[]);
    let (noop, _) = graph.export_changes(&published);
    assert_changes(&noop, &[], &[]);

    // A different destination starts with its own empty inventory.
    let (fresh, _) = graph.export_changes(&ExportCursor::default());
    assert_changes(&fresh, &expected, &[]);

    let tip = at_depth(2, [*child.hash()], 0);
    receive(&mut graph, signed_commit(&tip, 2));
    let (increment, next) = graph.export_changes(&published);
    assert_changes(&increment, &[ItemId::Commit(*tip.hash())], &[]);
    let (noop, _) = graph.export_changes(&next);
    assert_changes(&noop, &[], &[]);
}

#[test]
fn drafts_and_received_waiters_are_not_exported_until_signing_admits_them() {
    let root = at_depth(1, [], 0);
    let child = at_depth(1, [*root.hash()], 0);
    let mut graph = CommitGraph::new();
    graph.ingest_local_commit(root.clone(), ());
    receive(&mut graph, signed_commit(&child, 1));
    assert_eq!(graph.pending_items(), 1);
    assert!(!graph.has_commit(root.hash()));
    assert!(!graph.has_commit(child.hash()));
    let (changes, cursor) = graph.export_changes(&ExportCursor::default());
    assert_changes(&changes, &[], &[]);

    sign(&mut graph, &key(1), *root.hash());
    assert_eq!(graph.pending_items(), 0);
    let (changes, next) = graph.export_changes(&cursor);
    assert_changes(
        &changes,
        &[ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        &[],
    );
    let (noop, _) = graph.export_changes(&next);
    assert_changes(&noop, &[], &[]);
}

#[test]
fn a_new_fragment_certificate_changes_publication_without_changing_history() {
    let root = at_depth(1, [], 0);
    let head = at_depth(1, [*root.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let mut graph = CommitGraph::new();
    // A signed checkpoint announcement is not yet a fragment certificate.
    receive(&mut graph, signed_commit(&root, 1));
    receive(&mut graph, signed_commit(&head, 1));
    let view = graph.view_point();
    let signed_view = graph.signed_view_point();
    let (initial, cursor) = graph.export_changes(&ExportCursor::default());
    let old = [ItemId::Commit(*root.hash()), ItemId::Commit(*head.hash())];
    assert_changes(&initial, &old, &[]);

    let item = signed_fragment(&range, vec![root, head], 1);
    graph
        .ingest(item.verify().unwrap(), |_| panic!("no new payloads"))
        .unwrap();
    assert_eq!(graph.view_point(), view);
    assert_eq!(graph.signed_view_point(), signed_view);
    let (changes, _) = graph.export_changes(&cursor);
    assert_changes(&changes, &[ItemId::Fragment(range)], &old);
}

#[test]
fn permissions_and_different_sufficient_proofs_do_not_republish_the_same_item() {
    let root = at_depth(1, [], 0);
    let head = at_depth(2, [*root.hash()], 1);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let id = ItemId::Fragment(range.clone());
    let mut graph = CommitGraph::new();
    receive(
        &mut graph,
        UnverifiedItem::Fragment {
            borrowed: BTreeSet::new(),
            range: range.clone(),
            commits: vec![root.clone(), head.clone()],
            signatures: vec![proof(1, *root.hash()), proof(2, *head.hash())],
        },
    );
    let (initial, cursor) = graph.export_changes(&ExportCursor::default());
    assert_changes(&initial, std::slice::from_ref(&id), &[]);

    // Alice now signs Bob's descendant rather than her own root. Both proofs
    // authenticate the same members, but the retained certificate really changes.
    receive(
        &mut graph,
        UnverifiedItem::Fragment {
            borrowed: BTreeSet::new(),
            range,
            commits: vec![root, head.clone()],
            signatures: vec![proof(1, *head.hash()), proof(2, *head.hash())],
        },
    );
    let before = initial.added[0].clone().verify().unwrap();
    let after = graph.export(&id).unwrap().verify().unwrap();
    assert_ne!(before.signature_boundary(), after.signature_boundary());
    let (changes, _) = graph.export_changes(&cursor);
    assert_changes(&changes, &[], &[]);

    graph.set_write_permissions(&HashMap::from([
        (author(&key(1)), WritePermission::Unbounded),
        (author(&key(2)), WritePermission::Unbounded),
    ]));
    let (changes, _) = graph.export_changes(&cursor);
    assert_changes(&changes, &[], &[]);
    graph.set_write_permissions(&HashMap::new());
    let (changes, _) = graph.export_changes(&cursor);
    assert_changes(&changes, &[], &[]);
}

#[test]
fn a_deeper_fragment_retires_a_previously_published_fragment() {
    let root = at_depth(1, [], 1);
    let head = at_depth(1, [*root.hash()], 2);
    let old = FragmentRange {
        head: *root.hash(),
        boundary: BTreeSet::new(),
    };
    let new = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let mut graph = CommitGraph::new();
    receive(&mut graph, signed_fragment(&old, vec![root.clone()], 1));
    let (initial, cursor) = graph.export_changes(&ExportCursor::default());
    assert_changes(&initial, &[ItemId::Fragment(old.clone())], &[]);

    receive(&mut graph, signed_fragment(&new, vec![root, head], 1));
    assert!(graph.export(&ItemId::Fragment(old.clone())).is_err());
    let (changes, next) = graph.export_changes(&cursor);
    assert_changes(&changes, &[ItemId::Fragment(new)], &[ItemId::Fragment(old)]);
    let (noop, _) = graph.export_changes(&next);
    assert_changes(&noop, &[], &[]);

    // The retired fragment's previously returned certificate remains usable.
    let mut receiver = CommitGraph::new();
    for item in initial.added.into_iter().chain(changes.added) {
        receive(&mut receiver, item);
    }
    assert_eq!(receiver.minimal_cover(), graph.minimal_cover());
}

#[test]
fn skipped_covers_are_coalesced_and_owned_batches_survive_certificate_pruning() {
    let root = at_depth(1, [], 0);
    let middle = at_depth(1, [*root.hash()], 0);
    let head = at_depth(1, [*middle.hash()], 1);
    let root_id = ItemId::Commit(*root.hash());
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let mut graph = CommitGraph::new();
    receive(&mut graph, signed_commit(&root, 1));
    let (saved, cursor) = graph.export_changes(&ExportCursor::default());
    assert_changes(&saved, std::slice::from_ref(&root_id), &[]);

    // No export_changes call while middle is briefly part of the loose cover.
    receive(&mut graph, signed_commit(&middle, 1));
    graph.ingest_local_commit(head.clone(), ());
    sign(&mut graph, &key(1), *head.hash());
    assert!(
        graph.export(&root_id).is_err(),
        "old standalone proof was pruned"
    );
    let (changes, next) = graph.export_changes(&cursor);
    assert_changes(
        &changes,
        &[ItemId::Fragment(range.clone())],
        std::slice::from_ref(&root_id),
    );
    let (retry, _) = graph.export_changes(&cursor);
    assert_changes(&retry, &[ItemId::Fragment(range)], &[root_id]);
    let (noop, _) = graph.export_changes(&next);
    assert_changes(&noop, &[], &[]);

    // The already-returned batch owns its evidence even though the cursor does
    // not pin certificates in the source. Retirement does not invalidate it.
    let mut receiver = CommitGraph::new();
    for item in saved.added.into_iter().chain(changes.added) {
        receive(&mut receiver, item);
    }
    assert!(receiver.has_commit(root.hash()));
    assert!(receiver.has_commit(middle.hash()));
    assert!(receiver.has_commit(head.hash()));
    assert_eq!(receiver.pending_items(), 0);
}
