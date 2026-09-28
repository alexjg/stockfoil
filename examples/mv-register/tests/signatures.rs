//! One document-level signing flow; signature validation belongs in graph tests.
use std::collections::{BTreeSet, HashMap};

use stockfoil::WritePermission;
use stockfoil_mv_register::MvRegisterMap;

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn drafts_stay_local_until_published_and_historical_reads_survive_relay() {
    let mut local = MvRegisterMap::new();
    local.set("title", "before");
    let before = local.view_point();
    let mut peer = local.fork();

    local.prepare_set("title", "draft");
    let draft = local.view_point();
    local.prepare_set("title", "after");
    let after = local.view_point();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(local.get_at(&draft, "title").unwrap(), values("draft"));
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(&draft, "title").is_err());
    assert!(peer.get_at(&after, "title").is_err());
    assert_eq!(local.fork().get("title"), values("before"));

    // Signing does not restore revoked permission, nor does merge copy it.
    local.set_write_permissions(&HashMap::new());
    local.publish();
    assert!(local.get("title").is_empty());
    peer.merge(&local);
    let relay = peer.fork();
    for doc in [&local, &peer, &relay] {
        assert_eq!(doc.get_at(&before, "title").unwrap(), values("before"));
        assert_eq!(doc.get_at(&draft, "title").unwrap(), values("draft"));
        assert_eq!(doc.get_at(&after, "title").unwrap(), values("after"));
    }
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(relay.get("title"), values("after"));

    local.publish(); // Idempotent even if a fragment signed several edits at once.
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    local.set_write_permissions(&HashMap::from([(
        local.author(),
        WritePermission::Unbounded,
    )]));
    assert_eq!(local.get("title"), values("after"));
}
