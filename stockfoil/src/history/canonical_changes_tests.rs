//! Traversal order is part of the resumable-materialization contract: neither
//! item arrival order nor batching may change the record at a given position.
use crate::history::Change;
use crate::*;
use ed25519_dalek::SigningKey;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::collections::{BTreeMap, HashMap};

struct Commit {
    author: Author,
    parents: Vec<CommitHash>,
    bytes: Vec<u8>,
    id: PayloadId,
}

fn publish(history: &mut History, commit: &Commit) -> CommitHash {
    let mut builder = history
        .begin_commit(commit.author, commit.parents.clone())
        .unwrap();
    builder.update(&commit.bytes);
    let prepared = builder.finish();
    let hash = prepared.hash;
    history.publish_commit(prepared, commit.id).unwrap();
    hash
}

fn author(seed: u8) -> Author {
    Author::from(&SigningKey::from_bytes(&[seed; 32]))
}

fn drain(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
    size: usize,
) -> (Vec<Change>, MaterializationCursor) {
    let mut traversal = history.changes(cursor, target).unwrap();
    let count = traversal.remaining();
    let mut records = Vec::new();
    // A zero-sized poll is inert, even if the traversal has no records.
    history
        .next_changes(&mut traversal, &mut records, 0)
        .unwrap();
    assert!(records.is_empty());
    assert_eq!(traversal.remaining(), count);
    loop {
        let result = history
            .next_changes(&mut traversal, &mut records, size)
            .unwrap();
        assert_eq!(traversal.remaining(), count - records.len());
        if let ChangeBatch::Complete(next) = result {
            return (records, next);
        }
    }
}

#[test]
fn replay_order_and_batch_size_do_not_change_records() {
    for seed in 0..8 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut original = History::new();
        let mut commits = BTreeMap::new();
        let mut hashes = Vec::new();
        let authors: Vec<_> = (1..=4).map(author).collect();
        for id in 0..80 {
            let parents = if hashes.is_empty() {
                vec![]
            } else {
                (0..rng.random_range(0..=4))
                    .map(|_| hashes[rng.random_range(0..hashes.len())])
                    .collect()
            };
            let commit = Commit {
                author: authors[rng.random_range(0..authors.len())],
                parents,
                bytes: format!("{seed}-{id}").into_bytes(),
                id,
            };
            let hash = publish(&mut original, &commit);
            hashes.push(hash);
            commits.insert(hash, commit);
        }
        let mut replay = History::new();
        // Choose the largest ready hash at each insertion: a different valid
        // arrival order and, importantly, a different chain decomposition.
        let mut pending: Vec<_> = commits.iter().rev().collect();
        while !pending.is_empty() {
            let next = pending
                .iter()
                .position(|(_, c)| c.parents.iter().all(|p| replay.lookup(p).is_some()))
                .unwrap();
            let (&hash, commit) = pending.remove(next);
            assert_eq!(publish(&mut replay, commit), hash);
        }
        let mut cursor = MaterializationCursor::new();
        for _ in 0..20 {
            let permissions = authors
                .iter()
                .map(|&author| {
                    let permission = match rng.random_range(0..3) {
                        0 => WritePermission::Denied,
                        1 => WritePermission::Unbounded,
                        _ => WritePermission::Bounded(vec![
                            hashes[rng.random_range(0..hashes.len())],
                        ]),
                    };
                    (author, permission)
                })
                .collect();
            let heads = (0..rng.random_range(0..5))
                .map(|_| hashes[rng.random_range(0..hashes.len())])
                .collect();
            let target = ViewPoint::from_parts(heads, permissions);
            let original_at = original
                .ancestry(target.heads(), target.write_permissions())
                .unwrap();
            let replay_at = replay
                .ancestry(target.heads(), target.write_permissions())
                .unwrap();
            let original_before = original
                .ancestry(cursor.view_point().heads(), &HashMap::new())
                .unwrap();
            let replay_before = replay
                .ancestry(cursor.view_point().heads(), &HashMap::new())
                .unwrap();
            assert_eq!(
                original_at
                    .difference(&original_before, &original)
                    .unwrap()
                    .collect::<Vec<_>>(),
                replay_at
                    .difference(&replay_before, &replay)
                    .unwrap()
                    .collect::<Vec<_>>(),
            );
            assert_eq!(
                original
                    .commits_since(target.heads())
                    .unwrap()
                    .collect::<Vec<_>>(),
                replay
                    .commits_since(target.heads())
                    .unwrap()
                    .collect::<Vec<_>>(),
            );
            let (expected, next) = drain(&original, &cursor, &target, 1);
            let restored = MaterializationCursor::from_bytes(&cursor.to_bytes()).unwrap();
            for size in [1, 7, 1024] {
                let (got, got_cursor) = drain(&replay, &restored, &target, size);
                assert_eq!(got, expected, "seed {seed}, batch {size}");
                assert_eq!(got_cursor, next);
            }
            cursor = next;
        }
    }
}

