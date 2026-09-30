//! Contract tests for incremental publication of the certified export cover.
use super::support;

use std::collections::{BTreeSet, HashMap};

use super::CommitGraph;
use crate::export::ExportChanges;
use crate::{
    Checkpoint, ExportCursor, FragmentRange, ItemId, UnverifiedItem,
    WritePermission,
};
use support::{at_depth, author, key, proof, sign, signed_commit, signed_fragment};

fn receive(graph: &mut CommitGraph<()>, item: UnverifiedItem) {
    let report = graph.ingest_verified(item.verify().unwrap(), |_| ()).unwrap();
    assert!(report.rejected.is_empty());
}

fn checkpoints(item: &UnverifiedItem) -> &BTreeSet<Checkpoint> {
    let UnverifiedItem::Fragment { checkpoints, .. } = item else {
        panic!("expected fragment")
    };
    checkpoints
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
    let mut graph = CommitGraph::new(Default::default());
    let cursor = ExportCursor::default();
    let (empty, empty_cursor) = graph.export_items_since(&cursor);
    assert_changes(&empty, &[], &[]);

    let root = at_depth(1, [], 0);
    let child = at_depth(1, [*root.hash()], 0);
    receive(&mut graph, signed_commit(&root, 1));
    receive(&mut graph, signed_commit(&child, 1));
    let expected = [ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())];
    let (changes, published) = graph.export_items_since(&empty_cursor);
    assert_changes(&changes, &expected, &[]);

    // Not advancing (or cloning the old cursor) retries the publication.
    let (retry, _) = graph.export_items_since(&empty_cursor.clone());
    assert_changes(&retry, &expected, &[]);
    let (noop, _) = graph.export_items_since(&published);
    assert_changes(&noop, &[], &[]);

    // A different destination starts with its own empty inventory.
    let (fresh, _) = graph.export_items_since(&ExportCursor::default());
    assert_changes(&fresh, &expected, &[]);

    let tip = at_depth(2, [*child.hash()], 0);
    receive(&mut graph, signed_commit(&tip, 2));
    let (increment, next) = graph.export_items_since(&published);
    assert_changes(&increment, &[ItemId::Commit(*tip.hash())], &[]);
    let (noop, _) = graph.export_items_since(&next);
    assert_changes(&noop, &[], &[]);
}

