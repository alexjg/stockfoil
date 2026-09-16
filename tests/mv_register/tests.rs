use super::MvRegisterMap;
use std::collections::BTreeSet;

fn values(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn conflicting_replicas() -> (MvRegisterMap, MvRegisterMap) {
    let mut left = MvRegisterMap::new();
    left.set("title", "Hi");
    let mut right = left.fork();
    left.set("title", "Hello");
    right.set("title", "Howdy");
    (left, right)
}

#[test]
fn sequential_assignments_overwrite() {
    let mut doc = MvRegisterMap::new();
    assert!(doc.get("title").is_empty());
    doc.set("title", "Hi");
    assert_eq!(doc.get("title"), values(&["Hi"]));
    doc.set("title", "Hello");
    assert_eq!(doc.get("title"), values(&["Hello"]));
}

#[test]
fn concurrent_assignments_survive_both_merge_directions() {
    let (mut left, mut right) = conflicting_replicas();
    let original_left = left.fork();
    left.merge(&right);
    right.merge(&original_left);
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(right.get("title"), left.get("title"));

    // Neither repeated delivery nor exchanging merged histories changes the result.
    for _ in 0..3 {
        left.merge(&right);
        right.merge(&left);
        assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
        assert_eq!(right.get("title"), left.get("title"));
    }
}

#[test]
fn assignment_after_merge_resolves_conflict() {
    let (mut left, mut right) = conflicting_replicas();
    left.merge(&right);
    left.set("title", "Welcome");
    assert_eq!(left.get("title"), values(&["Welcome"]));
    right.merge(&left);
    assert_eq!(right.get("title"), left.get("title"));
}

#[test]
fn unrelated_key_does_not_resolve_conflict() {
    let (mut left, mut right) = conflicting_replicas();
    left.merge(&right);
    left.set("description", "A greeting");
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(left.get("description"), values(&["A greeting"]));
    right.merge(&left);
    assert_eq!(right.get("title"), left.get("title"));
    assert_eq!(right.get("description"), left.get("description"));

    // Both title writes are ancestors, even though the immediate parent writes
    // a different key. Supersession must account for transitive ancestry.
    right.set("title", "Welcome");
    left.merge(&right);
    assert_eq!(left.get("title"), values(&["Welcome"]));
    assert_eq!(left.get("description"), values(&["A greeting"]));
}

#[test]
fn identical_concurrent_values_keep_distinct_write_identities() {
    let mut left = MvRegisterMap::new();
    let mut right = left.fork();
    left.set("title", "Same");
    right.set("title", "Same");
    // Only the left-hand write has been observed by this assignment.
    left.set("title", "Updated");
    left.merge(&right);
    right.merge(&left);
    assert_eq!(left.get("title"), values(&["Same", "Updated"]));
    assert_eq!(right.get("title"), left.get("title"));
}

#[test]
fn three_way_merge_preserves_only_causally_maximal_writes() {
    let (mut left, mut right) = conflicting_replicas();
    let mut third = left.fork();
    third.set("title", "Welcome");
    // Welcome supersedes Hello but is concurrent with Howdy.
    left.merge(&right);
    left.merge(&third);
    right.merge(&third);
    right.merge(&left);
    third.merge(&right);
    for doc in [&left, &right, &third] {
        assert_eq!(doc.get("title"), values(&["Welcome", "Howdy"]));
    }
}