#[test]
fn unchanged_bridge_still_orders_selected_ancestors_before_descendants() {
    let mut history = History::new();
    let root = publish(
        &mut history,
        &Commit {
            author: author(1),
            parents: vec![],
            bytes: b"root".to_vec(),
            id: 0,
        },
    );
    let bridge = publish(
        &mut history,
        &Commit {
            author: author(2),
            parents: vec![root, root],
            bytes: b"bridge".to_vec(),
            id: 1,
        },
    );
    // Force the descendant to have a smaller hash. A sort by hash, or a
    // topological sort considering only selected *direct* parents, is wrong.
    let leaf = (0u64..)
        .find_map(|nonce| {
            let commit = Commit {
                author: author(3),
                parents: vec![bridge],
                bytes: nonce.to_le_bytes().to_vec(),
                id: 2,
            };
            let mut builder = history
                .begin_commit(commit.author, commit.parents.clone())
                .unwrap();
            builder.update(&commit.bytes);
            let hash = builder.finish().hash;
            (hash < root).then(|| publish(&mut history, &commit))
        })
        .unwrap();
    let before = ViewPoint::from_parts(
        vec![leaf],
        HashMap::from([
            (author(1), WritePermission::Unbounded),
            (author(3), WritePermission::Unbounded),
        ]),
    );
    let (_, cursor) = drain(&history, &MaterializationCursor::new(), &before, 1);
    let after = ViewPoint::from_parts(vec![leaf], HashMap::new());
    let (records, _) = drain(&history, &cursor, &after, 1);
    assert_eq!(
        records.iter().map(|c| c.hash).collect::<Vec<_>>(),
        [root, leaf]
    );
    assert!(
        records
            .iter()
            .all(|c| !c.newly_delivered && c.visibility == Vis::Before)
    );
}

#[test]
fn smallest_ready_hash_wins_even_when_another_chain_can_continue() {
    let mut history = History::new();
    let a = author(1);
    // A chain and an unrelated root. Recompute the naive ready set after
    // every emission, including when the chain releases its next member.
    let root = publish(
        &mut history,
        &Commit {
            author: a,
            parents: vec![],
            bytes: b"r".to_vec(),
            id: 0,
        },
    );
    let child = publish(
        &mut history,
        &Commit {
            author: a,
            parents: vec![root],
            bytes: b"c".to_vec(),
            id: 1,
        },
    );
    let independent = publish(
        &mut history,
        &Commit {
            author: author(2),
            parents: vec![],
            bytes: b"i".to_vec(),
            id: 2,
        },
    );
    let target = history.view_point(&HashMap::new());
    let (records, _) = drain(&history, &MaterializationCursor::new(), &target, 1);
    let mut ready = vec![root, independent];
    ready.sort();
    let mut expected = vec![];
    while !ready.is_empty() {
        let hash = ready.remove(0);
        expected.push(hash);
        if hash == root {
            ready.push(child);
            ready.sort();
        }
    }
    assert_eq!(records.iter().map(|c| c.hash).collect::<Vec<_>>(), expected);
    assert!(
        records
            .iter()
            .all(|c| c.newly_delivered && c.visibility == Vis::Neither)
    );
}

#[test]
fn appending_during_a_traversal_does_not_extend_its_intervals() {
    let mut history = History::new();
    let a = author(1);
    let root = publish(
        &mut history,
        &Commit {
            author: a,
            parents: vec![],
            bytes: vec![0],
            id: 0,
        },
    );
    let child = publish(
        &mut history,
        &Commit {
            author: a,
            parents: vec![root],
            bytes: vec![1],
            id: 1,
        },
    );
    let target = history.view_point(&HashMap::new());
    let mut traversal = history
        .changes(&MaterializationCursor::new(), &target)
        .unwrap();
    let mut records = vec![];
    assert_eq!(
        history
            .next_changes(&mut traversal, &mut records, 1)
            .unwrap(),
        ChangeBatch::More
    );
    publish(
        &mut history,
        &Commit {
            author: a,
            parents: vec![child],
            bytes: vec![2],
            id: 2,
        },
    );
    publish(
        &mut history,
        &Commit {
            author: author(2),
            parents: vec![],
            bytes: vec![3],
            id: 3,
        },
    );
    let ChangeBatch::Complete(cursor) = history
        .next_changes(&mut traversal, &mut records, 10)
        .unwrap()
    else {
        panic!("captured traversal must finish");
    };
    assert_eq!(
        records.iter().map(|c| c.hash).collect::<Vec<_>>(),
        [root, child]
    );
    let next = history.view_point(&HashMap::new());
    assert_eq!(drain(&history, &cursor, &next, 1).0.len(), 2);
}
