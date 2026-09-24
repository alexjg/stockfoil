//! Local edits don't wait for signing; merging them does.

use std::collections::{BTreeSet, HashMap};

use crate::support::{author, key, signature};
use stockfoil::{BlobHash, CommitContents, CommitMeta, WritePermission};

use super::MvRegisterMap;

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn own_author_merges_are_not_local_and_signing_does_not_grant_access() {
    let signing_key = key(7);
    let author = author(&signing_key);
    let mut local = MvRegisterMap::with_author(author);
    let hash = local.prepare_set("title", "pending");
    let working = local.view_point();
    let fork = local.fork();
    assert!(fork.get("title").is_empty());
    assert!(fork.get_at(&working, "title").is_err());

    local.set_write_permissions(&HashMap::new());
    assert!(local.get("title").is_empty());
    // Matching the local author does not make somebody else's unsigned work local.
    let mut peer = MvRegisterMap::with_author(author);
    peer.set_write_permissions(&HashMap::new());
    peer.merge(&local);
    assert!(peer.get("title").is_empty());
    assert!(peer.get_at(&working, "title").is_err());

    local
        .ingest_signature(author, hash, signature(&signing_key, hash))
        .unwrap();
    assert!(local.get("title").is_empty());
    assert_eq!(local.get_at(&working, "title").unwrap(), values("pending"));
    peer.merge(&local);
    assert!(peer.get("title").is_empty());
    assert_eq!(peer.get_at(&working, "title").unwrap(), values("pending"));
    // Permission restoration reveals authenticated history without replay.
    peer.set_write_permissions(&HashMap::from([(author, WritePermission::Unbounded)]));
    assert_eq!(peer.get("title"), values("pending"));

    let fork = local.fork();
    assert!(fork.get("title").is_empty()); // Copies the revoked permissions.
    assert_eq!(fork.get_at(&working, "title").unwrap(), values("pending"));
    peer.merge(&local);
    // Merge transfers history, not the sender's revoked permissions.
    assert_eq!(peer.get("title"), values("pending"));
}

#[test]
fn a_signed_nonfragment_tip_waits_for_earlier_edits() {
    let signing_key = key(10);
    let author = author(&signing_key);
    let mut source = MvRegisterMap::with_author(author);
    let first = source.prepare_set("title", "first");
    let middle = source.prepare_set("title", "middle");
    let middle_view = source.view_point();
    let tip = source.prepare_set("title", "last");
    for hash in [first, middle, tip] {
        assert_ne!(hash.as_bytes()[0], 0); // No checkpoint packages the batch.
    }
    source
        .ingest_signature(author, tip, signature(&signing_key, tip))
        .unwrap();
    assert_eq!(source.get("title"), values("last"));
    assert!(
        source
            .get_at(&source.signed_view_point(), "title")
            .unwrap()
            .is_empty()
    );
    let mut relay = MvRegisterMap::with_author(author);
    relay.merge(&source);
    assert!(relay.get("title").is_empty());

    source
        .ingest_signature(author, first, signature(&signing_key, first))
        .unwrap();
    relay.merge(&source);
    assert_eq!(relay.get("title"), values("first"));
    let mut peer = MvRegisterMap::with_author(author);
    peer.merge(&relay);
    assert_eq!(peer.get("title"), values("first"));
    assert!(peer.get_at(&middle_view, "title").is_err());

    source
        .ingest_signature(author, middle, signature(&signing_key, middle))
        .unwrap();
    relay.merge(&source);
    peer.merge(&relay);
    assert_eq!(peer.get("title"), values("last"));
    assert_eq!(
        peer.get_at(&middle_view, "title").unwrap(),
        values("middle")
    );
}

#[test]
fn signing_a_checkpoint_merges_the_new_value_without_losing_history() {
    let signing_key = key(10);
    let author = author(&signing_key);
    let mut source = MvRegisterMap::with_author(author);
    let first = source.prepare_set("title", "first");
    assert_ne!(first.as_bytes()[0], 0);
    source
        .ingest_signature(author, first, signature(&signing_key, first))
        .unwrap();
    let old_view = source.view_point();

    // Mine just the next payload, without inserting trial edits into the graph.
    let (value, checkpoint) = (0..)
        .find_map(|nonce| {
            let value = format!("checkpoint-{nonce}");
            let bytes = bcs::to_bytes(&(*author.as_array(), "title", &value)).unwrap();
            let meta = CommitMeta::new(CommitContents {
                parents: vec![first],
                author,
                payload: BlobHash::of_contents(&bytes),
            });
            (meta.hash().as_bytes()[0] == 0).then_some((value, *meta.hash()))
        })
        .unwrap();
    assert_eq!(source.prepare_set("title", &value), checkpoint);
    let pending_view = source.view_point();
    // The unsigned checkpoint neither leaks nor hides the signed first edit.
    let mut peer = MvRegisterMap::with_author(author);
    peer.merge(&source);
    assert_eq!(peer.get("title"), values("first"));
    assert!(peer.get_at(&pending_view, "title").is_err());

    source
        .ingest_signature(author, checkpoint, signature(&signing_key, checkpoint))
        .unwrap();
    peer.merge(&source);
    assert_eq!(peer.get("title"), values(&value));
    let mut fresh = MvRegisterMap::with_author(author);
    fresh.merge(&peer);
    assert_eq!(fresh.get("title"), values(&value));
    assert_eq!(fresh.get_at(&old_view, "title").unwrap(), values("first"));
}

