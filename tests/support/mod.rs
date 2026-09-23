//! Signing and verified-receipt fixtures shared by the integration tests.

// Each integration test compiles its own copy and uses a different subset.
#![allow(dead_code)]

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{
    Author, CommitGraph, CommitHash, CommitMeta, Signature, SignatureEvidence, UnverifiedItem,
};

/// Migrate legacy receipts through the public verification/admission boundary.
/// This is compile-only scaffolding until proof fixtures and verification exist.
/// In particular, unsigned receipts and missing-parent queues need behavioral
/// migration; their existing assertions are retained rather than bypassed.
pub fn receive_commit<P: Clone>(graph: &mut CommitGraph<P>, meta: CommitMeta, payload: P) {
    let signatures = received_signatures(&meta);
    let item = UnverifiedItem::Commit { meta, signatures }
        .verify(&*graph)
        .expect("received fixture must verify");
    let mut payload = Some(payload);
    graph
        .ingest(item, |_| {
            payload
                .take()
                .expect("single-commit payload requested twice")
        })
        .expect("verified fixture must be admitted");
}

fn received_signatures(_meta: &CommitMeta) -> Vec<SignatureEvidence> {
    // Signing every receipt with a fixture key would change tests that rely on
    // unsigned receipt or later signature arrival. Adapt those scenarios and
    // their proof dependencies explicitly, rather than inventing evidence.
    todo!("prepare legacy fixture proof evidence and adapt signature-timing tests")
}

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn author(key: &SigningKey) -> Author {
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

pub fn signature(key: &SigningKey, hash: CommitHash) -> Signature {
    Signature::from(key.sign(hash.as_bytes()).to_bytes())
}

pub fn sign<P: Clone>(graph: &mut CommitGraph<P>, key: &SigningKey, hash: CommitHash) {
    graph
        .ingest_signature(author(key), hash, signature(key, hash))
        .unwrap();
}
