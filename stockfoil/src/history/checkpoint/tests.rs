use super::*;
use crate::history::view::{self, Batch, Change};
use crate::{Author, Heads, History, Vis, WritePermission, WritePermissions};
use ed25519_dalek::SigningKey;

fn author(seed: u8) -> Author {
    Author::from(&SigningKey::from_bytes(&[seed; 32]))
}

fn commit(history: &mut History, author: Author, parents: &[CommitHash], tag: u64) -> CommitHash {
    let mut builder = history.begin_commit(author, parents.to_vec()).unwrap();
    builder.update(&tag.to_le_bytes());
    let prepared = builder.finish();
    let hash = prepared.hash;
    history.publish_commit(prepared, tag).unwrap();
    hash
}

fn drain(history: &History, changes: &mut crate::Changes) -> (Vec<Change>, MaterializationCursor) {
    let mut records = Vec::new();
    loop {
        if let Batch::Complete(cursor) = history.next_changes(changes, &mut records, 3).unwrap() {
            return (records, cursor);
        }
    }
}

fn fixture() -> Checkpoint {
    let mut history = History::new();
    let root = commit(&mut history, author(1), &[], 0);
    let leaf = commit(&mut history, author(2), &[root], 1);
    let permissions = WritePermissions::from([
        (author(1), WritePermission::Unbounded),
        (author(2), WritePermission::Bounded(vec![leaf, root])),
        (author(3), WritePermission::Denied),
    ])
    .with_unlisted(WritePermission::Bounded(vec![root]));
    let target = history.view_point(&permissions);
    let mut changes = history
        .changes(&MaterializationCursor::new(), &target)
        .unwrap();
    let (_, cursor) = drain(&history, &mut changes);
    Checkpoint {
        follow: Follow::Pinned(vec![leaf]),
        cursor,
        rejected: BTreeMap::from([(root, false), (leaf, true)]),
        in_progress: Some(InProgress {
            target,
            delivery_heads: vec![leaf],
            applied: 1,
        }),
    }
}

#[test]
fn complete_and_partial_checkpoints_round_trip_for_every_follow_rule() {
    let base = fixture();
    for follow in [
        Follow::Working,
        Follow::Signed,
        Follow::Pinned(vec![]),
        base.follow.clone(),
    ] {
        for partial in [false, true] {
            for rejections in [false, true] {
                let mut checkpoint = base.clone();
                checkpoint.follow = follow.clone();
                if !partial {
                    checkpoint.in_progress = None;
                }
                if !rejections {
                    checkpoint.rejected.clear();
                }
                let bytes = checkpoint.to_bytes();
                let decoded = Checkpoint::from_bytes(&bytes).unwrap();
                assert_eq!(decoded, checkpoint);
                assert_eq!(decoded.to_bytes(), bytes);
                assert_eq!(decoded.is_partial(), partial);
                assert_eq!(decoded.heads(), checkpoint.cursor.view_point().heads());
                assert_eq!(decoded.follow(), &follow);
                assert_eq!(
                    decoded.rejected().collect::<Vec<_>>(),
                    checkpoint.rejected().collect::<Vec<_>>()
                );
            }
        }
    }
    assert_eq!(Checkpoint::default(), Checkpoint::new(Follow::Working));
    let empty = Checkpoint::new(Follow::Signed);
    assert!(empty.heads().is_empty());
    assert!(!empty.is_partial());
    assert_eq!(empty.rejected().len(), 0);
    assert_eq!(Checkpoint::from_bytes(&empty.to_bytes()).unwrap(), empty);
}

#[test]
fn encoding_is_independent_of_permission_map_and_rejection_insertion_order() {
    let left = fixture();
    let mut right = left.clone();
    let progress = right.in_progress.as_mut().unwrap();
    let mut permissions: Vec<_> = progress.target.write_permissions().iter().collect();
    permissions.sort_by_key(|(a, _)| **a);
    let reordered = permissions
        .into_iter()
        .rev()
        .map(|(a, p)| (*a, p.clone()))
        .collect::<WritePermissions>()
        .with_unlisted(progress.target.write_permissions().unlisted().clone());
    progress.target = ViewPoint::from_parts(progress.target.heads().to_vec(), reordered);
    right.rejected = right.rejected.iter().rev().map(|(&h, &b)| (h, b)).collect();
    assert_eq!(left, right);
    assert_eq!(left.to_bytes(), right.to_bytes());
}

