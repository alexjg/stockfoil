//! Signing fixtures shared by the integration tests.

// Each integration test compiles its own copy and uses a different subset.
#![allow(dead_code)]

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{Author, CommitGraph, CommitHash, Signature};

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
