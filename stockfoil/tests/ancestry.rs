//! Owned ancestry snapshots do not freeze the mutable history, copy its hash
//! index, or silently reinterpret coordinates in a different history.
use ed25519_dalek::SigningKey;
use std::collections::{HashMap, HashSet};
use stockfoil::*;

fn author(seed: u8) -> Author {
    Author::from(&SigningKey::from_bytes(&[seed; 32]))
}

fn commit(history: &mut History, who: Author, parents: &[CommitHash], tag: u64) -> CommitHash {
    let mut builder = history.begin_commit(who, parents.to_vec()).unwrap();
    builder.update(&tag.to_le_bytes());
    let prepared = builder.finish();
    let hash = prepared.hash;
    history.publish_commit(prepared, tag).unwrap();
    hash
}

#[test]
fn causal_membership_includes_hidden_ancestors_of_visible_descendants() {
    let mut history = History::new();
    let hidden = commit(&mut history, author(1), &[], 0);
    let visible = commit(&mut history, author(2), &[hidden], 1);
    let at = history
        .ancestry(
            &[visible],
            &HashMap::from([(author(2), WritePermission::Unbounded)]),
        )
        .unwrap();
    assert_eq!(at.heads(), &[visible]);
    assert!(at.contains(&history, &hidden).unwrap());
    assert!(!at.participates(&history, &hidden).unwrap());
    assert!(at.contains(&history, &visible).unwrap());
    assert!(at.participates(&history, &visible).unwrap());
    let unknown = CommitHash::from([0xff; 32]);
    assert!(!at.contains(&history, &unknown).unwrap());
    assert!(!at.participates(&history, &unknown).unwrap());
}

#[test]
fn snapshots_survive_growth_permission_changes_and_dropping_the_original_handle() {
    let mut history = History::new();
    let a = author(1);
    let root = commit(&mut history, a, &[], 0);
    let mut permissions = HashMap::from([(a, WritePermission::Unbounded)]);
    let at = history.ancestry(&[root, root], &permissions).unwrap();
    assert_eq!(at.heads(), &[root]);
    let captured = at.clone();
    drop(at);

    permissions.clear();
    let next = commit(&mut history, a, &[root], 1);
    let fork = commit(&mut history, a, &[root], 2);
    let joined = commit(&mut history, a, &[next, fork], 3);
    let now = history.ancestry(&[joined], &permissions).unwrap();
    assert!(captured.participates(&history, &root).unwrap());
    for hash in [next, fork, joined] {
        assert!(!captured.contains(&history, &hash).unwrap());
        assert!(!captured.participates(&history, &hash).unwrap());
    }
    assert!(!now.participates(&history, &root).unwrap());
    // The iterator does not borrow either ancestry handle, only the history.
    let difference = now.difference(&captured, &history).unwrap();
    drop(now);
    drop(captured);
    assert_eq!(
        difference.collect::<HashSet<_>>(),
        HashSet::from([next, fork, joined])
    );
}

#[test]
fn unavailable_and_out_of_view_frontiers_grant_nothing() {
    let mut history = History::new();
    let a = author(1);
    let root = commit(&mut history, a, &[], 0);
    let mut builder = history.begin_commit(a, vec![root]).unwrap();
    builder.update(&1u64.to_le_bytes());
    let prepared = builder.finish();
    let future = prepared.hash;
    let permissions = HashMap::from([(a, WritePermission::Bounded(vec![future]))]);
    let unavailable = history.ancestry(&[root], &permissions).unwrap();
    assert!(!unavailable.participates(&history, &root).unwrap());
    history.publish_commit(prepared, 1).unwrap();
    assert!(!unavailable.participates(&history, &root).unwrap());
    let outside = history.ancestry(&[root], &permissions).unwrap();
    assert!(!outside.participates(&history, &root).unwrap());
    let reached = history.ancestry(&[future], &permissions).unwrap();
    assert!(reached.participates(&history, &root).unwrap());
    assert!(reached.participates(&history, &future).unwrap());
}

#[test]
fn wrong_histories_are_rejected_even_when_they_contain_the_same_hashes() {
    let (mut left, mut right) = (History::new(), History::new());
    let a = author(1);
    let hash = commit(&mut left, a, &[], 0);
    assert_eq!(commit(&mut right, a, &[], 0), hash);
    let l = left.ancestry(&[hash], &HashMap::new()).unwrap();
    let r = right.ancestry(&[hash], &HashMap::new()).unwrap();
    assert_eq!(l.contains(&right, &hash), Err(Error::WrongHistory));
    assert_eq!(l.participates(&right, &hash), Err(Error::WrongHistory));
    assert_eq!(l.difference(&r, &left).unwrap_err(), Error::WrongHistory);
    assert_eq!(l.difference(&l, &right).unwrap_err(), Error::WrongHistory);
    assert_eq!(r.difference(&l, &left).unwrap_err(), Error::WrongHistory);
}

#[test]
fn unknown_heads_fail_and_empty_heads_mean_empty_past() {
    let mut history = History::new();
    let a = author(1);
    let unknown = CommitHash::from([0xff; 32]);
    let empty = history.ancestry(&[], &HashMap::new()).unwrap();
    assert!(empty.heads().is_empty());
    assert_eq!(history.commits_since(&[]).unwrap().len(), 0);
    let hash = commit(&mut history, a, &[], 0);
    assert!(!empty.contains(&history, &hash).unwrap());
    assert_eq!(
        history.commits_since(&[]).unwrap().collect::<Vec<_>>(),
        [hash]
    );
    assert_eq!(history.commits_since(&[hash, hash]).unwrap().len(), 0);
    assert_eq!(
        history
            .ancestry(&[hash, unknown], &HashMap::new())
            .unwrap_err(),
        Error::UnknownCommit(unknown)
    );
    assert_eq!(
        history.commits_since(&[hash, unknown]).unwrap_err(),
        Error::UnknownCommit(unknown)
    );
}

#[test]
fn set_differences_ignore_permissions_and_have_exact_decreasing_lengths() {
    let mut history = History::new();
    let a = author(1);
    let root = commit(&mut history, a, &[], 0);
    let left = commit(&mut history, a, &[root], 1);
    let right = commit(&mut history, a, &[root], 2);
    let joined = commit(&mut history, a, &[left, right], 3);
    let before = history
        .ancestry(&[left], &HashMap::from([(a, WritePermission::Unbounded)]))
        .unwrap();
    let after = history.ancestry(&[joined], &HashMap::new()).unwrap();
    let mut difference = after.difference(&before, &history).unwrap();
    assert_eq!(difference.size_hint(), (2, Some(2)));
    assert_eq!(difference.next(), Some(right));
    assert_eq!(difference.len(), 1);
    assert_eq!(difference.next(), Some(joined));
    assert_eq!(difference.size_hint(), (0, Some(0)));
    assert_eq!(difference.next(), None);
    assert_eq!(difference.next(), None);
    assert_eq!(after.difference(&after, &history).unwrap().len(), 0);
    assert_eq!(before.difference(&after, &history).unwrap().len(), 0);
    assert_eq!(
        history.commits_since(&[left]).unwrap().collect::<Vec<_>>(),
        [right, joined]
    );
}
