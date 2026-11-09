//! Views, participation, and metadata-only change batches for materializers.
//!
//! A [`ViewPoint`] is a set of history heads plus write permissions. A commit
//! *participates* in a view if it is in the heads' ancestry and its author's
//! permission allows it. Change planning compares two participation sets and the
//! materializer's previously delivered history, and enumerates only the chain
//! intervals that differ, so small updates cost about as much as they change.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) use super::traversal::plan_changes_at;
use super::{Heads, History};
#[cfg(test)]
use crate::Author;
#[cfg(any(test, feature = "fuzzing"))]
use crate::CommitHash;
#[cfg(any(test, feature = "fuzzing"))]
use crate::Error;
#[cfg(test)]
use crate::WritePermission;
#[cfg(any(test, feature = "fuzzing"))]
pub use crate::verified::change_plan::Changes;
#[cfg(any(test, feature = "fuzzing"))]
pub use crate::verified::events::Vis;

pub use crate::verified::write_permissions::WritePermissions;

#[cfg(any(test, feature = "fuzzing"))]
pub use crate::verified::view::MaterializationCursor;
pub use crate::verified::view::ViewPoint;

#[cfg(test)]
pub(super) use crate::verified::metadata_encoding::encode_hashes;

#[cfg(any(test, feature = "fuzzing"))]
pub(super) use crate::verified::change_plan::emit_next_changes;
#[cfg(any(test, feature = "fuzzing"))]
pub use crate::verified::change_plan::{Batch, Change};

pub(super) fn view(history: &History, heads: Heads, permissions: &WritePermissions) -> ViewPoint {
    ViewPoint::from_parts(history.heads(heads).collect(), permissions.clone())
}

