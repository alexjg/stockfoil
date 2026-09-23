//! Local edits don't wait for signing; publication does.

use std::collections::{BTreeSet, HashMap};

use crate::support::{author, key, signature};
use stockfoil::{
    BlobHash, CommitContents, CommitMeta, SignatureEvidence, UnverifiedItem, WritePermission,
};

use super::MvRegisterMap;

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn authenticated_own_author_receipts_are_not_local_and_signing_does_not_grant_access() {
    let signing_key = key(7);
    let author = author(&signing_key);
    let mut local = MvRegisterMap::with_author(author);
    let hash = local.prepare_set("title", "pending");
    let working = local.view_point();
    let fork = local.fork();
    assert!(fork.get("title").is_empty());
    assert!(fork.export_assignments().is_empty());
    assert!(fork.get_at(&working, "title").is_err());

    local.set_write_permissions(&HashMap::new());
    assert!(local.get("title").is_empty());
    assert!(local.export_assignments().is_empty());
    let signature = signature(&signing_key, hash);
    local.ingest_signature(author, hash, signature).unwrap();
    assert!(local.get("title").is_empty());
    assert_eq!(local.get_at(&working, "title").unwrap(), values("pending"));

    // Even an own-author claim needs genuine evidence; it cannot enter through
    // optimistic local admission. Authentication also does not grant permission.
    let mut peer = MvRegisterMap::with_author(author);
    peer.set_write_permissions(&HashMap::new());
    let mut messages = local.export_assignments();
    assert_eq!(messages.len(), 1);
    let message = messages.pop().unwrap();
    let mut unsigned = message.clone();
    match &mut unsigned.item {
        UnverifiedItem::Commit { signatures, .. } | UnverifiedItem::Fragment { signatures, .. } => {
            signatures.clear()
        }
    }
    assert!(peer.receive_assignment(unsigned).is_err());
    assert!(peer.export_assignments().is_empty());
    assert!(peer.get_at(&working, "title").is_err());

    peer.receive_assignment(message).unwrap();
    assert!(peer.get("title").is_empty());
    assert_eq!(peer.get_at(&working, "title").unwrap(), values("pending"));
    // Permission restoration reveals already authenticated history, no replay.
    peer.set_write_permissions(&HashMap::from([(author, WritePermission::Unbounded)]));
    assert_eq!(peer.get("title"), values("pending"));

    let fork = local.fork();
    assert!(fork.get("title").is_empty()); // Copies the revoked permissions.
    assert_eq!(fork.get_at(&working, "title").unwrap(), values("pending"));
    peer.merge(&local);
    // Merge transfers evidence, not the sender's revoked permissions.
    assert_eq!(peer.get("title"), values("pending"));
}

#[test]
fn received_payload_must_match_the_signed_metadata_before_ingestion() {
    let key = key(8);
    let author = author(&key);
    let mut source = MvRegisterMap::with_author(author);
    let hash = source.prepare_set("title", "legitimate");
    let signature = signature(&key, hash);
    source.ingest_signature(author, hash, signature).unwrap();
    let mut peer = MvRegisterMap::with_author(author);

    let message = source.export_assignments().pop().unwrap();
    let mut substituted = message.clone();
    // Change a byte of the value while keeping the assignment encoding valid.
    *substituted
        .payloads
        .values_mut()
        .next()
        .unwrap()
        .1
        .last_mut()
        .unwrap() ^= 1;
    assert!(peer.receive_assignment(substituted).is_err());
    assert!(peer.get("title").is_empty());
    assert!(peer.export_assignments().is_empty());
    peer.receive_assignment(message).unwrap();
    assert_eq!(peer.get("title"), values("legitimate"));
}

#[test]
fn payload_and_metadata_cannot_be_paired_with_another_verified_item() {
    let signing_key = key(9);
    let author = author(&signing_key);
    let mut source = MvRegisterMap::with_author(author);
    let hash = source.prepare_set("title", "legitimate");
    source
        .ingest_signature(author, hash, signature(&signing_key, hash))
        .unwrap();
    let mut other = MvRegisterMap::with_author(author);
    let other_hash = other.prepare_set("title", "different");
    other
        .ingest_signature(author, other_hash, signature(&signing_key, other_hash))
        .unwrap();

    let message = source.export_assignments().pop().unwrap();
    let mut substituted = message.clone();
    substituted.item = other.export_assignments().pop().unwrap().item;
    let mut peer = MvRegisterMap::with_author(author);
    assert!(peer.receive_assignment(substituted).is_err());
    assert!(peer.get("title").is_empty());
    assert!(peer.export_assignments().is_empty());
    peer.receive_assignment(message).unwrap();
    assert_eq!(peer.get("title"), values("legitimate"));
}