#[test]
fn rejections_cost_only_a_hash_and_a_flag_each() {
    let mut checkpoint = Checkpoint::default();
    let empty = checkpoint.to_bytes().len();
    for i in 1..=10u8 {
        checkpoint
            .rejected
            .insert(CommitHash::from([i; 32]), i % 2 == 0);
        assert_eq!(checkpoint.to_bytes().len(), empty + usize::from(i) * 33);
    }
}

#[test]
fn every_truncation_and_trailing_byte_is_rejected() {
    let bytes = fixture().to_bytes();
    for end in 0..bytes.len() {
        assert!(
            Checkpoint::from_bytes(&bytes[..end]).is_err(),
            "prefix {end}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(Checkpoint::from_bytes(&trailing).is_err());
}

#[test]
fn unknown_versions_tags_oversized_lengths_and_duplicate_rejections_are_rejected() {
    let empty = Checkpoint::default().to_bytes();
    for (offset, byte) in [(0, b'X'), (4, 99), (5, 3), (empty.len() - 1, 2)] {
        let mut bytes = empty.clone();
        bytes[offset] = byte;
        assert!(Checkpoint::from_bytes(&bytes).is_err());
    }
    let cursor_len = Checkpoint::default().cursor.to_bytes().len();
    let rejection_count = 6 + 8 + cursor_len;
    for offset in [6, rejection_count] {
        let mut bytes = empty.clone();
        bytes[offset..offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Checkpoint::from_bytes(&bytes).is_err());
    }
    let mut pinned = Checkpoint::new(Follow::Pinned(vec![])).to_bytes();
    pinned[6..14].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Checkpoint::from_bytes(&pinned).is_err());

    let mut checkpoint = Checkpoint::default();
    checkpoint.rejected.insert(CommitHash::from([7; 32]), true);
    let mut bad_flag = checkpoint.to_bytes();
    bad_flag[rejection_count + 8 + 32] = 2;
    assert!(Checkpoint::from_bytes(&bad_flag).is_err());
    let mut duplicate = checkpoint.to_bytes();
    duplicate[rejection_count..rejection_count + 8].copy_from_slice(&2u64.to_le_bytes());
    let entry = duplicate[rejection_count + 8..rejection_count + 8 + 33].to_vec();
    duplicate.splice(rejection_count + 8 + 33..rejection_count + 8 + 33, entry);
    assert!(Checkpoint::from_bytes(&duplicate).is_err());
}

fn with_partial_target(target: &[u8]) -> Vec<u8> {
    let mut bytes = Checkpoint::default().to_bytes();
    assert_eq!(bytes.pop(), Some(0));
    bytes.push(1);
    bytes.extend(target);
    encode_hashes(&mut bytes, &[]);
    bytes.extend(0u64.to_le_bytes());
    bytes
}

#[test]
fn malformed_nested_views_and_duplicate_authors_are_rejected() {
    let mut view = Vec::new();
    encode_hashes(&mut view, &[]);
    view.extend(2u64.to_le_bytes());
    for _ in 0..2 {
        view.extend(author(1).as_slice());
        view.push(0);
    }
    assert!(Checkpoint::from_bytes(&with_partial_target(&view)).is_err());
    let mut view = Vec::new();
    encode_hashes(&mut view, &[]);
    view.extend(1u64.to_le_bytes());
    view.extend(author(1).as_slice());
    view.push(3); // unknown permission
    assert!(Checkpoint::from_bytes(&with_partial_target(&view)).is_err());
    view[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Checkpoint::from_bytes(&with_partial_target(&view)).is_err());
    let mut view = Vec::new();
    view.extend(u64::MAX.to_le_bytes()); // oversized head count
    assert!(Checkpoint::from_bytes(&with_partial_target(&view)).is_err());
}

#[test]
fn single_byte_corruptions_never_panic() {
    let original = fixture().to_bytes();
    for offset in 0..original.len() {
        let mut bytes = original.clone();
        bytes[offset] ^= 0xff;
        if let Ok(decoded) = Checkpoint::from_bytes(&bytes) {
            assert_eq!(
                Checkpoint::from_bytes(&decoded.to_bytes()).unwrap(),
                decoded
            );
        }
    }
}

#[derive(Clone)]
struct Commit {
    author: Author,
    parents: Vec<CommitHash>,
    tag: u64,
    hash: CommitHash,
}

fn add(
    history: &mut History,
    log: &mut Vec<Commit>,
    who: Author,
    parents: &[CommitHash],
) -> CommitHash {
    let tag = log.len() as u64;
    let hash = commit(history, who, parents, tag);
    log.push(Commit {
        author: who,
        parents: parents.to_vec(),
        tag,
        hash,
    });
    hash
}

fn summary(records: &[Change]) -> Vec<(CommitHash, bool, Vis)> {
    records
        .iter()
        .map(|c| (c.hash, c.newly_delivered, c.visibility))
        .collect()
}

#[test]
fn partial_selection_replays_after_reordered_rebuild_and_later_deliveries() {
    let (a, b, c) = (author(1), author(2), author(3));
    let mut history = History::new();
    let mut log = Vec::new();
    let root = add(&mut history, &mut log, a, &[]);
    let bob = add(&mut history, &mut log, b, &[root]);
    let alice = add(&mut history, &mut log, a, &[bob]);
    add(&mut history, &mut log, b, &[root]);
    let outside = add(&mut history, &mut log, c, &[]);
    let permissions = WritePermissions::from([
        (a, WritePermission::Unbounded),
        (b, WritePermission::Unbounded),
        (c, WritePermission::Unbounded),
    ]);
    let initial = history.view_point(&permissions);
    let (_, cursor) = drain(
        &history,
        &mut history
            .changes(&MaterializationCursor::new(), &initial)
            .unwrap(),
    );
    let later_bob = add(&mut history, &mut log, b, &[alice]);
    let tip = add(&mut history, &mut log, a, &[later_bob]);
    let newly_outside = add(&mut history, &mut log, c, &[outside]);
    let target = ViewPoint::from_parts(
        vec![tip],
        WritePermissions::from([(a, WritePermission::Unbounded)]),
    );
    let captured: Vec<_> = history.heads(Heads::Delivered).collect();
    let (expected, completed) = drain(&history, &mut history.changes(&cursor, &target).unwrap());
    assert!(
        expected
            .iter()
            .any(|r| r.hash == newly_outside && r.newly_delivered)
    );

    // Rebuild in a different topological order with different local payload
    // IDs, then grow both existing chains and an unrelated root before resume.
    let mut rebuilt = History::new();
    let mut pending: Vec<_> = log.iter().rev().collect();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .position(|record| record.parents.iter().all(|h| rebuilt.lookup(h).is_some()))
            .unwrap();
        let record = pending.remove(next);
        let mut builder = rebuilt
            .begin_commit(record.author, record.parents.clone())
            .unwrap();
        builder.update(&record.tag.to_le_bytes());
        let prepared = builder.finish();
        assert_eq!(prepared.hash, record.hash);
        rebuilt
            .publish_commit(prepared, record.tag + 10_000)
            .unwrap();
    }
    commit(&mut rebuilt, a, &[tip], 1000);
    commit(&mut rebuilt, c, &[newly_outside], 1001);
    commit(&mut rebuilt, b, &[], 1002);

    for count in 0..=expected.len() {
        let checkpoint = Checkpoint {
            follow: Follow::Pinned(vec![tip]),
            cursor: cursor.clone(),
            rejected: BTreeMap::new(),
            in_progress: Some(InProgress {
                target: target.clone(),
                delivery_heads: captured.clone(),
                applied: count as u64,
            }),
        };
        let decoded = Checkpoint::from_bytes(&checkpoint.to_bytes()).unwrap();
        let progress = decoded.in_progress.as_ref().unwrap();
        let mut changes = view::plan_changes_at(
            &rebuilt,
            &decoded.cursor,
            &progress.target,
            &progress.delivery_heads,
        )
        .unwrap();
        let mut skipped = Vec::new();
        rebuilt
            .next_changes(&mut changes, &mut skipped, progress.applied as usize)
            .unwrap();
        assert_eq!(summary(&skipped), summary(&expected[..count]));
        let (remaining, next) = drain(&rebuilt, &mut changes);
        assert_eq!(summary(&remaining), summary(&expected[count..]));
        assert_eq!(next, completed);
        // The next traversal sees the new commits, not the replayed one.
        let (later, _) = drain(&rebuilt, &mut rebuilt.changes(&next, &target).unwrap());
        assert_eq!(later.len(), 3);
        assert!(later.iter().all(|r| r.newly_delivered));
    }
    let missing = [CommitHash::from([0xff; 32])];
    assert!(matches!(
        view::plan_changes_at(&rebuilt, &cursor, &target, &missing),
        Err(Error::UnknownCommit(_))
    ));
    assert!(matches!(
        view::plan_changes_at(&rebuilt, &cursor, &target, &[root]),
        Err(Error::InvalidItem(_))
    ));
}
