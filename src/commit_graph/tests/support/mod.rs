//! Signing and verified-receipt fixtures shared by internal graph regressions.

// Some helpers are also useful to individual regression suites in isolation.
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use super::CommitGraph;
use crate::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, FragmentRange,
    Signature, SignatureEvidence, UnverifiedItem,
};

/// Receive a signed fixture in its canonical item through verification and admission.
/// Parents must already be admitted: this fixture builds fragments from destination metadata.
pub fn receive_commit<P: Clone>(graph: &mut CommitGraph<P>, meta: CommitMeta, payload: P) {
    let hash = *meta.hash();
    let item = received_item(graph, meta)
        .verify()
        .expect("received fixture must verify");
    let mut payload = Some(payload);
    graph
        .ingest_verified(item, |meta| {
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
/// admits the signed envelope through private structured admission.
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
        }
    }
    // A bypass edge can reach behind a boundary. Subtract its entire inclusive
    // history, not just the nodes at which the raw discovery walk stopped.
    let mut excluded = BTreeSet::new();
    let mut pending: Vec<_> = boundary.iter().copied().collect();
    while let Some(hash) = pending.pop() {
        if excluded.insert(hash) {
            pending.extend(known[&hash].parents().iter().copied());
        }
    }
    let members: BTreeSet<_> = visited.difference(&excluded).copied().collect();
    let commits: Vec<_> = members.iter().map(|hash| known[hash].clone()).collect();
    let signatures = commits.iter().flat_map(received_signatures).collect();
    UnverifiedItem::Fragment {
        range: FragmentRange { head, boundary },
        checkpoints: BTreeSet::new(),
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

pub fn seeded_author(seed: u8) -> Author {
    author(&key(seed))
}

/// Mine deterministic metadata at an exact leading-zero-byte depth.
pub fn at_depth(seed: u8, parents: impl AsRef<[CommitHash]>, depth: usize) -> CommitMeta {
    let author = seeded_author(seed);
    (0u64..10_000_000)
        .map(|nonce| {
            CommitMeta::new(CommitContents {
                author,
                parents: parents.as_ref().to_vec(),
                payload: BlobHash::of_contents(&nonce.to_le_bytes()),
            })
        })
        .find(|meta| {
            meta.hash()
                .as_bytes()
                .iter()
                .take_while(|byte| **byte == 0)
                .count()
                == depth
        })
        .expect("find a real hash at the requested depth")
}

pub fn proof(seed: u8, hash: CommitHash) -> SignatureEvidence {
    let key = key(seed);
    SignatureEvidence {
        author: author(&key),
        commit: hash,
        signature: signature(&key, hash),
    }
}

/// A standalone announcement, even at a checkpoint; never invents a fragment certificate.
pub fn signed_commit(meta: &CommitMeta, seed: u8) -> UnverifiedItem {
    UnverifiedItem::Commit {
        meta: meta.clone(),
        signatures: vec![proof(seed, *meta.hash())],
    }
}

/// Sign the supplied range's head without normalizing or filtering its members.
/// Malformed fragment fixtures must remain malformed.
pub fn signed_fragment(
    range: &FragmentRange,
    commits: Vec<CommitMeta>,
    seed: u8,
) -> UnverifiedItem {
    UnverifiedItem::Fragment {
        range: range.clone(),
        checkpoints: BTreeSet::new(),
        commits,
        signatures: vec![proof(seed, range.head)],
    }
}

pub fn sign<P: Clone>(graph: &mut CommitGraph<P>, key: &SigningKey, hash: CommitHash) {
    graph
        .ingest_signature(author(key), hash, signature(key, hash))
        .unwrap();
}