#[test]
fn a_signed_nonfragment_batch_is_withheld_until_each_edit_is_directly_signed() {
    let signing_key = key(10);
    let author = author(&signing_key);
    let mut source = MvRegisterMap::with_author(author);
    let first = source.prepare_set("title", "first");
    let middle = source.prepare_set("title", "middle");
    let tip = source.prepare_set("title", "last");
    for hash in [first, middle, tip] {
        assert_ne!(hash.as_bytes()[0], 0); // No checkpoint fragment packages the batch.
    }
    source
        .ingest_signature(author, tip, signature(&signing_key, tip))
        .unwrap();

    assert_eq!(source.get("title"), values("last"));
    assert_eq!(
        source.get_at(&source.signed_view_point(), "title").unwrap(),
        BTreeSet::new()
    );
    assert!(source.export_assignments().is_empty());
    let mut relay = MvRegisterMap::with_author(author);
    relay.merge(&source);
    assert!(relay.get("title").is_empty());

    // These depth-zero edits need direct signatures for the whole cover.
    for hash in [first, middle] {
        source
            .ingest_signature(author, hash, signature(&signing_key, hash))
            .unwrap();
    }
    let messages = source.export_assignments();
    assert_eq!(messages.len(), 3);
    relay.receive_assignment(messages[0].clone()).unwrap();
    assert_eq!(relay.get("title"), values("first"));
    let relayed = relay.export_assignments();
    assert_eq!(relayed.len(), 1); // Only the first assignment has arrived.
    let mut peer = MvRegisterMap::with_author(author);
    peer.receive_assignment(relayed[0].clone()).unwrap();
    assert_eq!(peer.get("title"), values("first"));
    peer.merge(&relay);
    assert_eq!(peer.export_assignments().len(), 1);
    for message in messages.into_iter().skip(1) {
        peer.receive_assignment(message).unwrap();
    }
    assert_eq!(peer.get("title"), values("last"));
}

#[test]
fn checkpoint_messages_replace_old_assignments_and_bind_every_member_payload() {
    let signing_key = key(10);
    let author = author(&signing_key);
    let mut source = MvRegisterMap::with_author(author);
    let first = source.prepare_set("title", "first");
    assert_ne!(first.as_bytes()[0], 0);
    source
        .ingest_signature(author, first, signature(&signing_key, first))
        .unwrap();
    let old = source.export_assignments();
    assert_eq!(old.len(), 1);

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
    // The uncertified replacement does not erase the previously published cover.
    let waiting = source.export_assignments();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].item.id(), old[0].item.id());
    source
        .ingest_signature(author, checkpoint, signature(&signing_key, checkpoint))
        .unwrap();
    let mut messages = source.export_assignments();
    assert_eq!(messages.len(), 1);
    let message = messages.pop().unwrap();
    assert!(matches!(&message.item, UnverifiedItem::Fragment { .. }));
    assert_eq!(
        message.payloads.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([first, checkpoint])
    );
    let mut peer = MvRegisterMap::with_author(author);
    let mut corrupt = message.clone();
    *corrupt
        .payloads
        .get_mut(&first)
        .unwrap()
        .1
        .last_mut()
        .unwrap() ^= 1;
    assert!(peer.receive_assignment(corrupt).is_err());
    assert!(peer.export_assignments().is_empty());
    let mut incomplete = message.clone();
    incomplete.payloads.remove(&first);
    assert!(peer.receive_assignment(incomplete).is_err());
    assert!(peer.get("title").is_empty());
    peer.receive_assignment(message).unwrap();
    assert_eq!(peer.get("title"), values(&value));
    let relayed = peer.export_assignments();
    assert_eq!(relayed.len(), 1);
    assert!(matches!(&relayed[0].item, UnverifiedItem::Fragment { .. }));
    let mut fresh = MvRegisterMap::with_author(author);
    fresh.receive_assignment(relayed[0].clone()).unwrap();
    assert_eq!(fresh.get("title"), values(&value));
}

#[test]
fn a_valid_payload_hash_and_signature_do_not_authorize_another_assignment_actor() {
    let signing_key = key(8);
    let signer = author(&signing_key);
    let mut source = MvRegisterMap::with_author(signer);
    let hash = source.prepare_set("title", "legitimate");
    source
        .ingest_signature(signer, hash, signature(&signing_key, hash))
        .unwrap();
    let mut message = source.export_assignments().pop().unwrap();
    let (meta, bytes) = (0..)
        .find_map(|nonce| {
            let bytes = bcs::to_bytes(&(
                *author(&key(9)).as_array(),
                "title",
                format!("forged-{nonce}"),
            ))
            .unwrap();
            let meta = CommitMeta::new(CommitContents {
                parents: vec![],
                author: signer,
                payload: BlobHash::of_contents(&bytes),
            });
            (meta.hash().as_bytes()[0] != 0).then_some((meta, bytes))
        })
        .unwrap();
    message.item = UnverifiedItem::Commit {
        signatures: vec![SignatureEvidence {
            author: signer,
            commit: *meta.hash(),
            signature: signature(&signing_key, *meta.hash()),
        }],
        meta: meta.clone(),
    };
    assert!(message.item.clone().verify().is_ok());
    message.payloads = std::collections::BTreeMap::from([(*meta.hash(), (meta, bytes))]);
    let mut peer = MvRegisterMap::with_author(signer);
    assert_eq!(
        peer.receive_assignment(message),
        Err("assignment actor does not match the commit author")
    );
    assert!(peer.get("title").is_empty());
    assert!(peer.export_assignments().is_empty());
}

