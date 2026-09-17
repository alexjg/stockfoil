//! Local edits don't wait for signing; publication does.

use std::collections::{BTreeSet, HashMap};

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{Author, CommitHash, Signature, WritePermission};

use super::MvRegisterMap;

fn sign(key: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(key.sign(hash.as_bytes()).to_bytes())
}

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

#[test]
fn incoming_own_author_claims_are_not_local_and_signing_does_not_grant_access() {
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let author = Author::try_from(signing_key.verifying_key().as_bytes().as_slice()).unwrap();
    let mut local = MvRegisterMap::with_author(author);
    let hash = local.prepare_set("title", "pending");
    let working = local.pov();
    let fork = local.fork();
    assert!(fork.get("title").is_empty());
    assert!(fork.export_assignments().is_empty());
    assert!(fork.get_at(&working, "title").is_err());

    local.set_write_permissions(&HashMap::new());
    assert!(local.get("title").is_empty());
    assert!(local.export_assignments().is_empty());
    let signature = sign(&signing_key, &hash);
    local.ingest_signature(author, hash, signature).unwrap();
    assert!(local.get("title").is_empty());
    assert_eq!(local.get_at(&working, "title").unwrap(), values("pending"));

    // Receive the published assignment without its signature, even claiming
    // our own identity. It must not be mistaken for a local unsigned edit.
    let mut peer = MvRegisterMap::with_author(author);
    let mut messages = local.export_assignments();
    assert_eq!(messages.len(), 1);
    peer.receive_assignment(messages.pop().unwrap()).unwrap();
    assert!(peer.get("title").is_empty());
    assert!(peer.get_at(&working, "title").is_err());
    peer.ingest_signature(author, hash, signature).unwrap();
    // Signature delivery alone reveals the value; no assignment is redelivered.
    assert_eq!(peer.get("title"), values("pending"));
    assert_eq!(peer.get_at(&working, "title").unwrap(), values("pending"));

    let fork = local.fork();
    assert!(fork.get("title").is_empty()); // Copies the revoked permissions.
    assert_eq!(fork.get_at(&working, "title").unwrap(), values("pending"));
    peer.merge(&local);
    // Merge transfers evidence, not the sender's revoked permissions.
    assert_eq!(peer.get("title"), values("pending"));
}

#[test]
fn received_payload_must_match_the_signed_metadata_before_ingestion() {
    let key = SigningKey::from_bytes(&[8; 32]);
    let author = Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
    let mut source = MvRegisterMap::with_author(author);
    let hash = source.prepare_set("title", "legitimate");
    let signature = sign(&key, &hash);
    source.ingest_signature(author, hash, signature).unwrap();
    let mut peer = MvRegisterMap::with_author(author);
    peer.ingest_signature(author, hash, signature).unwrap();

    let message = source.export_assignments().pop().unwrap();
    let mut substituted = message.clone();
    // Change a byte of the value while keeping the assignment encoding valid.
    *substituted.payload.last_mut().unwrap() ^= 1;
    assert!(peer.receive_assignment(substituted).is_err());
    assert!(peer.get("title").is_empty());
    assert!(peer.export_assignments().is_empty());
    peer.receive_assignment(message).unwrap();
    assert_eq!(peer.get("title"), values("legitimate"));
}

#[test]
fn local_edits_appear_before_signing_and_a_frontier_signature_publishes_the_batch() {
    // Fixed seed for reproducibility. Sign actual commit hashes, then hold back
    // delivery to simulate the application's asynchronous signer.
    let signing_key = SigningKey::from_bytes(&[42; 32]);
    let author = Author::try_from(signing_key.verifying_key().as_bytes().as_slice()).unwrap();
    let mut local = MvRegisterMap::with_author(author);
    let mut peer = MvRegisterMap::new();
    let permissions = HashMap::from([(author, WritePermission::Unbounded)]);
    local.set_write_permissions(&permissions);
    peer.set_write_permissions(&permissions);

    let base = local.prepare_set("title", "before");
    let base_signature = sign(&signing_key, &base);
    assert_eq!(local.get("title"), values("before"));
    assert!(local.export_assignments().is_empty());
    assert!(
        local
            .get_at(&local.signed_pov(), "title")
            .unwrap()
            .is_empty()
    );
    local
        .ingest_signature(author, base, base_signature)
        .unwrap();
    let initial_working = local.pov();
    let initial_signed = local.signed_pov();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));

    // First edit: visible locally, but no signature has arrived.
    let first = local.prepare_set("title", "pending");
    let first_working = local.pov();
    let first_signed = local.signed_pov();
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
        local.get_at(&local.signed_pov(), "title").unwrap(),
        values("before")
    );
    assert_eq!(local.export_assignments().len(), 1);

    // Another keystroke overwrites the pending value. Neither edit may leak.
    let tip = local.prepare_set("title", "after");
    let tip_signature = sign(&signing_key, &tip);
    let batch_working = local.pov();
    let batch_signed = local.signed_pov();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&batch_signed, "title").unwrap(),
        values("before")
    );
    assert_eq!(local.export_assignments().len(), 1);
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));
    assert!(peer.get_at(&batch_working, "title").is_err());

    // Only the tip is signed: it authenticates both pending edits.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    let published_signed = local.signed_pov();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&published_signed, "title").unwrap(),
        values("after")
    );
    assert_eq!(local.export_assignments().len(), 3);

    // Old document views retain their values, including the overwritten draft.
    for pov in [
        &initial_working,
        &initial_signed,
        &first_signed,
        &batch_signed,
    ] {
        assert_eq!(local.get_at(pov, "title").unwrap(), values("before"));
    }
    assert_eq!(
        local.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        local.get_at(&batch_working, "title").unwrap(),
        values("after")
    );

    // Another replica can now read the formerly unsigned historical views.
    peer.merge(&local);
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

    // Retrying the signing response and merge is harmless.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(local.export_assignments().len(), 3);
    assert_eq!(
        local.get_at(&local.signed_pov(), "title").unwrap(),
        values("after")
    );
}