#[test]
fn publish_makes_all_pending_assignments_mergeable() {
    let mut local = MvRegisterMap::new();
    let mut views = Vec::new();
    for value in ["first", "middle", "last"] {
        local.prepare_set("title", value);
        views.push((local.view_point(), value));
    }
    assert_eq!(local.get("title"), values("last"));
    let mut peer = MvRegisterMap::with_author(local.author());
    peer.merge(&local);
    assert!(peer.get("title").is_empty());
    local.publish();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("last"));
    for (view, value) in views {
        assert_eq!(peer.get_at(&view, "title").unwrap(), values(value));
    }
    let published = local.signed_view_point();
    local.publish();
    assert_eq!(local.signed_view_point(), published);
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("last"));
}

#[test]
fn local_edits_appear_before_signing_and_direct_signatures_publish_the_batch() {
    let signing_key = key(42);
    let author = author(&signing_key);
    let mut local = MvRegisterMap::with_author(author);
    let mut peer = MvRegisterMap::with_author(author);

    let base = local.prepare_set("title", "before");
    let base_signature = signature(&signing_key, base);
    assert_eq!(local.get("title"), values("before"));
    assert!(
        local
            .get_at(&local.signed_view_point(), "title")
            .unwrap()
            .is_empty()
    );
    peer.merge(&local);
    assert!(peer.get("title").is_empty());
    local
        .ingest_signature(author, base, base_signature)
        .unwrap();
    let initial_working = local.view_point();
    let initial_signed = local.signed_view_point();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));

    let first = local.prepare_set("title", "pending");
    let first_working = local.view_point();
    let first_signed = local.signed_view_point();
    assert_eq!(local.get("title"), values("pending"));
    assert_eq!(
        local.get_at(&first_signed, "title").unwrap(),
        values("before")
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(&first_working, "title").is_err());

    // A valid signature for the wrong hash must not satisfy the request.
    assert!(
        local
            .ingest_signature(author, first, base_signature)
            .is_err()
    );
    assert_eq!(local.get("title"), values("pending"));
    assert_eq!(
        local.get_at(&local.signed_view_point(), "title").unwrap(),
        values("before")
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));

    let tip = local.prepare_set("title", "after");
    let tip_signature = signature(&signing_key, tip);
    for hash in [base, first, tip] {
        assert_ne!(hash.as_bytes()[0], 0); // This regression uses loose edits.
    }
    let batch_working = local.view_point();
    let batch_signed = local.signed_view_point();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&batch_signed, "title").unwrap(),
        values("before")
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(&batch_working, "title").is_err());

    // Signing the loose tip alone doesn't admit its unsigned prerequisite.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    let published_signed = local.signed_view_point();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&published_signed, "title").unwrap(),
        values("before")
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    local
        .ingest_signature(author, first, signature(&signing_key, first))
        .unwrap();
    assert_ne!(local.signed_view_point(), published_signed);

    // Captured views retain their values, including the overwritten draft.
    for view in [
        &initial_working,
        &initial_signed,
        &first_signed,
        &batch_signed,
    ] {
        assert_eq!(local.get_at(view, "title").unwrap(), values("before"));
    }
    assert_eq!(
        local.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        local.get_at(&batch_working, "title").unwrap(),
        values("after")
    );

    peer.merge(&local);
    let mut relay_peer = MvRegisterMap::with_author(author);
    relay_peer.merge(&peer);
    for doc in [&peer, &relay_peer] {
        assert_eq!(doc.get("title"), values("after"));
        assert_eq!(
            doc.get_at(&first_working, "title").unwrap(),
            values("pending")
        );
        assert_eq!(
            doc.get_at(&batch_working, "title").unwrap(),
            values("after")
        );
        assert_eq!(
            doc.get_at(&batch_signed, "title").unwrap(),
            values("before")
        );
    }

    // A stale signing response is rejected; repeated merges remain idempotent.
    assert_eq!(
        local.ingest_signature(author, tip, tip_signature),
        Err(stockfoil::SignatureError::NotPendingLocalCommit(tip))
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(
        local.get_at(&local.signed_view_point(), "title").unwrap(),
        values("after")
    );
}
