//! Multi-value registers: retain assignments, then select each key's causally
//! maximal participating writes. This example only merges in-memory replicas.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use stockfoil::{
    Author, CommitGraph, CommitHash, DiffError, Signature, ViewPoint, Vis, WritePermission,
};

pub struct MvRegisterMap {
    graph: CommitGraph<Assignment>,
    signing_key: SigningKey,
    pending: BTreeSet<CommitHash>,
}

#[derive(Clone, Serialize)]
struct Assignment {
    key: String,
    value: String,
}

impl MvRegisterMap {
    pub fn new() -> Self {
        let mut doc = Self {
            graph: CommitGraph::new(),
            signing_key: SigningKey::from_bytes(&rand::random()),
            pending: BTreeSet::new(),
        };
        doc.set_write_permissions(&HashMap::from([(doc.author(), WritePermission::Unbounded)]));
        doc
    }

    pub fn author(&self) -> Author {
        Author::try_from(self.signing_key.verifying_key().as_bytes().as_slice()).unwrap()
    }

    pub fn prepare_set(&mut self, key: &str, value: &str) -> CommitHash {
        let assignment = Assignment {
            key: key.to_owned(),
            value: value.to_owned(),
        };
        let meta = self
            .graph
            .commit_by(self.author(), &bcs::to_bytes(&assignment).unwrap());
        let hash = *meta.hash();
        self.graph.ingest_local_commit(meta, assignment);
        self.pending.insert(hash);
        hash
    }

    pub fn publish(&mut self) {
        for hash in std::mem::take(&mut self.pending) {
            // Signing a fragment may already have admitted another pending edit.
            if !self.graph.has_commit(&hash) {
                let signature = Signature::from(self.signing_key.sign(hash.as_bytes()).to_bytes());
                self.graph
                    .ingest_signature(self.author(), hash, signature)
                    .unwrap();
            }
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> CommitHash {
        let hash = self.prepare_set(key, value);
        self.publish();
        hash
    }

    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.graph.set_write_permissions(permissions);
    }

    pub fn view_point(&self) -> ViewPoint {
        self.graph.view_point()
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.get_at(&self.view_point(), key).unwrap()
    }

    pub fn get_at(&self, view: &ViewPoint, key: &str) -> Result<BTreeSet<String>, DiffError> {
        let participation = self.graph.diff(view, view)?;
        let mut writes = BTreeMap::new();
        self.graph.traverse(|meta, assignment| {
            if assignment.key == key && participation.visibility(meta.hash()).unwrap() == Vis::Both
            {
                writes.retain(|hash, _| !self.graph.is_ancestor(hash, meta.hash()));
                writes.insert(*meta.hash(), assignment.value.clone());
            }
            ControlFlow::Continue(())
        });
        Ok(writes.into_values().collect())
    }

    pub fn fork(&self) -> Self {
        let mut fork = Self::new();
        fork.set_write_permissions(self.view_point().write_permissions());
        fork.merge(self);
        fork
    }

    pub fn merge(&mut self, other: &Self) {
        self.graph.merge(&other.graph);
    }
}
