use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use serde::{Deserialize, Serialize};
use stockfoil::{CommitGraph, CommitHash};

#[derive(Serialize, Deserialize)]
pub enum Op {
    Add {
        value: String,
    },
    Remove {
        #[serde(with = "commit_hashes")]
        predecessors: Vec<CommitHash>,
    },
}

pub fn evaluate_orset(graph: &CommitGraph<OrSetCommit>) -> BTreeSet<String> {
    let mut state = State {
        adds: HashMap::new(),
        removes: BTreeSet::new(),
    };
    graph.traverse(|meta, payload| {
        for op in &payload.ops {
            match op {
                Op::Add { value } => {
                    state
                        .adds
                        .entry(value.to_string())
                        .or_default()
                        .insert(*meta.hash());
                }
                Op::Remove { predecessors } => {
                    state.removes.extend(predecessors);
                }
            }
        }
        ControlFlow::Continue(())
    });
    state.finish()
}

#[derive(Serialize, Deserialize)]
pub struct OrSetCommit {
    pub ops: Vec<Op>,
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

impl State {
    pub fn finish(&self) -> BTreeSet<String> {
        let mut result = BTreeSet::new();
        for (key, commits) in &self.adds {
            let remaining_adds = commits.difference(&self.removes);
            if remaining_adds.count() > 0 {
                result.insert(key.to_string());
            }
        }
        result
    }
}
