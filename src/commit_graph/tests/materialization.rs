//! Delivery and participation updates through one materialization callback.
use super::support;

use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    panic::{AssertUnwindSafe, catch_unwind},
};

use super::CommitGraph;
use crate::{
    BlobHash, CommitContents, CommitHash, CommitMeta, DiffError,
    MaterializationCursor, ViewPoint, Vis, WritePermission,
};
use support::{author, key, sign};

fn meta(seed: u8, value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        author: author(&key(seed)),
        parents: parents.to_vec(),
        payload: BlobHash::of_contents(&[value]),
    })
}

fn append(graph: &mut CommitGraph<u8>, seed: u8, value: u8) -> CommitHash {
    let meta = meta(seed, value, &graph.heads().collect::<Vec<_>>());
    let hash = *meta.hash();
    graph.insert_local_commit(meta, value);
    hash
}

fn grant(graph: &mut CommitGraph<u8>, seeds: &[u8]) {
    graph.set_write_permissions(
        &seeds
            .iter()
            .map(|&seed| (author(&key(seed)), WritePermission::Unbounded))
            .collect(),
    );
}

fn visit(
    graph: &CommitGraph<u8>,
    cursor: &MaterializationCursor,
    after: &ViewPoint,
) -> (MaterializationCursor, Vec<(u8, bool, Vis)>) {
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    let next = graph
        .visit_changes(cursor, after, |change| {
            // Check topological ordering even across commits omitted from this
            // traversal, and reject duplicates when delivery and visibility overlap.
            assert!(
                seen.iter()
                    .all(|hash| !graph.is_ancestor(change.meta.hash(), hash))
            );
            assert!(seen.insert(*change.meta.hash()));
            events.push((*change.payload, change.newly_delivered, change.visibility));
        })
        .unwrap();
    assert_eq!(next.view_point(), after);
    assert_eq!(next.delivery_heads(), graph.heads().collect::<Vec<_>>());
    (next, events)
}

#[test]
fn initial_delivery_includes_denied_commits_and_noop_calls_advance_the_viewpoint() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    let initial = MaterializationCursor::new();
    assert_eq!(initial, MaterializationCursor::default());
    assert!(initial.delivery_heads().is_empty());
    assert!(initial.view_point().heads().is_empty());
    let (cursor, events) = visit(&graph, &initial, &graph.view_point());
    assert!(events.is_empty());
    assert_ne!(cursor, initial); // Even an empty graph captures new permissions.

    append(&mut graph, 1, 1);
    append(&mut graph, 2, 2);
    append(&mut graph, 1, 3);
    let (cursor, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(
        events,
        vec![
            (1, true, Vis::After),
            (2, true, Vis::Neither),
            (3, true, Vis::After)
        ]
    );
    let (same, events) = visit(&graph, &cursor, &graph.view_point());
    assert!(events.is_empty());
    assert_eq!(same, cursor);

    // This commit lies beyond both participation clocks but still needs storing.
    append(&mut graph, 2, 4);
    let (cursor, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(events, vec![(4, true, Vis::Neither)]);
    assert!(visit(&graph, &cursor, &graph.view_point()).1.is_empty());
}

#[test]
fn permission_only_changes_do_not_redeliver_and_preserve_topological_order() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1, 2]);
    append(&mut graph, 1, 1);
    append(&mut graph, 2, 2);
    append(&mut graph, 1, 3);
    let allowed = graph.view_point();
    let (cursor, _) = visit(&graph, &MaterializationCursor::new(), &allowed);
    let saved = cursor.clone();
    grant(&mut graph, &[2]);
    let denied = graph.view_point();
    assert_eq!(allowed.heads(), denied.heads());
    let (revoked, events) = visit(&graph, &cursor, &denied);
    assert_eq!(
        events,
        vec![(1, false, Vis::Before), (3, false, Vis::Before)]
    );
    assert_eq!(cursor, saved);
    assert!(visit(&graph, &revoked, &denied).1.is_empty());
    let (_, restored) = visit(&graph, &revoked, &allowed);
    assert_eq!(
        restored,
        vec![(1, false, Vis::After), (3, false, Vis::After)]
    );
    assert_eq!(visit(&graph, &saved, &denied).1, events); // Old cursors replay.
}

