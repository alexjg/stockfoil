//! Drafts are local until signed; signing admits them for export.
use std::collections::{BTreeSet, HashMap};
use stockfoil::{Heads, WritePermission};
use stockfoil_mv_register::MvRegisterMap;

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn drafts_stay_local_until_published_and_historical_reads_survive_relay() {
    let mut local = MvRegisterMap::new();
    local.set("title", "before").unwrap();
    let before = local.view_point();
    let mut peer = local.fork().unwrap();

    local.prepare_set("title", "draft").unwrap();
    let draft = local.view_point();
    local.prepare_set("title", "after").unwrap();
    let after = local.view_point();
    assert_eq!(local.document().history().drafts().count(), 2);
    assert_eq!(local.document().history().heads(Heads::Admitted).count(), 1);
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(draft.heads(), "title").unwrap(),
        values("draft")
    );
    peer.merge(&mut local).unwrap();
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(draft.heads(), "title").is_err());
    assert!(peer.get_at(after.heads(), "title").is_err());
    assert_eq!(local.fork().unwrap().get("title"), values("before"));

    // Signing does not restore revoked permission, nor does merge copy it.
    local.set_write_permissions(HashMap::new()).unwrap();
    local.publish().unwrap();
    assert_eq!(local.document().history().drafts().count(), 0);
    assert!(local.get("title").is_empty());
    peer.merge(&mut local).unwrap();
    let mut relay = peer.fork().unwrap();
    // Heads alone do not override the document's policy: local still denies
    // its author, while the peer and relay continue to permit those writes.
    assert!(local.get_at(before.heads(), "title").unwrap().is_empty());
    for doc in [&mut peer, &mut relay] {
        assert_eq!(
            doc.get_at(before.heads(), "title").unwrap(),
            values("before")
        );
        assert_eq!(doc.get_at(draft.heads(), "title").unwrap(), values("draft"));
        assert_eq!(doc.get_at(after.heads(), "title").unwrap(), values("after"));
    }
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(relay.get("title"), values("after"));

    local.publish().unwrap(); // Idempotent.
    peer.merge(&mut local).unwrap();
    assert_eq!(peer.get("title"), values("after"));
    local
        .set_write_permissions(HashMap::from([(
            local.author(),
            WritePermission::Unbounded,
        )]))
        .unwrap();
    assert_eq!(local.get("title"), values("after"));
}

#[test]
fn tampered_signatures_and_payloads_are_rejected() {
    let mut writer = MvRegisterMap::new();
    writer.set("title", "signed").unwrap();
    let (_, items) = writer.export_items(&Default::default()).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    let value = item.windows(6).position(|w| w == b"signed").unwrap();
    // Body: 0x00 uleb(5) "title" uleb(6) "signed". The header ends with the signature.
    let signature = value - 8 - 1;
    for offset in [value, signature, value - 1] {
        let mut tampered = item.clone();
        tampered[offset] ^= 1;
        let mut reader = MvRegisterMap::new();
        assert!(reader.receive(&tampered).is_err(), "byte {offset}");
        assert!(reader.document().history().is_empty());
    }
    let mut reader = MvRegisterMap::new();
    reader.receive(item).unwrap();
    assert_eq!(reader.document().history().len(), 1);
}
