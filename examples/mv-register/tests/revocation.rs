//! Revocation changes both a write's value and its overwriting effect.
use std::collections::{BTreeSet, HashMap};
use stockfoil::WritePermission;
use stockfoil_mv_register::MvRegisterMap;

fn values(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn revocation_restores_values_without_losing_history() {
    let mut alice = MvRegisterMap::new();
    alice.set("title", "original").unwrap();
    let mut bob = alice.fork().unwrap();
    let allowed = HashMap::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&allowed);
    bob.set_write_permissions(&allowed);
    let bound = alice.set("title", "allowed").unwrap();
    bob.set("title", "concurrent").unwrap();
    alice.set("title", "revoked").unwrap();
    alice.set("private", "secret").unwrap();
    bob.merge(&alice).unwrap();
    bob.set("description", "still here").unwrap();
    alice.merge(&bob).unwrap();
    let before = alice.view_point();
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));

    let restricted = HashMap::from([
        (alice.author(), WritePermission::Bounded(vec![bound])),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&restricted);
    let after = alice.view_point();
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));
    assert!(alice.get("private").is_empty());
    // Bob's descendant of revoked writes still participates.
    assert_eq!(alice.get("description"), values(&["still here"]));
    assert_eq!(
        alice.get_at(&before, "title").unwrap(),
        values(&["revoked", "concurrent"])
    );
    assert_eq!(
        bob.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
    assert_eq!(bob.get("title"), values(&["revoked", "concurrent"]));
    alice.merge(&bob).unwrap();
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));

    // Restoring permission needs no replay or new signatures.
    alice.set_write_permissions(&allowed);
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));
    assert_eq!(alice.get("private"), values(&["secret"]));
    assert!(alice.get_at(&after, "private").unwrap().is_empty());

    // Supersession still follows ancestry through revoked writes.
    bob.set_write_permissions(&restricted);
    bob.set("title", "descendant wins").unwrap();
    assert_eq!(bob.get("title"), values(&["descendant wins"]));
    bob.set_write_permissions(&allowed);
    assert_eq!(bob.get("title"), values(&["descendant wins"]));
    assert_eq!(
        bob.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
}