#[test]
fn drafts_and_received_waiters_are_not_exported_until_signing_admits_them() {
    let root = at_depth(1, [], 0);
    let child = at_depth(1, [*root.hash()], 0);
    let mut graph = CommitGraph::new(Default::default());
    graph.insert_local_commit(root.clone(), ());
    receive(&mut graph, signed_commit(&child, 1));
    assert_eq!(graph.pending_items(), 1);
    assert!(!graph.has_commit(root.hash()));
    assert!(!graph.has_commit(child.hash()));
    let (changes, cursor) = graph.export_items_since(&ExportCursor::default());
    assert_changes(&changes, &[], &[]);

    sign(&mut graph, &key(1), *root.hash());
    assert_eq!(graph.pending_items(), 0);
    let (changes, next) = graph.export_items_since(&cursor);
    assert_changes(
        &changes,
        &[ItemId::Commit(*root.hash()), ItemId::Commit(*child.hash())],
        &[],
    );
    let (noop, _) = graph.export_items_since(&next);
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
    let mut graph = CommitGraph::new(Default::default());
    // A signed checkpoint announcement is not yet a fragment certificate.
    receive(&mut graph, signed_commit(&root, 1));
    receive(&mut graph, signed_commit(&head, 1));
    let view = graph.view_point();
    let signed_view = graph.signed_view_point();
    let (initial, cursor) = graph.export_items_since(&ExportCursor::default());
    let old = [ItemId::Commit(*root.hash()), ItemId::Commit(*head.hash())];
    assert_changes(&initial, &old, &[]);

    let item = signed_fragment(&range, vec![root, head], 1);
    graph
        .ingest_verified(item.verify().unwrap(), |_| panic!("no new payloads"))
        .unwrap();
    assert_eq!(graph.view_point(), view);
    assert_eq!(graph.signed_view_point(), signed_view);
    let (changes, _) = graph.export_items_since(&cursor);
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
    let mut graph = CommitGraph::new(Default::default());
    receive(
        &mut graph,
        UnverifiedItem::Fragment {
            range: range.clone(),
            checkpoints: BTreeSet::new(),
            commits: vec![root.clone(), head.clone()],
            signatures: vec![proof(1, *root.hash()), proof(2, *head.hash())],
        },
    );
    let (initial, cursor) = graph.export_items_since(&ExportCursor::default());
    assert_changes(&initial, std::slice::from_ref(&id), &[]);

    // Alice now signs Bob's descendant rather than her own root. Both proofs
    // authenticate the same members, but the retained certificate really changes.
    receive(
        &mut graph,
        UnverifiedItem::Fragment {
            range,
            checkpoints: BTreeSet::new(),
            commits: vec![root, head.clone()],
            signatures: vec![proof(1, *head.hash()), proof(2, *head.hash())],
        },
    );
    let before = initial.added[0].clone().verify().unwrap();
    let after = graph.export_item(&id).unwrap().verify().unwrap();
    assert_ne!(before.signature_boundary(), after.signature_boundary());
    let (changes, _) = graph.export_items_since(&cursor);
    assert_changes(&changes, &[], &[]);

    graph.set_write_permissions(&HashMap::from([
        (author(&key(1)), WritePermission::Unbounded),
        (author(&key(2)), WritePermission::Unbounded),
    ]));
    let (changes, _) = graph.export_items_since(&cursor);
    assert_changes(&changes, &[], &[]);
    graph.set_write_permissions(&HashMap::new());
    let (changes, _) = graph.export_items_since(&cursor);
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
    let mut graph = CommitGraph::new(Default::default());
    receive(&mut graph, signed_fragment(&old, vec![root.clone()], 1));
    let (initial, cursor) = graph.export_items_since(&ExportCursor::default());
    assert_changes(&initial, &[ItemId::Fragment(old.clone())], &[]);
    let captured = graph.export_item(&ItemId::Fragment(old.clone())).unwrap();
    assert!(checkpoints(&captured).is_empty());
    assert_eq!(checkpoints(&initial.added[0]), checkpoints(&captured));
    let expected = BTreeSet::from([Checkpoint(root.hash().as_bytes()[..12].try_into().unwrap())]);

    receive(&mut graph, signed_fragment(&new, vec![root, head], 1));
    assert!(graph.export_item(&ItemId::Fragment(old.clone())).is_err());
    let (changes, next) = graph.export_items_since(&cursor);
    assert_changes(&changes, &[ItemId::Fragment(new.clone())], &[ItemId::Fragment(old)]);
    let direct = graph.export_item(&ItemId::Fragment(new)).unwrap();
    assert_eq!(checkpoints(&changes.added[0]), &expected);
    assert_eq!(checkpoints(&direct), &expected);
    assert_eq!(graph.minimal_cover().fragments[0].checkpoints, expected);
    let (noop, _) = graph.export_items_since(&next);
    assert_changes(&noop, &[], &[]);

    // Permissions affect materialization, not checkpoint metadata or identity.
    graph.set_write_permissions(&HashMap::from([(author(&key(1)), WritePermission::Unbounded)]));
    let (noop, _) = graph.export_items_since(&next);
    assert_changes(&noop, &[], &[]);
    graph.set_write_permissions(&HashMap::new());
    let (noop, _) = graph.export_items_since(&next);
    assert_changes(&noop, &[], &[]);
    assert_eq!(checkpoints(&graph.export_item(&direct.id()).unwrap()), &expected);
    assert_eq!(checkpoints(&changes.added[0]), &expected);

    // Direct and incremental captures own their metadata and certificates,
    // even after the source prunes and retires their range.
    assert!(checkpoints(&captured).is_empty());
    assert!(checkpoints(&initial.added[0]).is_empty());
    let mut direct_receiver = CommitGraph::new(Default::default());
    receive(&mut direct_receiver, captured);
    assert_eq!(direct_receiver.minimal_cover().fragments.len(), 1);

    // The retired fragment's previously returned certificate remains usable.
    let mut receiver = CommitGraph::new(Default::default());
    for item in initial.added.into_iter().chain(changes.added) {
        receive(&mut receiver, item);
    }
    assert_eq!(receiver.minimal_cover(), graph.minimal_cover());
}