#[test]
fn publish_directly_signs_every_pending_assignment() {
    let mut local = MvRegisterMap::new();
    let hashes = [
        local.prepare_set("title", "first"),
        local.prepare_set("title", "middle"),
        local.prepare_set("title", "last"),
    ];
    assert_eq!(local.get("title"), values("last"));
    assert!(local.export_assignments().is_empty());
    local.publish();
    let messages = local.export_assignments();
    let message_count = messages.len();
    let mut received = BTreeSet::new();
    let mut peer = MvRegisterMap::with_author(local.author());
    for message in messages {
        let verified = message.item.clone().verify().unwrap();
        received.extend(verified.commits().map(|meta| *meta.hash()));
        peer.receive_assignment(message).unwrap();
    }
    assert_eq!(received, BTreeSet::from(hashes));
    assert_eq!(peer.get("title"), values("last"));
    let published = local.signed_view_point();
    local.publish();
    assert_eq!(local.signed_view_point(), published);
    assert_eq!(local.export_assignments().len(), message_count);
}

#[test]
fn local_edits_appear_before_signing_and_direct_signatures_publish_the_batch() {
    // Fixed seed for reproducibility. Sign actual commit hashes, then hold back
    // delivery to simulate the application's asynchronous signer.
    let signing_key = key(42);
    let author = author(&signing_key);
    let mut local = MvRegisterMap::with_author(author);
    let mut peer = MvRegisterMap::new();
    let permissions = HashMap::from([(author, WritePermission::Unbounded)]);
    local.set_write_permissions(&permissions);
    peer.set_write_permissions(&permissions);

    let base = local.prepare_set("title", "before");
    let base_signature = signature(&signing_key, base);
    assert_eq!(local.get("title"), values("before"));
    assert!(local.export_assignments().is_empty());
    assert!(
        local
            .get_at(&local.signed_view_point(), "title")
            .unwrap()
            .is_empty()
    );
    local
        .ingest_signature(author, base, base_signature)
        .unwrap();
    let initial_working = local.view_point();
    let initial_signed = local.signed_view_point();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));

    // First edit: visible locally, but no signature has arrived.
    let first = local.prepare_set("title", "pending");
    let first_working = local.view_point();
    let first_signed = local.signed_view_point();
    assert_eq!(local.get("title"), values("pending"));
    assert_eq!(
        local.get_at(&first_signed, "title").unwrap(),
        values("before")
    );
    assert_eq!(local.export_assignments().len(), 1);
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
    assert_eq!(local.export_assignments().len(), 1);

    // Another keystroke overwrites the pending value. Neither edit may leak.
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
    assert_eq!(local.export_assignments().len(), 1);
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(&batch_working, "title").is_err());

    // A loose tip waits for its earlier edit to be admitted. Local working
    // visibility remains optimistic, but signed views contain admitted items.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    let published_signed = local.signed_view_point();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&published_signed, "title").unwrap(),
        values("before")
    );
    assert_eq!(local.export_assignments().len(), 1);
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    local
        .ingest_signature(author, first, signature(&signing_key, first))
        .unwrap();
    assert_eq!(local.export_assignments().len(), 3);
    assert_ne!(local.signed_view_point(), published_signed);

    // Old document views retain their values, including the overwritten draft.
    for view_point in [
        &initial_working,
        &initial_signed,
        &first_signed,
        &batch_signed,
    ] {
        assert_eq!(local.get_at(view_point, "title").unwrap(), values("before"));
    }
    assert_eq!(
        local.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        local.get_at(&batch_working, "title").unwrap(),
        values("after")
    );

    // Each message carries its own signature, so earlier edits can be received
    // before the tip's payload without any out-of-item proof.
    for message in local.export_assignments() {
        peer.receive_assignment(message).unwrap();
    }
    // A relay retains direct signatures and can republish to a fresh peer.
    let mut relay_peer = MvRegisterMap::new();
    relay_peer.set_write_permissions(&permissions);
    for message in peer.export_assignments() {
        relay_peer.receive_assignment(message).unwrap();
    }
    assert_eq!(relay_peer.get("title"), values("after"));
    assert_eq!(
        relay_peer.get_at(&first_working, "title").unwrap(),
        values("pending")
    );

    // Another replica can now read the formerly unsigned historical views.
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(
        peer.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        peer.get_at(&batch_working, "title").unwrap(),
        values("after")
    );
    assert_eq!(
        peer.get_at(&batch_signed, "title").unwrap(),
        values("before")
    );

    // A stale signing response is rejected; repeated merges remain idempotent.
    assert_eq!(
        local.ingest_signature(author, tip, tip_signature),
        Err(stockfoil::SignatureError::NotPendingLocalCommit(tip))
    );
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(local.export_assignments().len(), 3);
    assert_eq!(
        local.get_at(&local.signed_view_point(), "title").unwrap(),
        values("after")
    );
}
