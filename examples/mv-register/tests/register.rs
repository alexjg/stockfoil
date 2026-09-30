use std::collections::{BTreeSet, HashMap};
use stockfoil::WritePermission;
use stockfoil_mv_register::MvRegisterMap;

fn grant_access(docs: &mut [&mut MvRegisterMap]) {
    let permissions = docs
        .iter()
        .map(|doc| (doc.author(), WritePermission::Unbounded))
        .collect::<HashMap<_, _>>();
    for doc in docs {
        doc.set_write_permissions(&permissions);
    }
}

fn values(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn conflicting_replicas() -> (MvRegisterMap, MvRegisterMap) {
    let mut left = MvRegisterMap::new();
    left.set("title", "Hi").unwrap();
    let mut right = left.fork().unwrap();
    grant_access(&mut [&mut left, &mut right]);
    left.set("title", "Hello").unwrap();
    right.set("title", "Howdy").unwrap();
    (left, right)
}

#[test]
fn sequential_assignments_overwrite() {
    let mut doc = MvRegisterMap::new();
    assert!(doc.get("title").is_empty());
    doc.set("title", "Hi").unwrap();
    assert_eq!(doc.get("title"), values(&["Hi"]));
    doc.set("title", "Hello").unwrap();
    assert_eq!(doc.get("title"), values(&["Hello"]));
}

#[test]
fn concurrent_assignments_survive_both_merge_directions() {
    let (mut left, mut right) = conflicting_replicas();
    let original_left = left.fork().unwrap();
    left.merge(&right).unwrap();
    right.merge(&original_left).unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(right.get("title"), left.get("title"));

    // Neither repeated delivery nor exchanging merged histories changes the result.
    left.merge(&right).unwrap();
    right.merge(&left).unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(right.get("title"), left.get("title"));
}

#[test]
fn unrelated_key_does_not_resolve_conflict() {
    let (mut left, mut right) = conflicting_replicas();
    left.merge(&right).unwrap();
    left.set("description", "A greeting").unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(left.get("description"), values(&["A greeting"]));
    right.merge(&left).unwrap();
    assert_eq!(right.get("title"), left.get("title"));
    assert_eq!(right.get("description"), left.get("description"));

    // Both title writes are ancestors, even though the immediate parent writes
    // a different key. Supersession must account for transitive ancestry.
    right.set("title", "Welcome").unwrap();
    left.merge(&right).unwrap();
    assert_eq!(left.get("title"), values(&["Welcome"]));
    assert_eq!(left.get("description"), values(&["A greeting"]));
}

#[test]
fn identical_concurrent_values_keep_distinct_write_identities() {
    let mut left = MvRegisterMap::new();
    let mut right = left.fork().unwrap();
    grant_access(&mut [&mut left, &mut right]);
    left.set("title", "Same").unwrap();
    right.set("title", "Same").unwrap();
    // Only the left-hand write has been observed by this assignment.
    left.set("title", "Updated").unwrap();
    left.merge(&right).unwrap();
    right.merge(&left).unwrap();
    assert_eq!(left.get("title"), values(&["Same", "Updated"]));
    assert_eq!(right.get("title"), left.get("title"));
}

#[test]
fn three_way_merge_preserves_only_causally_maximal_writes() {
    let (mut left, mut right) = conflicting_replicas();
    let mut third = left.fork().unwrap();
    grant_access(&mut [&mut left, &mut right, &mut third]);
    third.set("title", "Welcome").unwrap();
    // Welcome supersedes Hello but is concurrent with Howdy.
    left.merge(&right).unwrap();
    left.merge(&third).unwrap();
    right.merge(&third).unwrap();
    right.merge(&left).unwrap();
    third.merge(&right).unwrap();
    for doc in [&left, &right, &third] {
        assert_eq!(doc.get("title"), values(&["Welcome", "Howdy"]));
    }
}
