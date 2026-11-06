//! Register semantics: overwrite, concurrency, and causal supersession.
use std::collections::{BTreeSet, HashMap};
use stockfoil::WritePermission;
use stockfoil_mv_register::MvRegisterMap;

fn grant_access(docs: &mut [&mut MvRegisterMap]) {
    let permissions: HashMap<_, _> = docs
        .iter()
        .map(|doc| (doc.author(), WritePermission::Unbounded))
        .collect();
    for doc in docs {
        doc.set_write_permissions(permissions.clone()).unwrap();
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
    let mut original_left = left.fork().unwrap();
    left.merge(&mut right).unwrap();
    right.merge(&mut original_left).unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(right.get("title"), left.get("title"));

    // Neither repeated delivery nor exchanging merged histories changes the result.
    left.merge(&mut right).unwrap();
    right.merge(&mut left).unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(right.get("title"), left.get("title"));
}

#[test]
fn unrelated_key_does_not_resolve_conflict() {
    let (mut left, mut right) = conflicting_replicas();
    left.merge(&mut right).unwrap();
    left.set("description", "A greeting").unwrap();
    assert_eq!(left.get("title"), values(&["Hello", "Howdy"]));
    assert_eq!(left.get("description"), values(&["A greeting"]));
    right.merge(&mut left).unwrap();
    assert_eq!(right.get("title"), left.get("title"));
    assert_eq!(right.get("description"), left.get("description"));

    // Both title writes are ancestors, even though the immediate parent writes
    // a different key. Supersession must account for transitive ancestry.
    right.set("title", "Welcome").unwrap();
    left.merge(&mut right).unwrap();
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
    left.merge(&mut right).unwrap();
    right.merge(&mut left).unwrap();
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
    left.merge(&mut right).unwrap();
    left.merge(&mut third).unwrap();
    right.merge(&mut third).unwrap();
    right.merge(&mut left).unwrap();
    third.merge(&mut right).unwrap();
    for doc in [&left, &right, &third] {
        assert_eq!(doc.get("title"), values(&["Welcome", "Howdy"]));
    }
}

#[test]
fn keys_are_bounded() {
    let mut doc = MvRegisterMap::new();
    let long = "k".repeat(stockfoil_mv_register::codec::MAX_KEY_BYTES + 1);
    assert!(doc.set(&long, "v").is_err());
    let max = &long[1..];
    doc.set(max, "v").unwrap();
    let mut peer = doc.fork().unwrap();
    peer.set_write_permissions(doc.write_permissions().clone())
        .unwrap();
    assert_eq!(peer.get(max), values(&["v"]));
}
