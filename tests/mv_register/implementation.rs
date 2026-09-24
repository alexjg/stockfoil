//! A map of multi-value registers. Keep every assignment in the graph, then
//! select the causally maximal participating writes for each key.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use stockfoil::{
    Author, CommitGraph, CommitHash, ResolveError, Signature, SignatureError, ViewPoint, Vis,
    WritePermission,
};

pub struct MvRegisterMap {
    graph: CommitGraph<Assignment>,
    state: State,
    actor: ActorId,
    signing_key: Option<SigningKey>,
    pending_signatures: BTreeSet<CommitHash>,
    // Only locally captured working viewpoints may be read before signature-backed
    // resolution. This small demo retains them to support historical reads.
    local_view_points: Vec<ViewPoint>,
}

#[derive(Clone, Copy, Serialize)]
struct ActorId([u8; 32]);

#[derive(Clone, Serialize)]
struct Assignment {
    actor: ActorId,
    key: String,
    value: String,
}

#[derive(Clone, Default)]
struct State {
    values: HashMap<String, BTreeMap<CommitHash, String>>,
}

impl State {
    fn get(&self, key: &str) -> BTreeSet<String> {
        self.values
            .get(key)
            .into_iter()
            .flat_map(|writes| writes.values().cloned())
            .collect()
    }
}

impl MvRegisterMap {
    pub fn new() -> Self {
        let key = SigningKey::from_bytes(&rand::random());
        let author = Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
        let mut doc = Self::with_author(author);
        doc.signing_key = Some(key);
        doc
    }

    pub fn with_author(author: Author) -> Self {
        let mut graph = CommitGraph::new();
        // An explicit demo grant, not a library default.
        graph.set_write_permissions(&HashMap::from([(author, WritePermission::Unbounded)]));
        let view_point = graph.view_point();
        Self {
            graph,
            state: State::default(),
            actor: ActorId(*author.as_array()),
            signing_key: None,
            pending_signatures: BTreeSet::new(),
            local_view_points: vec![view_point],
        }
    }

    pub fn author(&self) -> Author {
        Author::try_from(self.actor.0.as_slice()).unwrap()
    }

    pub fn prepare_set(&mut self, key: &str, value: &str) -> CommitHash {
        let assignment = Assignment {
            actor: self.actor,
            key: key.to_owned(),
            value: value.to_owned(),
        };
        let bytes = bcs::to_bytes(&assignment).expect("assignment serialization failed");
        let meta = self.graph.commit_by(self.author(), &bytes);
        let hash = *meta.hash();
        self.graph.ingest_local_commit(meta, assignment);
        self.pending_signatures.insert(hash);
        self.refresh();
        hash
    }

    pub fn ingest_signature(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<(), SignatureError> {
        self.graph.ingest_signature(author, hash, signature)?;
        if author == self.author() {
            self.pending_signatures.remove(&hash);
        }
        self.refresh();
        Ok(())
    }

    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.graph.set_write_permissions(permissions);
        self.refresh();
    }

    /// Capture the current document view, including local unsigned edits.
    pub fn view_point(&self) -> ViewPoint {
        self.graph.view_point()
    }

    /// Capture the document view backed by signatures.
    pub fn signed_view_point(&self) -> ViewPoint {
        self.graph.signed_view_point()
    }

    pub fn get_at(
        &self,
        view_point: &ViewPoint,
        key: &str,
    ) -> Result<BTreeSet<String>, ResolveError> {
        if !self.local_view_points.contains(view_point) {
            // Received descriptions are not authority to skip verification.
            self.graph.resolve(view_point)?;
        }
        Ok(self.state_at(view_point)?.get(key))
    }

    /// Direct-sign pending assignments so the whole structural cover can publish.
    pub fn publish(&mut self) {
        let signing_key = self
            .signing_key
            .as_ref()
            .expect("external signers must use prepare_set and ingest_signature")
            .clone();
        for hash in self.pending_signatures.clone() {
            // A just-signed fragment may have admitted earlier pending edits.
            if self.graph.has_admitted_commit(&hash) {
                self.pending_signatures.remove(&hash);
                continue;
            }
            let signature = Signature::from(signing_key.sign(hash.as_bytes()).to_bytes());
            self.ingest_signature(self.author(), hash, signature)
                .expect("signature from our own signing key");
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> CommitHash {
        assert!(
            self.signing_key.is_some(),
            "external signers must use prepare_set and ingest_signature"
        );
        let hash = self.prepare_set(key, value);
        self.publish();
        hash
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.state.get(key)
    }

    pub fn fork(&self) -> Self {
        let mut fork = Self::new();
        fork.set_write_permissions(self.graph.view_point().write_permissions());
        fork.merge(self);
        fork
    }

    pub fn merge(&mut self, other: &Self) {
        self.graph.merge(&other.graph);
        self.refresh();
    }

    fn refresh(&mut self) {
        let view_point = self.graph.view_point();
        self.state = self
            .state_at(&view_point)
            .expect("current working history is available");
        if !self.local_view_points.contains(&view_point) {
            self.local_view_points.push(view_point);
        }
    }

    fn state_at(&self, view_point: &ViewPoint) -> Result<State, ResolveError> {
        let graph = &self.graph;
        let participation = graph.diff(view_point, view_point)?;
        let mut state = State::default();
        graph.traverse(|meta, assignment| {
            if participation
                .visibility(meta.hash())
                .expect("delivered commit")
                == Vis::Both
            {
                let writes = state.values.entry(assignment.key.clone()).or_default();
                // Inactive writes don't overwrite, but ancestry still passes
                // through them. An allowed descendant can supersede older values.
                writes.retain(|hash, _| !graph.is_ancestor(hash, meta.hash()));
                writes.insert(*meta.hash(), assignment.value.clone());
            }
            ControlFlow::Continue(())
        });
        Ok(state)
    }
}
