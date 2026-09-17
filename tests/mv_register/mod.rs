//! A map of multi-value registers: causal writes replace earlier writes to the
//! same key, while concurrent writes remain visible as conflicts.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

use serde::Serialize;
use stockfoil::{CommitGraph, CommitHash, CommitMeta};

mod revocation;
mod signatures;
mod tests;

pub struct MvRegisterMap {
    graph: CommitGraph<Assignment>,
    state: State,
    actor: ActorId,
}

#[derive(Clone, Copy, Serialize)]
struct ActorId([u8; 32]);

impl ActorId {
    fn generate() -> Self {
        Self(rand::random())
    }
}

#[derive(Clone, Serialize)]
struct Assignment {
    // Distinguish independent writes with identical parents, key, and value.
    actor: ActorId,
    key: String,
    value: String,
}

#[derive(Clone, Default)]
struct State {
    values: HashMap<String, BTreeMap<CommitHash, String>>,
}

impl MvRegisterMap {
    pub fn new() -> Self {
        Self {
            graph: CommitGraph::new(),
            state: State::default(),
            actor: ActorId::generate(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let assignment = Assignment {
            actor: self.actor,
            key: key.to_owned(),
            value: value.to_owned(),
        };
        let bytes = bcs::to_bytes(&assignment).expect("assignment serialization failed");
        self.ingest_commit(self.graph.commit(&bytes), assignment);
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.state
            .values
            .get(key)
            .into_iter()
            .flat_map(|writes| writes.values().cloned())
            .collect()
    }

    pub fn fork(&self) -> Self {
        Self {
            graph: self.graph.clone(),
            state: self.state.clone(),
            actor: ActorId::generate(),
        }
    }

    pub fn merge(&mut self, other: &Self) {
        let before = self.graph.heads().collect::<Vec<_>>();
        self.graph.merge(&other.graph);
        self.evaluate(&before);
    }

    fn ingest_commit(&mut self, meta: CommitMeta, assignment: Assignment) {
        let before = self.graph.heads().collect::<Vec<_>>();
        self.graph.ingest_commit(meta, assignment);
        self.evaluate(&before);
    }

    fn evaluate(&mut self, since: &[CommitHash]) {
        let graph = &self.graph;
        graph.traverse_since(since, |meta, assignment| {
            let writes = self.state.values.entry(assignment.key.clone()).or_default();
            // Replace only causally earlier writes, not concurrent assignments.
            writes.retain(|hash, _| !graph.is_ancestor(hash, meta.hash()));
            writes.insert(*meta.hash(), assignment.value.clone());
            ControlFlow::Continue(())
        });
    }
}
