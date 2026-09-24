//! Signing and verified-receipt fixtures shared by the integration tests.

// Each integration test compiles its own copy and uses a different subset.
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{
    Author, CommitGraph, CommitHash, CommitMeta, FragmentRange, Signature, SignatureEvidence,
    UnverifiedItem,
};

/// Receive a signed fixture in its canonical item through verification and admission.
/// Parents must already be verified in the graph; callers buffer early arrivals.
pub fn receive_commit<P: Clone>(graph: &mut CommitGraph<P>, meta: CommitMeta, payload: P) {
    let hash = *meta.hash();
    let item = received_item(graph, meta)
        .verify()
        .expect("received fixture must verify");
    let mut payload = Some(payload);
    graph
        .ingest(item, |meta| {
            assert_eq!(
                *meta.hash(),
                hash,
                "fragment ancestors must already be present"
            );
            payload
                .take()
                .expect("single-commit payload requested twice")
        })
        .expect("verified fixture must be admitted");
}

/// Build the canonical transport envelope, including checkpoint ancestors when
/// needed. This only reads destination metadata: receipt still verifies and
/// admits the signed envelope through the public trust boundary.
pub fn received_item<P: Clone>(graph: &CommitGraph<P>, meta: CommitMeta) -> UnverifiedItem {
    let depth = |hash: &CommitHash| hash.as_bytes().iter().take_while(|b| **b == 0).count();
    let head = *meta.hash();
    let head_depth = depth(&head);
    if head_depth == 0 {
        let signatures = received_signatures(&meta);
        return UnverifiedItem::Commit { meta, signatures };
    }
    let mut known = BTreeMap::new();
    graph.traverse(|meta, _| {
        known.insert(*meta.hash(), meta.clone());
        ControlFlow::Continue(())
    });
    known.insert(head, meta);
    let mut pending = vec![head];
    let mut visited = BTreeSet::new();
    let mut boundary = BTreeSet::new();
    let mut commits = Vec::new();
    let mut signatures = Vec::new();
    while let Some(hash) = pending.pop() {
        if !visited.insert(hash) {
            continue;
        }
        let meta = known
            .get(&hash)
            .expect("fixture fragment ancestor must be delivered");
        if hash != head && depth(&hash) >= head_depth {
            boundary.insert(hash);
        } else {
            pending.extend(meta.parents().iter().copied());
            signatures.extend(received_signatures(meta));
        }
        commits.push(meta.clone());
    }
    UnverifiedItem::Fragment {
        range: FragmentRange { head, boundary },
        commits,
        signatures,
    }
}

fn received_signatures(meta: &CommitMeta) -> Vec<SignatureEvidence> {
    let key = (0..=255)
        .map(key)
        .find(|key| &author(key) == meta.author())
        .expect("received fixture author must use a repeated-byte signing key (0..=255)");
    vec![SignatureEvidence {
        author: author(&key),
        commit: *meta.hash(),
        signature: signature(&key, *meta.hash()),
    }]
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
