//! Multi-value registers materialized through a single `visit_changes` callback.
//! Retain all assignments so revocation can reveal previously overwritten values.
//! This example only merges in-memory replicas.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use stockfoil::{
    Author, CommitGraph, CommitHash, DiffError, MaterializationCursor, Signature, ViewPoint, Vis,
    WritePermission,
};

pub struct MvRegisterMap {
    graph: CommitGraph<Assignment>,
    signing_key: SigningKey,
    pending: BTreeSet<CommitHash>,
    materialized: Materialized,
}

#[derive(Clone, Serialize)]
struct Assignment {
    key: String,
    value: String,
}

#[derive(Clone, Default)]
struct Materialized {
    cursor: MaterializationCursor,
    // Delivery is parent-closed, so appending each new batch preserves this
    // topological order even when the displayed viewpoint moves backwards.
    assignments: Vec<(CommitHash, Assignment)>,
    participating: BTreeSet<CommitHash>,
    values: BTreeMap<String, BTreeSet<String>>,
}

impl Materialized {
    fn update(
        &mut self,
        graph: &CommitGraph<Assignment>,
        view: &ViewPoint,
    ) -> Result<(), DiffError> {
        let mut affected = BTreeSet::new();
        let next = graph.visit_changes(&self.cursor, view, |change| {
            let hash = *change.meta.hash();
            if change.newly_delivered {
                // Keep even denied writes: a later permission change can enable
                // them without delivering their payloads again.
                self.assignments.push((hash, change.payload.clone()));
            }
            match change.visibility {
                Vis::After => {
                    self.participating.insert(hash);
                    affected.insert(change.payload.key.clone());
                }
                Vis::Before => {
                    self.participating.remove(&hash);
                    affected.insert(change.payload.key.clone());
                }
                Vis::Both | Vis::Neither => {}
            }
        })?;

        // A revoked assignment can reveal an older write that wasn't revisited.
        // Recompute only affected keys, using all retained participating writes.
        for key in affected {
            let mut maximal = BTreeMap::new();
            for (hash, assignment) in &self.assignments {
                if assignment.key == key && self.participating.contains(hash) {
                    maximal.retain(|older, _| !graph.is_ancestor(older, hash));
                    maximal.insert(*hash, assignment.value.clone());
                }
            }
            self.values.insert(key, maximal.into_values().collect());
        }
        self.cursor = next;
        Ok(())
    }

    fn get(&self, key: &str) -> BTreeSet<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }
}

impl MvRegisterMap {
    pub fn new() -> Self {
        let mut doc = Self {
            graph: CommitGraph::new(),
            signing_key: SigningKey::from_bytes(&rand::random()),
            pending: BTreeSet::new(),
            materialized: Materialized::default(),
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
        self.materialize();
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
        self.materialize();
    }

    pub fn set(&mut self, key: &str, value: &str) -> CommitHash {
        let hash = self.prepare_set(key, value);
        self.publish();
        hash
    }

    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.graph.set_write_permissions(permissions);
        self.materialize();
    }

    pub fn view_point(&self) -> ViewPoint {
        self.graph.view_point()
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.materialized.get(key)
    }

    pub fn get_at(&self, view: &ViewPoint, key: &str) -> Result<BTreeSet<String>, DiffError> {
        // Use the same update path on a private copy of the state and cursor.
        // Historical reads must not move the live document's materialization.
        let mut historical = self.materialized.clone();
        historical.update(&self.graph, view)?;
        Ok(historical.get(key))
    }

    pub fn fork(&self) -> Self {
        let mut fork = Self::new();
        fork.set_write_permissions(self.view_point().write_permissions());
        fork.merge(self);
        fork
    }

    pub fn merge(&mut self, other: &Self) {
        self.graph.merge(&other.graph);
        self.materialize();
    }

    fn materialize(&mut self) {
        self.materialized
            .update(&self.graph, &self.graph.view_point())
            .unwrap();
    }
}