#[cfg(any(test, feature = "fuzzing"))]
pub(super) fn plan_changes(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> Result<Changes, Error> {
    let delivery_heads: Vec<_> = history.topology.heads().iter().copied().collect();
    plan_changes_at(history, cursor, target, &delivery_heads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn emitter_preserves_prefix_and_classifies_arbitrary_typed_cursors() {
        let mut history = History::new();
        let mut wrong = History::new();
        let author = Author([1; 32]);
        let hashes = [1, 2, 3, 4].map(|b| CommitHash([b; 32]));
        for (i, hash) in hashes.iter().enumerate() {
            for h in [&mut history, &mut wrong] {
                h.topology
                    .insert(*hash, author, &[], 10 + i as u64, 0)
                    .unwrap();
            }
        }
        let open = || WritePermissions::new().with_unlisted(WritePermission::Unbounded);
        // Arbitrary typed/persisted cursors may name prior visibility separately
        // from prior delivery. Exercise all four classifier cases at this boundary.
        let base = MaterializationCursor::from_parts(
            vec![],
            ViewPoint::from_parts(vec![hashes[0], hashes[2]], open()),
        );
        let target = ViewPoint::from_parts(vec![hashes[1], hashes[2]], open());
        let delivery = vec![hashes[3], hashes[1], hashes[2], hashes[0]];
        let mut changes = plan_changes_at(&history, &base, &target, &delivery).unwrap();
        let sentinel = Change {
            hash: CommitHash([255; 32]),
            payload: 999,
            newly_delivered: false,
            visibility: Vis::Both,
        };
        let mut out = vec![sentinel];
        assert!(matches!(
            emit_next_changes(&history, &mut changes, &mut out, 0).unwrap(),
            Batch::More
        ));
        assert_eq!(out, vec![sentinel]);
        assert_eq!(changes.remaining(), 4);
        for max in [0, usize::MAX] {
            assert!(matches!(
                emit_next_changes(&wrong, &mut changes, &mut out, max),
                Err(Error::WrongHistory)
            ));
            assert_eq!(out, vec![sentinel]);
            assert_eq!(changes.remaining(), 4);
        }
        assert!(matches!(
            emit_next_changes(&history, &mut changes, &mut out, 2).unwrap(),
            Batch::More
        ));
        assert_eq!(out.len(), 3);
        assert_eq!(changes.remaining(), 2);
        history
            .topology
            .insert(CommitHash([0; 32]), author, &[], 100, 0)
            .unwrap();
        let Batch::Complete(cursor) =
            emit_next_changes(&history, &mut changes, &mut out, usize::MAX).unwrap()
        else {
            panic!("complete captured batch");
        };
        let expected = [Vis::Before, Vis::After, Vis::Both, Vis::Neither];
        assert_eq!(out[0], sentinel);
        assert_eq!(out.len(), 5);
        for (i, change) in out[1..].iter().enumerate() {
            assert_eq!(
                *change,
                Change {
                    hash: hashes[i],
                    payload: 10 + i as u64,
                    newly_delivered: true,
                    visibility: expected[i]
                }
            );
        }
        assert_eq!(cursor.delivery_heads(), delivery);
        assert_eq!(cursor.view_point(), &target);
        let finished = out.clone();
        assert!(matches!(
            emit_next_changes(&history, &mut changes, &mut out, 1).unwrap(),
            Batch::Complete(_)
        ));
        assert_eq!(out, finished);
        assert!(matches!(
            emit_next_changes(&wrong, &mut changes, &mut out, 1),
            Err(Error::WrongHistory)
        ));
        assert_eq!(out, finished);
    }

    #[test]
    fn planning_checks_each_head_source_and_uses_the_captured_frontier() {
        let mut history = History::new();
        let alice = Author([1; 32]);
        let [root, later, unknown] = [7, 8, 99].map(|b| CommitHash([b; 32]));
        history.topology.insert(root, alice, &[], 10, 0).unwrap();
        history
            .topology
            .insert(later, alice, &[root], 11, 0)
            .unwrap();
        let open = || WritePermissions::new().with_unlisted(WritePermission::Unbounded);
        let at = |hash| ViewPoint::from_parts(vec![hash], open());
        let cursor = MaterializationCursor::from_parts(vec![root], at(root));
        let target = at(root);
        let unchanged = plan_changes_at(&history, &cursor, &target, &[root]).unwrap();
        assert_eq!(unchanged.remaining(), 0); // The later delivered commit is outside this capture.

        for (old_delivery, before_head, after_head, captured) in [
            (unknown, root, root, root),
            (root, unknown, root, root),
            (root, root, unknown, root),
            (root, root, root, unknown),
        ] {
            let base = MaterializationCursor::from_parts(vec![old_delivery], at(before_head));
            assert!(
                matches!(plan_changes_at(&history, &base, &at(after_head), &[captured]),
                Err(Error::UnknownCommit(h)) if h == unknown)
            );
        }
        for (old_delivery, before_head, after_head) in [
            (later, root, root),
            (root, later, root),
            (root, root, later),
        ] {
            let base = MaterializationCursor::from_parts(vec![old_delivery], at(before_head));
            assert!(matches!(
                plan_changes_at(&history, &base, &at(after_head), &[root]),
                Err(Error::InvalidItem(_))
            ));
        }

        // Permission bounds are not required heads. Missing bounds and known
        // bounds outside the target ancestry both grant nothing.
        for frontier in [unknown, later] {
            let denied = ViewPoint::from_parts(
                vec![root],
                WritePermissions::from([(alice, WritePermission::Bounded(vec![frontier]))]),
            );
            let mut changes = plan_changes_at(&history, &cursor, &denied, &[root]).unwrap();
            let mut records = Vec::new();
            let Batch::Complete(completed) = history
                .next_changes(&mut changes, &mut records, 10)
                .unwrap()
            else {
                panic!("complete retraction");
            };
            assert_eq!(
                records,
                vec![Change {
                    hash: root,
                    payload: 10,
                    newly_delivered: false,
                    visibility: Vis::Before
                }]
            );
            assert_eq!(completed.delivery_heads(), &[root]);
            assert_eq!(completed.view_point(), &denied);
        }
    }

    #[test]
    fn paused_changes_keep_captured_order_and_metadata_across_history_growth() {
        let mut history = History::new();
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let carol = Author([3; 32]);
        let [root, tip, child, independent, later, new_root, merge] =
            [8, 9, 1, 2, 0, 3, 4].map(|b| CommitHash([b; 32]));
        for (hash, author, parents, payload) in [
            (root, alice, vec![], 10),
            (tip, alice, vec![root], 11),
            (child, bob, vec![tip], 12),
            (independent, carol, vec![], 13),
        ] {
            history
                .topology
                .insert(hash, author, &parents, payload, 0)
                .unwrap();
        }
        let target = ViewPoint::from_parts(
            vec![child, independent],
            WritePermissions::from([(alice, WritePermission::Bounded(vec![root]))]),
        );
        let empty = MaterializationCursor::new();
        let mut baseline = history.changes(&empty, &target).unwrap();
        let mut paused = history.changes(&empty, &target).unwrap();
        let mut expected = Vec::new();
        let Batch::Complete(expected_cursor) = history
            .next_changes(&mut baseline, &mut expected, 20)
            .unwrap()
        else {
            panic!("complete baseline");
        };
        assert_eq!(
            expected.iter().map(|c| c.hash).collect::<Vec<_>>(),
            vec![independent, root, tip, child]
        );
        let mut actual = Vec::new();
        assert!(matches!(
            history.next_changes(&mut paused, &mut actual, 1).unwrap(),
            Batch::More
        ));
        assert_eq!(paused.remaining(), 3);
        let wrong = History::new();
        assert!(matches!(
            wrong.next_changes(&mut paused, &mut actual, 0),
            Err(Error::WrongHistory)
        ));
        assert_eq!(paused.remaining(), 3);
        assert_eq!(actual, expected[..1]);

        // Extend a selected chain, introduce a root, then merge across chains.
        history
            .topology
            .insert(later, alice, &[tip], 14, 0)
            .unwrap();
        history
            .topology
            .insert(new_root, carol, &[], 15, 0)
            .unwrap();
        assert!(matches!(
            history.next_changes(&mut paused, &mut actual, 1).unwrap(),
            Batch::More
        ));
        history
            .topology
            .insert(merge, bob, &[child, later, new_root], 16, 0)
            .unwrap();
        let Batch::Complete(cursor) = history.next_changes(&mut paused, &mut actual, 20).unwrap()
        else {
            panic!("complete captured traversal");
        };
        assert_eq!(actual, expected);
        assert_eq!(cursor, expected_cursor);
        assert_eq!(paused.remaining(), 0);
        assert!(matches!(
            history.next_changes(&mut paused, &mut actual, 1).unwrap(),
            Batch::Complete(_)
        ));
        assert_eq!(actual, expected);
        assert!(matches!(
            wrong.next_changes(&mut paused, &mut actual, 1),
            Err(Error::WrongHistory)
        ));
        assert_eq!(actual, expected);
    }

    #[test]
    fn traversal_retains_intervals_not_one_entry_per_record() {
        let author = Author::from(&SigningKey::from_bytes(&[1; 32]));
        let mut history = History::new();
        let mut hashes = Vec::new();
        for i in 0..10_000u64 {
            let mut builder = history
                .begin_commit(author, hashes.last().copied().into_iter().collect())
                .unwrap();
            builder.update(&i.to_le_bytes());
            let prepared = builder.finish();
            hashes.push(prepared.hash);
            history.publish_commit(prepared, i).unwrap();
        }
        let view = history.view_point(&WritePermissions::from([(
            author,
            WritePermission::Bounded(vec![hashes[99]]),
        )]));
        let mut cold = history
            .changes(&MaterializationCursor::new(), &view)
            .unwrap();
        assert_eq!(cold.remaining(), 10_000);
        assert_eq!(
            cold.plan.traversal.as_ref().unwrap().retained_counts(),
            (1, 1, 1, 0)
        );
        let mut batch = Vec::new();
        let cursor = loop {
            batch.clear();
            if let Batch::Complete(cursor) =
                history.next_changes(&mut cold, &mut batch, 128).unwrap()
            {
                break cursor;
            }
        };
        assert_eq!(cold.remaining(), 0);
        assert_eq!(
            cold.plan.traversal.as_ref().unwrap().retained_counts(),
            (0, 0, 0, 0)
        );

        for i in 10_000..10_030u64 {
            let mut builder = history
                .begin_commit(author, vec![*hashes.last().unwrap()])
                .unwrap();
            builder.update(&i.to_le_bytes());
            let prepared = builder.finish();
            hashes.push(prepared.hash);
            history.publish_commit(prepared, i).unwrap();
        }
        // Two disjoint intervals on the same chain: retract 90 earlier writes
        // and deliver 30 new inactive writes, skipping 9,900 unchanged commits.
        let view = history.view_point(&WritePermissions::from([(
            author,
            WritePermission::Bounded(vec![hashes[9]]),
        )]));
        let mut changes = history.changes(&cursor, &view).unwrap();
        assert_eq!(changes.remaining(), 120);
        assert_eq!(
            changes.plan.traversal.as_ref().unwrap().retained_counts(),
            (1, 2, 1, 0)
        );
        batch.clear();
        assert!(matches!(
            history.next_changes(&mut changes, &mut batch, 200).unwrap(),
            Batch::Complete(_)
        ));
        let expected: Vec<_> = hashes[10..100]
            .iter()
            .chain(&hashes[10_000..])
            .copied()
            .collect();
        assert_eq!(batch.iter().map(|c| c.hash).collect::<Vec<_>>(), expected);
    }
}
