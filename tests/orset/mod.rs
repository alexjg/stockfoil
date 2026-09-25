//! Delivery-only observed-remove set. The graph supplies causal ordering and
//! deduplication, so the materialized state only needs the surviving add IDs.
use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use stockfoil::{Author, CommitGraph, CommitHash, Signature};

pub struct OrSet {
    graph: CommitGraph<Op>,
    adds: HashMap<String, BTreeSet<CommitHash>>,
    signing_key: SigningKey,
}

#[derive(Clone, Serialize)]
enum Op {
    Add(String),
    Remove {
        value: String,
        observed: Vec<[u8; 32]>,
    },
}

impl OrSet {
    pub fn new() -> Self {
        Self {
            graph: CommitGraph::new(),
            adds: HashMap::new(),
            signing_key: SigningKey::from_bytes(&rand::random()),
        }
    }

    fn author(&self) -> Author {
        Author::try_from(self.signing_key.verifying_key().as_bytes().as_slice()).unwrap()
    }

    pub fn add(&mut self, value: String) {
        self.commit(Op::Add(value));
    }

    pub fn remove(&mut self, value: String) {
        let observed: Vec<_> = self
            .adds
            .get(&value)
            .into_iter()
            .flatten()
            .map(|h| *h.as_bytes())
            .collect();
        if !observed.is_empty() {
            self.commit(Op::Remove { value, observed });
        }
    }

    fn commit(&mut self, op: Op) {
        let before: Vec<_> = self.graph.heads().collect();
        let meta = self
            .graph
            .commit_by(self.author(), &bcs::to_bytes(&op).unwrap());
        let hash = *meta.hash();
        let signature = Signature::from(self.signing_key.sign(hash.as_bytes()).to_bytes());
        self.graph.ingest_local_commit(meta, op);
        self.graph
            .ingest_signature(self.author(), hash, signature)
            .unwrap();
        self.evaluate(&before);
    }

    pub fn fork(&self) -> Self {
        let mut fork = Self::new();
        fork.merge(self);
        fork
    }

    pub fn merge(&mut self, other: &Self) {
        let before: Vec<_> = self.graph.heads().collect();
        self.graph.merge(&other.graph);
        self.evaluate(&before);
    }

    fn evaluate(&mut self, since: &[CommitHash]) {
        self.graph.traverse_since(since, |meta, op| {
            match op {
                Op::Add(value) => {
                    self.adds
                        .entry(value.clone())
                        .or_default()
                        .insert(*meta.hash());
                }
                Op::Remove { value, observed } => {
                    if let Some(adds) = self.adds.get_mut(value) {
                        adds.retain(|hash| !observed.contains(hash.as_bytes()));
                    }
                }
            }
            ControlFlow::Continue(())
        });
    }

    pub fn value(&self) -> BTreeSet<String> {
        self.adds
            .iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(value, _)| value.clone())
            .collect()
    }
}

#[test]
fn removes_observed_adds_but_preserves_concurrent_adds() {
    let mut left = OrSet::new();
    left.remove("absent".into());
    left.add("old".into());
    left.remove("old".into());
    assert!(left.value().is_empty());
    let mut right = left.fork();
    assert_ne!(left.author(), right.author());
    left.add("shared".into());
    right.add("shared".into());
    left.remove("shared".into());

    right.merge(&left);
    left.merge(&right);
    left.merge(&right);
    assert_eq!(left.value(), BTreeSet::from(["shared".into()]));
    assert_eq!(right.value(), left.value());

    right.remove("shared".into());
    left.merge(&right);
    right.merge(&left);
    assert!(left.value().is_empty());
    assert!(right.value().is_empty());
    // Replaying old history cannot resurrect an observed, removed add.
    assert!(left.fork().value().is_empty());
}