#[test]
fn checkpoint_hints_do_not_override_authenticated_members_or_canonical_reexports() {
    let root = at_depth(1, [], 1);
    let head = at_depth(1, [*root.hash()], 2);
    let range = FragmentRange {
        head: *head.hash(),
        boundary: BTreeSet::new(),
    };
    let id = ItemId::Fragment(range.clone());
    let mut source = CommitGraph::new(Default::default());
    receive(&mut source, signed_fragment(&range, vec![root, head], 1));
    let exported = source.export_item(&id).unwrap();
    let expected = exported.clone().verify().unwrap();
    assert!(!checkpoints(&exported).is_empty());

    for hints in [BTreeSet::new(), BTreeSet::from([Checkpoint([255; 12])])] {
        let mut incoming = exported.clone();
        let UnverifiedItem::Fragment { checkpoints: received_hints, .. } = &mut incoming else {
            panic!("expected fragment")
        };
        *received_hints = hints;
        let verified = incoming.verify().unwrap();
        assert_eq!(verified.id(), expected.id());
        assert_eq!(
            verified.commits().map(|meta| *meta.hash()).collect::<Vec<_>>(),
            expected.commits().map(|meta| *meta.hash()).collect::<Vec<_>>()
        );
        assert_eq!(verified.signature_boundary(), expected.signature_boundary());
        let mut receiver = CommitGraph::new(Default::default());
        let report = receiver.ingest_verified(verified, |_| ()).unwrap();
        assert!(report.rejected.is_empty());
        assert_eq!(receiver.minimal_cover(), source.minimal_cover());
        assert_eq!(
            checkpoints(&receiver.export_item(&id).unwrap()),
            checkpoints(&exported)
        );
        let (changes, _) = receiver.export_items_since(&ExportCursor::default());
        assert_changes(&changes, std::slice::from_ref(&id), &[]);
        assert_eq!(checkpoints(&changes.added[0]), checkpoints(&exported));
    }
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
    let mut graph = CommitGraph::new(Default::default());
    receive(&mut graph, signed_commit(&root, 1));
    let (saved, cursor) = graph.export_items_since(&ExportCursor::default());
    assert_changes(&saved, std::slice::from_ref(&root_id), &[]);

    // No export_changes call while middle is briefly part of the loose cover.
    receive(&mut graph, signed_commit(&middle, 1));
    graph.insert_local_commit(head.clone(), ());
    sign(&mut graph, &key(1), *head.hash());
    assert!(
        graph.export_item(&root_id).is_err(),
        "old standalone proof was pruned"
    );
    let (changes, next) = graph.export_items_since(&cursor);
    assert_changes(
        &changes,
        &[ItemId::Fragment(range.clone())],
        std::slice::from_ref(&root_id),
    );
    let (retry, _) = graph.export_items_since(&cursor);
    assert_changes(&retry, &[ItemId::Fragment(range)], &[root_id]);
    let (noop, _) = graph.export_items_since(&next);
    assert_changes(&noop, &[], &[]);

    // The already-returned batch owns its evidence even though the cursor does
    // not pin certificates in the source. Retirement does not invalidate it.
    let mut receiver = CommitGraph::new(Default::default());
    for item in saved.added.into_iter().chain(changes.added) {
        receive(&mut receiver, item);
    }
    assert!(receiver.has_commit(root.hash()));
    assert!(receiver.has_commit(middle.hash()));
    assert!(receiver.has_commit(head.hash()));
    assert_eq!(receiver.pending_items(), 0);
}
