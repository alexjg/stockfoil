//! Owned ancestry snapshots do not freeze the mutable history, copy its hash
//! index, or silently reinterpret coordinates in a different history.
use ed25519_dalek::SigningKey;
use std::collections::HashSet;
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
fn normalized_heads_keep_redundant_ancestors_and_share_storage_on_clone() {
    let mut history = History::new();
    let root = commit(&mut history, author(1), &[], 0);
    let child = commit(&mut history, author(1), &[root], 1);
    let unrelated = commit(&mut history, author(2), &[], 2);
    let requested = [unrelated, child, root, child, root, unrelated];
    let captured = history
        .ancestry(&requested, &WritePermissions::new())
        .unwrap();
    let mut expected = vec![root, child, unrelated];
    expected.sort_unstable();
    assert_eq!(captured.heads(), expected);
    let cloned = captured.clone();
    assert!(std::ptr::eq(captured.heads(), cloned.heads()));
    drop(captured);
    assert_eq!(cloned.heads(), expected);
    assert!(cloned.contains(&history, &root).unwrap());
    assert!(!cloned.participates(&history, &root).unwrap());
}

#[test]
fn delivered_head_shortcut_preserves_exact_requested_ancestry() {
    let mut history = History::new();
    let permissions = WritePermissions::new();
    let empty = history.ancestry(&[], &permissions).unwrap();
    let root = commit(&mut history, author(1), &[], 10);
    let unrelated = commit(&mut history, author(2), &[], 11);
    let child = commit(&mut history, author(1), &[root, root], 12);
    let mut expected_heads = vec![child, unrelated];
    expected_heads.sort_unstable();
    assert_eq!(
        history.heads(Heads::Delivered).collect::<Vec<_>>(),
        expected_heads
    );
    assert_eq!(
        history.parents(&child).unwrap().collect::<Vec<_>>(),
        vec![root, root]
    );
    assert!(!empty.contains(&history, &root).unwrap());

    // Each request has the same raw length as the delivered head set. Neither
    // duplicates nor a redundant ancestor may substitute for the unrelated head.
    for requested in [vec![child, child], vec![child, root]] {
        let at = history.ancestry(&requested, &permissions).unwrap();
        assert!(at.contains(&history, &root).unwrap());
        assert!(at.contains(&history, &child).unwrap());
        assert!(!at.contains(&history, &unrelated).unwrap());
        assert_eq!(
            history
                .commits_since(&requested)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![unrelated]
        );
    }
    for requested in [vec![child, unrelated, child], vec![root, unrelated, child]] {
        let at = history.ancestry(&requested, &permissions).unwrap();
        for hash in [root, unrelated, child] {
            assert!(at.contains(&history, &hash).unwrap());
        }
    }
    let unknown = CommitHash::from([0xff; 32]);
    assert!(matches!(history.ancestry(&[child, unknown], &permissions),
        Err(Error::UnknownCommit(hash)) if hash == unknown));
    let saved = history.ancestry(&expected_heads, &permissions).unwrap();
    let later = commit(&mut history, author(3), &[], 13);
    assert!(!saved.contains(&history, &later).unwrap());
    assert!(history.heads(Heads::Delivered).any(|hash| hash == later));
}

#[test]
fn causal_membership_includes_hidden_ancestors_of_visible_descendants() {
    let mut history = History::new();
    let hidden = commit(&mut history, author(1), &[], 0);
    let visible = commit(&mut history, author(2), &[hidden], 1);
    let at = history
        .ancestry(
            &[visible],
            &WritePermissions::from([(author(2), WritePermission::Unbounded)]),
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
fn unlisted_rule_covers_every_author_without_an_entry() {
    let mut history = History::new();
    let root = commit(&mut history, author(1), &[], 0);
    let later = commit(&mut history, author(1), &[root], 1);
    let stranger = commit(&mut history, author(2), &[later], 2);
    let denied = commit(&mut history, author(3), &[stranger], 3);
    let heads = [denied];
    let participating = |permissions: &WritePermissions| {
        let at = history.ancestry(&heads, permissions).unwrap();
        [root, later, stranger, denied]
            .into_iter()
            .filter(|hash| at.participates(&history, hash).unwrap())
            .collect::<Vec<_>>()
    };
    // Denied by default; unbounded lets anyone in, and a listed rule overrides it.
    assert!(participating(&WritePermissions::new()).is_empty());
    let open = WritePermissions::from([(author(3), WritePermission::Denied)])
        .with_unlisted(WritePermission::Unbounded);
    assert_eq!(open.permission(&author(2)), &WritePermission::Unbounded);
    assert_eq!(participating(&open), [root, later, stranger]);
    // A bounded unlisted rule shares one frontier across all unlisted authors.
    let closed = WritePermissions::new().with_unlisted(WritePermission::Bounded(vec![later]));
    assert_eq!(participating(&closed), [root, later]);
}

#[test]
fn snapshots_survive_growth_permission_changes_and_dropping_the_original_handle() {
    let mut history = History::new();
    let a = author(1);
    let root = commit(&mut history, a, &[], 0);
    let mut permissions = WritePermissions::from([(a, WritePermission::Unbounded)]);
    let at = history.ancestry(&[root, root], &permissions).unwrap();
    assert_eq!(at.heads(), &[root]);
    let captured = at.clone();
    drop(at);

    permissions.remove(&a);
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
    let permissions = WritePermissions::from([(a, WritePermission::Bounded(vec![future]))]);
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
    let l = left.ancestry(&[hash], &WritePermissions::new()).unwrap();
    let r = right.ancestry(&[hash], &WritePermissions::new()).unwrap();
    assert_eq!(l.contains(&right, &hash), Err(Error::WrongHistory));
    assert_eq!(l.participates(&right, &hash), Err(Error::WrongHistory));
    let unknown = CommitHash::from([0xff; 32]);
    assert_eq!(l.contains(&right, &unknown), Err(Error::WrongHistory));
    assert_eq!(l.participates(&right, &unknown), Err(Error::WrongHistory));
    assert_eq!(l.difference(&r, &left).unwrap_err(), Error::WrongHistory);
    assert_eq!(l.difference(&l, &right).unwrap_err(), Error::WrongHistory);
    assert_eq!(r.difference(&l, &left).unwrap_err(), Error::WrongHistory);
}

#[test]
fn unknown_heads_fail_and_empty_heads_mean_empty_past() {
    let mut history = History::new();
    let a = author(1);
    let unknown = CommitHash::from([0xff; 32]);
    let empty = history.ancestry(&[], &WritePermissions::new()).unwrap();
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
            .ancestry(&[hash, unknown], &WritePermissions::new())
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
        .ancestry(
            &[left],
            &WritePermissions::from([(a, WritePermission::Unbounded)]),
        )
        .unwrap();
    let after = history
        .ancestry(&[joined], &WritePermissions::new())
        .unwrap();
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
