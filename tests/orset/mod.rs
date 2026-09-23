//! Delivery-only OR-set example. The MVR demo exercises authentication and permissions.
//! Local signing and signature-backed export remain deferred; local edits use the
//! local ingestion path, not received-history admission.

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use stockfoil::{Author, CommitGraph, CommitHash, CommitMeta};

pub struct OrSet {
    graph: CommitGraph<OrSetCommit>,
    state: State,
    actor: ActorId,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ActorId([u8; 32]);

impl ActorId {
    pub fn generate() -> Self {
        let key = SigningKey::from_bytes(&rand::random());
        Self(key.verifying_key().to_bytes())
    }

    fn author(&self) -> Author {
        Author::try_from(self.0.as_slice()).unwrap()
    }
}

impl OrSet {
    pub fn new() -> Self {
        Self {
            graph: CommitGraph::new(),
            state: State {
                adds: HashMap::new(),
                removes: BTreeSet::new(),
            },
            actor: ActorId::generate(),
        }
    }

    pub fn add(&mut self, value: String) {
        let commit = OrSetCommit {
            actor: self.actor.clone(),
            ops: vec![Op::Add { value }],
        };
        self.ingest_local_commit(
            self.graph.commit_by(self.actor.author(), &commit.encode()),
            commit,
        );
    }

    pub fn remove(&mut self, value: String) {
        let predecessors = self
            .state
            .adds
            .get(&value)
            .map(|commits| {
                let remaining_adds = commits.difference(&self.state.removes);
                remaining_adds.cloned().collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // There is no surviving key which sets this value. Nothing to do
        if predecessors.is_empty() {
            return;
        }
        let commit = OrSetCommit {
            actor: self.actor.clone(),
            ops: vec![Op::Remove { predecessors }],
        };
        self.ingest_local_commit(
            self.graph.commit_by(self.actor.author(), &commit.encode()),
            commit,
        );
    }

    pub fn fork(&self) -> Self {
        Self {
            graph: self.graph.clone(),
            state: State {
                adds: self.state.adds.clone(),
                removes: self.state.removes.clone(),
            },
            actor: ActorId::generate(),
        }
    }

    pub fn merge(&mut self, other: &Self) {
        let heads_before = self.graph.delivered_heads().collect::<Vec<_>>();
        self.graph.merge(&other.graph);
        self.evaluate(&heads_before);
    }

    fn ingest_local_commit(&mut self, meta: CommitMeta, commit: OrSetCommit) {
        let heads_before = self.graph.delivered_heads().collect::<Vec<_>>();
        self.graph.ingest_local_commit(meta, commit);
        self.evaluate(&heads_before);
    }

    fn evaluate(&mut self, start_at: &[CommitHash]) {
        self.graph
            .traverse_delivered_since(start_at, |meta, commit| {
                for op in &commit.ops {
                    match op {
                        Op::Add { value } => {
                            self.state
                                .adds
                                .entry(value.to_string())
                                .or_default()
                                .insert(*meta.hash());
                        }
                        Op::Remove { predecessors } => {
                            self.state.removes.extend(predecessors);
                        }
                    }
                }
                ControlFlow::Continue(())
            });
    }

    pub fn value(&self) -> BTreeSet<String> {
        let mut result = BTreeSet::new();
        for (key, commits) in &self.state.adds {
            let remaining_adds = commits.difference(&self.state.removes);
            if remaining_adds.count() > 0 {
                result.insert(key.to_string());
            }
        }
        result
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Op {
    Add {
        value: String,
    },
    Remove {
        #[serde(with = "commit_hashes")]
        predecessors: Vec<CommitHash>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OrSetCommit {
    pub ops: Vec<Op>,
    pub actor: ActorId,
}

impl OrSetCommit {
    pub fn encode(&self) -> Vec<u8> {
        bcs::to_bytes(self).expect("OR-set commit serialization failed")
    }

    pub fn decode(data: &[u8]) -> bcs::Result<Self> {
        bcs::from_bytes(data)
    }
}

// Keep Serde support local to the tests; hashes encode as fixed-size byte arrays.
mod commit_hashes {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        hashes: &[CommitHash],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(hashes.iter().map(CommitHash::as_bytes))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<CommitHash>, D::Error> {
        let hashes = Vec::<[u8; 32]>::deserialize(deserializer)?;
        Ok(hashes.into_iter().map(CommitHash::from).collect())
    }
}

pub struct State {
    adds: HashMap<String, BTreeSet<CommitHash>>,
    removes: BTreeSet<CommitHash>,
}