#[test]
fn historical_views_do_not_rewind_delivery_or_redeliver_stored_commits() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    append(&mut graph, 1, 1);
    let old = graph.view_point();
    append(&mut graph, 1, 2);
    let current = graph.view_point();
    let (cursor, events) = visit(&graph, &MaterializationCursor::new(), &old);
    assert_eq!(events, vec![(1, true, Vis::After), (2, true, Vis::Neither)]);
    assert_ne!(cursor.delivery_heads(), cursor.view_point().heads());
    let (cursor, events) = visit(&graph, &cursor, &current);
    assert_eq!(events, vec![(2, false, Vis::After)]);
    let (cursor, events) = visit(&graph, &cursor, &old);
    assert_eq!(events, vec![(2, false, Vis::Before)]);
    append(&mut graph, 1, 3);
    let (cursor, events) = visit(&graph, &cursor, &old);
    assert_eq!(events, vec![(3, true, Vis::Neither)]);
    let (_, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(events, vec![(2, false, Vis::After), (3, false, Vis::After)]);
}

#[test]
fn delivery_and_permission_changes_share_one_deduplicated_stream() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    let root = append(&mut graph, 1, 1);
    let frontier = meta(2, 2, &[root]);
    graph.set_write_permissions(&HashMap::from([(
        author(&key(1)),
        WritePermission::Bounded(vec![*frontier.hash()]),
    )]));
    let (cursor, events) = visit(&graph, &MaterializationCursor::new(), &graph.view_point());
    assert_eq!(events, vec![(1, true, Vis::Neither)]);
    graph.insert_local_commit(frontier, 2);
    let (cursor, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(
        events,
        vec![(1, false, Vis::After), (2, true, Vis::Neither)]
    );

    // An old removal and newly delivered additions can occur in the same batch.
    append(&mut graph, 2, 3);
    grant(&mut graph, &[2]);
    let (_, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(
        events,
        vec![
            (1, false, Vis::Before),
            (2, false, Vis::After),
            (3, true, Vis::After)
        ]
    );
}

#[test]
fn signed_and_working_views_track_delivery_independently_of_admission() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    let root = append(&mut graph, 1, 1);
    sign(&mut graph, &key(1), root);
    let (cursor, _) = visit(
        &graph,
        &MaterializationCursor::new(),
        &graph.signed_view_point(),
    );
    let draft = append(&mut graph, 1, 2);
    assert!(!graph.has_commit(&draft));
    let (signed, events) = visit(&graph, &cursor, &graph.signed_view_point());
    assert_eq!(events, vec![(2, true, Vis::Neither)]);
    let (working, events) = visit(&graph, &cursor, &graph.view_point());
    assert_eq!(events, vec![(2, true, Vis::After)]);

    sign(&mut graph, &key(1), draft);
    let (_, events) = visit(&graph, &signed, &graph.signed_view_point());
    assert_eq!(events, vec![(2, false, Vis::After)]);
    assert!(visit(&graph, &working, &graph.view_point()).1.is_empty());
}

#[test]
fn missing_target_heads_fail_before_callbacks_and_leave_the_cursor_reusable() {
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    append(&mut graph, 1, 1);
    let before = graph.view_point();
    let (cursor, _) = visit(&graph, &MaterializationCursor::new(), &before);
    let missing = meta(1, 2, before.heads());
    for permissions in [HashMap::new(), before.write_permissions().clone()] {
        let after = ViewPoint::from_parts(vec![*missing.hash()], permissions);
        assert_eq!(
            graph.visit_changes(&cursor, &after, |_| panic!("validate before callbacks")),
            Err(DiffError::MissingCommit(*missing.hash()))
        );
    }
    assert!(visit(&graph, &cursor, &before).1.is_empty());
    graph.insert_local_commit(missing, 2);
    assert_eq!(
        visit(&graph, &cursor, &graph.view_point()).1,
        vec![(2, true, Vis::After)]
    );
}

#[test]
fn cursors_require_their_delivery_history_even_when_the_materialized_view_is_empty() {
    let mut source = CommitGraph::new(Default::default());
    let root = append(&mut source, 1, 1);
    let mut destination = source.clone();
    let later = meta(1, 2, &[root]);
    source.insert_local_commit(later.clone(), 2);
    let empty = ViewPoint::from_parts(vec![], HashMap::new());
    let (cursor, _) = visit(&source, &MaterializationCursor::new(), &empty);
    assert_eq!(
        destination.visit_changes(&cursor, &empty, |_| panic!("validate delivery heads too")),
        Err(DiffError::MissingCommit(*later.hash()))
    );
    destination.insert_local_commit(later, 2);
    assert!(visit(&destination, &cursor, &empty).1.is_empty());
}

