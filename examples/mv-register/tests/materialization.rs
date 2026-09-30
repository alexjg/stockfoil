//! Black-box checks that delivery, permissions, and historical reads all use the
//! same materialization path without losing stored writes or moving live state.
use std::collections::{BTreeSet, HashMap};

use stockfoil::WritePermission;

use stockfoil_mv_register::MvRegisterMap;

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn denied_deliveries_can_be_enabled_and_revoked_without_another_merge() {
    let mut writer = MvRegisterMap::new();
    let first = writer.set("title", "first").unwrap();
    let mut reader = MvRegisterMap::new();
    reader.merge(&writer).unwrap();
    writer.set("title", "second").unwrap();
    reader.merge(&writer).unwrap();
    assert!(reader.get("title").is_empty());

    // Both writes arrived inactive, but the materializer must have kept them.
    reader.set_write_permissions(&HashMap::from([(
        writer.author(),
        WritePermission::Unbounded,
    )]));
    assert_eq!(reader.get("title"), values("second"));
    reader.set_write_permissions(&HashMap::from([(
        writer.author(),
        WritePermission::Bounded(vec![first]),
    )]));
    assert_eq!(reader.get("title"), values("first"));
    reader.merge(&writer).unwrap(); // Repeated delivery must not undo the restriction.
    assert_eq!(reader.get("title"), values("first"));
    reader.set_write_permissions(&HashMap::new());
    assert!(reader.get("title").is_empty());
}

#[test]
fn historical_and_failed_reads_do_not_change_the_live_materialization() {
    let mut doc = MvRegisterMap::new();
    doc.set("title", "first").unwrap();
    let first = doc.view_point();
    doc.set("title", "second").unwrap();
    let second = doc.view_point();
    doc.set("description", "current").unwrap();
    for _ in 0..3 {
        assert_eq!(doc.get_at(&first, "title").unwrap(), values("first"));
        assert!(doc.get_at(&first, "description").unwrap().is_empty());
        assert_eq!(doc.get_at(&second, "title").unwrap(), values("second"));
        assert_eq!(doc.get("title"), values("second"));
        assert_eq!(doc.get("description"), values("current"));
    }

    let mut unrelated = MvRegisterMap::new();
    unrelated.set("title", "missing history").unwrap();
    assert!(doc.get_at(&unrelated.view_point(), "title").is_err());
    assert_eq!(doc.get("title"), values("second"));
    doc.set("title", "third").unwrap();
    assert_eq!(doc.get("title"), values("third"));
    assert_eq!(doc.get("description"), values("current"));
    assert_eq!(doc.get_at(&first, "title").unwrap(), values("first"));
    assert_eq!(doc.get("title"), values("third"));
}

#[test]
fn receiving_a_denied_frontier_can_enable_an_already_stored_assignment() {
    let mut alice = MvRegisterMap::new();
    alice.set("title", "visible with proof").unwrap();
    let mut bob = alice.fork().unwrap();
    let frontier = bob.set("witness", "not permitted").unwrap();
    alice.set_write_permissions(&HashMap::from([(
        alice.author(),
        WritePermission::Bounded(vec![frontier]),
    )]));
    let before = alice.view_point();
    assert!(alice.get("title").is_empty());
    alice.merge(&bob).unwrap();
    assert_eq!(alice.get("title"), values("visible with proof"));
    assert!(alice.get("witness").is_empty());
    assert!(alice.get_at(&before, "title").unwrap().is_empty());
    assert_eq!(alice.get("title"), values("visible with proof"));
    alice.merge(&bob).unwrap();
    assert_eq!(alice.get("title"), values("visible with proof"));
}
