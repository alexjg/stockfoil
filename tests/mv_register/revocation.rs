//! End-to-end revocation and historical reads through the MVR API.

use std::collections::{BTreeSet, HashMap};

use stockfoil::WritePermission;

use super::MvRegisterMap;

fn values(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn revocation_restores_values_without_losing_history() {
    let mut alice = MvRegisterMap::new();
    let alice_id = alice.author();
    alice.set("title", "original");
    let mut bob = alice.fork();
    let bob_id = bob.author();
    let allowed = HashMap::from([
        (alice_id, WritePermission::Unbounded),
        (bob_id, WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&allowed);
    bob.set_write_permissions(&allowed);

    // Concurrent branches, captured as document views rather than graph queries.
    let bound = alice.set("title", "allowed");
    let alice_branch = alice.view_point();
    bob.set("title", "concurrent");
    let bob_branch = bob.view_point();
    alice.set("title", "revoked");
    alice.set("private", "hidden after revocation");

    bob.merge(&alice);
    bob.set("description", "still here");
    alice.merge(&bob);
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));
    assert_eq!(alice.get("private"), values(&["hidden after revocation"]));
    assert_eq!(alice.get("description"), values(&["still here"]));
    assert_eq!(
        alice.get_at(&alice_branch, "title").unwrap(),
        values(&["allowed"])
    );
    assert_eq!(
        alice.get_at(&bob_branch, "title").unwrap(),
        values(&["concurrent"])
    );

    let before = alice.view_point();
    let restricted = HashMap::from([
        (alice_id, WritePermission::Bounded(vec![bound])),
        (bob_id, WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&restricted);
    let after = alice.view_point();

    // Restore the overwritten value and remove the new key. Bob's concurrent
    // write and his descendant of Alice's revoked writes still contribute.
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));
    assert!(alice.get("private").is_empty());
    assert_eq!(alice.get("description"), values(&["still here"]));

    // Old viewpoints don't change. Reading another replica's view doesn't adopt its
    // permissions as the reader's current policy.
    assert_eq!(
        alice.get_at(&before, "title").unwrap(),
        values(&["revoked", "concurrent"])
    );
    assert_eq!(
        bob.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
    assert!(bob.get_at(&after, "private").unwrap().is_empty());
    assert_eq!(bob.get("title"), values(&["revoked", "concurrent"]));

    // Repeated delivery doesn't import permissions or undo revocation.
    alice.merge(&bob);
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));
    assert!(alice.get("private").is_empty());
    bob.set_write_permissions(&restricted);
    bob.merge(&alice);
    assert_eq!(bob.get("title"), alice.get("title"));
    assert_eq!(bob.get("description"), alice.get("description"));
    assert!(bob.get("private").is_empty());

    // Omitting Alice, explicit denial, and an empty bound all deny her writes.
    for permissions in [
        HashMap::from([(bob_id, WritePermission::Unbounded)]),
        HashMap::from([
            (alice_id, WritePermission::Denied),
            (bob_id, WritePermission::Unbounded),
        ]),
        HashMap::from([
            (alice_id, WritePermission::Bounded(vec![])),
            (bob_id, WritePermission::Unbounded),
        ]),
    ] {
        alice.set_write_permissions(&permissions);
        assert_eq!(alice.get("title"), values(&["concurrent"]));
        assert_eq!(alice.get("description"), values(&["still here"]));
        assert!(alice.get("private").is_empty());
    }

    // Restoring permission needs no replay or new signatures.
    alice.set_write_permissions(&allowed);
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));
    assert_eq!(alice.get("private"), values(&["hidden after revocation"]));
    assert_eq!(
        alice.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
    assert!(alice.get_at(&after, "private").unwrap().is_empty());

    // Bob can overwrite through revoked history. Re-enabling Alice's writes
    // must not resurrect values superseded by Bob.
    bob.set("title", "descendant wins");
    assert_eq!(bob.get("title"), values(&["descendant wins"]));
    bob.set_write_permissions(&allowed);
    assert_eq!(bob.get("title"), values(&["descendant wins"]));

    // Unbounded permits future writes without expanding captured views.
    alice.set("title", "future write");
    assert_eq!(alice.get("title"), values(&["future write"]));
    assert_eq!(
        alice.get_at(&before, "title").unwrap(),
        values(&["revoked", "concurrent"])
    );
    assert_eq!(
        alice.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
}
