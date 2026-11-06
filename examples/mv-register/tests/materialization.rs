//! Delivery, permissions, and historical reads share one materialization path,
//! whether it runs incrementally or from a cold start.
use std::collections::BTreeSet;
use stockfoil::{ItemId, WritePermission, WritePermissions};
use stockfoil_mv_register::{Assignment, MvRegisterMap};

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn denied_deliveries_can_be_enabled_and_revoked_without_another_merge() {
    let mut writer = MvRegisterMap::new();
    let first = writer.set("title", "first").unwrap();
    let mut reader = MvRegisterMap::new();
    reader.merge(&mut writer).unwrap();
    writer.set("title", "second").unwrap();
    reader.merge(&mut writer).unwrap();
    assert!(reader.get("title").is_empty());

    // Both writes arrived inactive, but the materializer must have kept them.
    reader
        .set_write_permissions(WritePermissions::from([(
            writer.author(),
            WritePermission::Unbounded,
        )]))
        .unwrap();
    assert_eq!(reader.get("title"), values("second"));
    reader
        .set_write_permissions(WritePermissions::from([(
            writer.author(),
            WritePermission::Bounded(vec![first]),
        )]))
        .unwrap();
    assert_eq!(reader.get("title"), values("first"));
    reader.merge(&mut writer).unwrap(); // Repeated delivery must not undo the restriction.
    assert_eq!(reader.get("title"), values("first"));
    reader
        .set_write_permissions(WritePermissions::new())
        .unwrap();
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
        assert_eq!(doc.get_at(first.heads(), "title").unwrap(), values("first"));
        assert!(doc.get_at(first.heads(), "description").unwrap().is_empty());
        assert_eq!(
            doc.get_at(second.heads(), "title").unwrap(),
            values("second")
        );
        assert_eq!(doc.get("title"), values("second"));
        assert_eq!(doc.get("description"), values("current"));
    }

    let mut unrelated = MvRegisterMap::new();
    unrelated.set("title", "missing history").unwrap();
    assert!(doc.get_at(unrelated.view_point().heads(), "title").is_err());
    assert_eq!(doc.get("title"), values("second"));
    doc.set("title", "third").unwrap();
    assert_eq!(doc.get("title"), values("third"));
    assert_eq!(doc.get("description"), values("current"));
    assert_eq!(doc.get_at(first.heads(), "title").unwrap(), values("first"));
    assert_eq!(doc.get("title"), values("third"));
}

#[test]
fn receiving_a_denied_frontier_can_enable_an_already_stored_assignment() {
    let mut alice = MvRegisterMap::new();
    alice.set("title", "visible with proof").unwrap();
    let mut bob = alice.fork().unwrap();
    let frontier = bob.set("witness", "not permitted").unwrap();
    alice
        .set_write_permissions(WritePermissions::from([(
            alice.author(),
            WritePermission::Bounded(vec![frontier]),
        )]))
        .unwrap();
    let before = alice.view_point();
    assert!(alice.get("title").is_empty());
    alice.merge(&mut bob).unwrap();
    assert_eq!(alice.get("title"), values("visible with proof"));
    assert!(alice.get("witness").is_empty());
    assert!(alice.get_at(before.heads(), "title").unwrap().is_empty());
    assert_eq!(alice.get("title"), values("visible with proof"));
    alice.merge(&mut bob).unwrap();
    assert_eq!(alice.get("title"), values("visible with proof"));
}

/// A value whose assignment, committed next by `doc`, is a sedimentree
/// checkpoint (depth >= 1), so signing it assembles a fragment.
fn checkpoint_value(doc: &MvRegisterMap, key: &str) -> String {
    let history = doc.document().history();
    let heads: Vec<_> = history.heads(stockfoil::Heads::Delivered).collect();
    (0u64..)
        .map(|i| format!("checkpoint {i}"))
        .find(|value| {
            let mut builder = history.begin_commit(doc.author(), heads.clone()).unwrap();
            builder.update(&Assignment::new(key, value.as_str()).encode());
            builder.finish().hash.as_bytes()[0] == 0
        })
        .unwrap()
}

#[test]
fn cold_start_matches_incremental_materialization() {
    let mut alice = MvRegisterMap::new();
    let mut bob = alice.fork().unwrap();
    let mut permissions = WritePermissions::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(permissions.clone()).unwrap();
    bob.set_write_permissions(permissions.clone()).unwrap();
    // Enough writes for several change batches, with concurrency and a bound.
    let mut bound = None;
    for i in 0..400 {
        let key = format!("key{}", i % 13);
        let hash = if i % 3 == 0 {
            bob.set(&key, &format!("bob {i}")).unwrap()
        } else {
            alice.set(&key, &format!("alice {i}")).unwrap()
        };
        if i == 200 {
            bound = Some(hash);
        }
        if i % 50 == 49 {
            alice.merge(&mut bob).unwrap();
            bob.merge(&mut alice).unwrap();
        }
    }
    alice.merge(&mut bob).unwrap();
    for author in [bob.author(), alice.author()] {
        permissions.insert(author, WritePermission::Bounded(vec![bound.unwrap()]));
        alice.set_write_permissions(permissions.clone()).unwrap();
        let incremental = alice.values().clone();
        let cursor = alice.checkpoint().clone();
        assert!(!incremental.is_empty());
        alice.rematerialize().unwrap();
        assert_eq!(alice.values(), &incremental);
        assert_eq!(alice.checkpoint(), &cursor);
    }

    // A persisted cursor round-trips and has nothing left to apply.
    let cursor = alice.checkpoint();
    let restored = stockfoil::Checkpoint::from_bytes(&cursor.to_bytes()).unwrap();
    assert_eq!(&restored, cursor);
    let mut planner = stockfoil::Materialization::resume(restored, true);
    assert!(
        planner
            .plan(alice.document().ledger(), 64)
            .unwrap()
            .is_none()
    );
}

#[test]
fn fragments_materialize_and_relay_from_stored_members() {
    let mut writer = MvRegisterMap::new();
    for i in 0..300 {
        writer
            .set(&format!("key{}", i % 7), &format!("value {i}"))
            .unwrap();
    }
    let value = checkpoint_value(&writer, "key0");
    writer.set("key0", &value).unwrap();
    let (plan, items) = writer.export_items(&Default::default()).unwrap();
    let fragment = (0..plan.len())
        .find_map(|i| match plan.id(i) {
            Some(ItemId::Fragment(range)) => Some(range.clone()),
            _ => None,
        })
        .expect("a fragment item");
    assert!(fragment.boundary.is_empty() || plan.len() > 1);

    let permissions = writer.write_permissions().clone();
    let mut reader = MvRegisterMap::new();
    reader.set_write_permissions(permissions.clone()).unwrap();
    for item in &items {
        reader.receive(item).unwrap();
    }
    assert_eq!(reader.waiting(), 0);
    assert_eq!(reader.values(), writer.values());
    assert_eq!(reader.get("key0"), values(&value));

    // The reader's payloads are members of stored items; its encoder decodes
    // them again to relay them, and the third replica materializes cold.
    let mut third = MvRegisterMap::new();
    third.set_write_permissions(permissions).unwrap();
    reader.sync_to(&mut third).unwrap();
    assert_eq!(third.values(), writer.values());
    assert_eq!(third.document().history().len(), 301);
}