#[test]
fn cursors_are_portable_across_different_graph_local_chain_assignments() {
    let a = meta(1, 1, &[]);
    let b = meta(2, 2, &[]);
    let join = meta(1, 3, &[*a.hash(), *b.hash()]);
    let mut left = CommitGraph::new(Default::default());
    grant(&mut left, &[1, 2]);
    left.insert_local_commit(a.clone(), 1);
    left.insert_local_commit(b.clone(), 2);
    let (cursor, _) = visit(&left, &MaterializationCursor::new(), &left.view_point());
    left.insert_local_commit(join.clone(), 3);
    let mut right = CommitGraph::new(Default::default());
    right.insert_local_commit(b, 2);
    right.insert_local_commit(a, 1);
    right.insert_local_commit(join, 3);
    let after = left.view_point();
    let expected = visit(&left, &cursor, &after);
    assert_eq!(expected.1, vec![(3, true, Vis::After)]);
    assert_eq!(visit(&right, &cursor, &after), expected);
    assert!(right.view_point().write_permissions().is_empty());
}

#[test]
fn visits_borrow_payloads_and_panics_do_not_advance_the_input_cursor() {
    #[derive(Debug)]
    struct Payload(u8);
    impl Clone for Payload {
        fn clone(&self) -> Self {
            panic!("traversal must not clone payloads")
        }
    }
    let mut graph = CommitGraph::new(Default::default());
    graph.insert_local_commit(meta(1, 1, &[]), Payload(1));
    let cursor = MaterializationCursor::new();
    let after = graph.view_point();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            graph.visit_changes(&cursor, &after, |_| panic!("materializer failed"))
        }))
        .is_err()
    );
    assert_eq!(cursor, MaterializationCursor::new());
    let mut changes = Vec::new();
    let next = graph
        .visit_changes(&cursor, &after, |change| changes.push(change))
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].payload.0, 1);
    assert!(changes[0].newly_delivered);
    assert_eq!(changes[0].visibility, Vis::Neither);
    graph.traverse(|meta, payload| {
        assert!(std::ptr::eq(meta, changes[0].meta));
        assert!(std::ptr::eq(payload, changes[0].payload));
        ControlFlow::Continue(())
    });
    graph
        .visit_changes(&next, &after, |_| panic!("already processed"))
        .unwrap();
}

#[test]
fn one_callback_can_restore_an_older_register_value_without_redelivery() {
    #[derive(Default)]
    struct Register {
        stored: HashMap<CommitHash, u8>,
        participating: HashSet<CommitHash>,
        cursor: MaterializationCursor,
    }
    impl Register {
        fn update(&mut self, graph: &CommitGraph<u8>, after: &ViewPoint) {
            self.cursor = graph
                .visit_changes(&self.cursor, after, |change| {
                    let hash = *change.meta.hash();
                    if change.newly_delivered {
                        assert!(self.stored.insert(hash, *change.payload).is_none());
                    }
                    match change.visibility {
                        Vis::After => {
                            self.participating.insert(hash);
                        }
                        Vis::Before => {
                            self.participating.remove(&hash);
                        }
                        Vis::Both | Vis::Neither => {}
                    }
                })
                .unwrap();
        }

        fn values(&self, graph: &CommitGraph<u8>) -> HashSet<u8> {
            self.participating
                .iter()
                .filter(|hash| {
                    !self
                        .participating
                        .iter()
                        .any(|other| graph.is_ancestor(hash, other))
                })
                .map(|hash| self.stored[hash])
                .collect()
        }
    }
    let mut graph = CommitGraph::new(Default::default());
    grant(&mut graph, &[1]);
    let root = append(&mut graph, 1, 1);
    append(&mut graph, 1, 2);
    let allowed = graph.view_point();
    let mut register = Register::default();
    register.update(&graph, &allowed);
    assert_eq!(register.values(&graph), HashSet::from([2]));
    graph.set_write_permissions(&HashMap::from([(
        author(&key(1)),
        WritePermission::Bounded(vec![root]),
    )]));
    register.update(&graph, &graph.view_point());
    assert_eq!(register.values(&graph), HashSet::from([1]));
    assert_eq!(register.stored.len(), 2);
    register.update(&graph, &allowed);
    assert_eq!(register.values(&graph), HashSet::from([2]));
}
