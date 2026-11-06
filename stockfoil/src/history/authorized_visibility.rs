//! Internal traversal contract examples for the independent Verus authorization policy.
//! These are runtime regressions; raw changes APIs are now internal to Stockfoil.
use super::*;
use crate::WritePermission;
use ed25519_dalek::SigningKey;
use std::collections::{BTreeSet, HashMap};

fn author(seed: u8) -> Author {
    let key = SigningKey::from_bytes(&[seed; 32]);
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

fn commit(history: &mut History, author: Author, parents: Vec<CommitHash>, id: u64) -> CommitHash {
    let mut builder = history.begin_commit(author, parents).unwrap();
    builder.update(&id.to_le_bytes());
    let prepared = builder.finish();
    let hash = prepared.hash;
    history.publish_commit(prepared, id).unwrap();
    hash
}

// Alice root -> Bob -> Alice child, plus an unrelated Alice root.
fn fixture() -> (History, [CommitHash; 4], Author, Author) {
    let mut history = History::new();
    let (alice, bob) = (author(1), author(2));
    let root = commit(&mut history, alice, vec![], 0);
    let middle = commit(&mut history, bob, vec![root], 1);
    let child = commit(&mut history, alice, vec![middle], 2);
    let other = commit(&mut history, alice, vec![], 3);
    (history, [root, middle, child, other], alice, bob)
}

fn drain(
    history: &History,
    changes: &mut Changes,
    size: usize,
) -> (Vec<Change>, MaterializationCursor) {
    let mut records = vec![];
    loop {
        if let Batch::Complete(cursor) = history.next_changes(changes, &mut records, size).unwrap()
        {
            return (records, cursor);
        }
    }
}

#[test]
fn authorization_rules_are_observable_through_changes() {
    let (history, hashes, alice, bob) = fixture();
    let [root, middle, child, other] = hashes;
    let unknown = CommitHash::from([99; 32]);
    assert!(history.lookup(&unknown).is_none());
    let cases = [
        ("missing", vec![child, other], None, vec![]),
        (
            "denied",
            vec![child, other],
            Some(WritePermission::Denied),
            vec![],
        ),
        (
            "empty bound",
            vec![child, other],
            Some(WritePermission::Bounded(vec![])),
            vec![],
        ),
        (
            "unknown bound",
            vec![child, other],
            Some(WritePermission::Bounded(vec![unknown])),
            vec![],
        ),
        (
            "unbounded in selected history",
            vec![root],
            Some(WritePermission::Unbounded),
            vec![root],
        ),
        (
            "known unavailable bound",
            vec![root],
            Some(WritePermission::Bounded(vec![child])),
            vec![],
        ),
        (
            "cross-author inclusive bound",
            vec![child, other],
            Some(WritePermission::Bounded(vec![middle, other])),
            vec![root, other],
        ),
        (
            "cross denied intermediate",
            vec![child],
            Some(WritePermission::Bounded(vec![child])),
            vec![root, child],
        ),
        (
            "duplicate heads and bounds",
            vec![child, other, child],
            Some(WritePermission::Bounded(vec![
                middle, middle, unknown, other,
            ])),
            vec![root, other],
        ),
    ];
    for (name, heads, permission, expected) in cases {
        let mut permissions = HashMap::from([(bob, WritePermission::Denied)]);
        if let Some(permission) = permission {
            permissions.insert(alice, permission);
        }
        let view = ViewPoint::from_parts(heads, permissions);
        // Different budgets must give exactly the same authorization result.
        for size in [1, 2, usize::MAX] {
            let mut changes = history
                .changes(&MaterializationCursor::new(), &view)
                .unwrap();
            let (records, _) = drain(&history, &mut changes, size);
            // The canonical scheduler orders ready commits by hash, not local index.
            assert_eq!(records.len(), hashes.len(), "{name}");
            assert_eq!(
                records.iter().map(|r| r.hash).collect::<BTreeSet<_>>(),
                hashes.into_iter().collect::<BTreeSet<_>>(),
                "{name}"
            );
            for r in records {
                assert!(r.newly_delivered, "{name}");
                assert_eq!(
                    r.visibility,
                    if expected.contains(&r.hash) {
                        Vis::After
                    } else {
                        Vis::Neither
                    },
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn changing_one_author_does_not_change_another_authors_participation() {
    let (history, [root, middle, child, other], alice, bob) = fixture();
    let view = history.view_point(&HashMap::from([(alice, WritePermission::Unbounded)]));
    let mut initial = history
        .changes(&MaterializationCursor::new(), &view)
        .unwrap();
    let (_, cursor) = drain(&history, &mut initial, usize::MAX);
    let allowed = history.view_point(&HashMap::from([
        (alice, WritePermission::Unbounded),
        (bob, WritePermission::Bounded(vec![middle])),
    ]));
    let mut changes = history.changes(&cursor, &allowed).unwrap();
    let (records, cursor) = drain(&history, &mut changes, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hash, middle);
    assert_eq!(records[0].visibility, Vis::After);
    assert!(!records[0].newly_delivered);

    let revoked = history.view_point(&HashMap::from([(bob, WritePermission::Unbounded)]));
    let mut changes = history.changes(&cursor, &revoked).unwrap();
    let (records, _) = drain(&history, &mut changes, 2);
    assert_eq!(records.len(), 3);
    assert_eq!(
        records.iter().map(|r| r.hash).collect::<BTreeSet<_>>(),
        BTreeSet::from([root, child, other])
    );
    assert!(
        records
            .iter()
            .all(|r| r.visibility == Vis::Before && !r.newly_delivered)
    );
}

#[test]
fn a_frontier_delivered_after_capture_does_not_reauthorize_the_captured_view() {
    let mut history = History::new();
    let alice = author(1);
    let root = commit(&mut history, alice, vec![], 0);
    let mut builder = history.begin_commit(alice, vec![root]).unwrap();
    builder.update(b"future frontier");
    let prepared = builder.finish();
    let future = prepared.hash;
    let permissions = HashMap::from([(alice, WritePermission::Bounded(vec![future]))]);
    let view = ViewPoint::from_parts(vec![root], permissions.clone());
    let mut captured = history
        .changes(&MaterializationCursor::new(), &view)
        .unwrap();
    history.publish_commit(prepared, 1).unwrap();
    let (records, cursor) = drain(&history, &mut captured, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hash, root);
    assert_eq!(records[0].visibility, Vis::Neither);
    // Known now, but still outside the original selected history.
    let mut same_view = history.changes(&cursor, &view).unwrap();
    let (records, _) = drain(&history, &mut same_view, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hash, future);
    assert_eq!(records[0].visibility, Vis::Neither);
    // Expanding the selected history legitimately authorizes both commits.
    let expanded = ViewPoint::from_parts(vec![future], permissions);
    let mut changes = history.changes(&cursor, &expanded).unwrap();
    let (records, _) = drain(&history, &mut changes, 1);
    assert_eq!(
        records.iter().map(|r| r.hash).collect::<Vec<_>>(),
        [root, future]
    );
    assert!(records.iter().all(|r| r.visibility == Vis::After));
    assert!(!records[0].newly_delivered && records[1].newly_delivered);
}

#[test]
fn invalid_view_and_wrong_history_do_not_emit_records() {
    let (history, _, alice, _) = fixture();
    let unknown = CommitHash::from([99; 32]);
    let invalid = ViewPoint::from_parts(vec![unknown], HashMap::new());
    assert_eq!(
        history
            .changes(&MaterializationCursor::new(), &invalid)
            .unwrap_err(),
        Error::UnknownCommit(unknown)
    );
    let valid = history.view_point(&HashMap::from([(alice, WritePermission::Unbounded)]));
    let mut changes = history
        .changes(&MaterializationCursor::new(), &valid)
        .unwrap();
    let remaining = changes.remaining();
    let mut records = vec![];
    assert_eq!(
        History::new()
            .next_changes(&mut changes, &mut records, 1)
            .unwrap_err(),
        Error::WrongHistory
    );
    assert!(records.is_empty());
    assert_eq!(changes.remaining(), remaining);
    // A rejected call must not consume an authorized record from the original.
    let (records, _) = drain(&history, &mut changes, usize::MAX);
    assert_eq!(records.len(), remaining);
}
