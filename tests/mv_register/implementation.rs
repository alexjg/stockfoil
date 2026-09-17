//! A map of multi-value registers. Keep every assignment in the graph, then
//! select the causally maximal participating writes for each key.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, BlobHash, CommitGraph, CommitHash, CommitMeta, Pov, ResolveError, Signature,
    SignatureError, Vis, WritePermission,
};

pub struct MvRegisterMap {
    graph: CommitGraph<Assignment>,
    state: State,
    actor: ActorId,
    signing_key: Option<SigningKey>,
    // Only locally captured working POVs may be read before signature-backed
    // resolution. This small demo retains them to support historical reads.
    local_povs: Vec<Pov>,
}

/// A published assignment's envelope and canonical payload bytes. Signatures
/// travel separately. The graph metadata stays opaque to document callers.
#[derive(Clone)]
pub struct AssignmentMessage {
    meta: CommitMeta,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct ActorId([u8; 32]);

#[derive(Clone, Serialize, Deserialize)]
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
        let pov = graph.pov();
        Self {
            graph,
            state: State::default(),
            actor: ActorId(*author.as_array()),
            signing_key: None,
            local_povs: vec![pov],
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
        self.refresh();
        Ok(())
    }

    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.graph.set_write_permissions(permissions);
        self.refresh();
    }

    /// Capture the current document view, including local unsigned edits.
    pub fn pov(&self) -> Pov {
        self.graph.pov()
    }

    /// Capture the document view backed by signatures.
    pub fn signed_pov(&self) -> Pov {
        self.graph.signed_pov()
    }

    pub fn get_at(&self, pov: &Pov, key: &str) -> Result<BTreeSet<String>, ResolveError> {
        if !self.local_povs.contains(pov) {
            // Received descriptions are not authority to skip verification.
            self.graph.resolve(pov)?;
        }
        Ok(self.state_at(pov)?.get(key))
    }

    pub fn set(&mut self, key: &str, value: &str) -> CommitHash {
        let signing_key = self
            .signing_key
            .as_ref()
            .expect("external signers must use prepare_set and ingest_signature")
            .clone();
        let hash = self.prepare_set(key, value);
        let signature = Signature::from(signing_key.sign(hash.as_bytes()).to_bytes());
        self.ingest_signature(self.author(), hash, signature)
            .expect("signature from our own signing key");
        hash
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.state.get(key)
    }

    pub fn fork(&self) -> Self {
        let mut fork = Self::new();
        fork.set_write_permissions(self.graph.pov().write_permissions());
        fork.merge(self);
        fork
    }

    pub fn merge(&mut self, other: &Self) {
        self.graph.merge(&other.graph);
        self.refresh();
    }

    /// Export assignment messages, withholding unsigned local edits. Unlike
    /// `merge`, this does not transfer signature evidence.
    pub fn export_assignments(&self) -> Vec<AssignmentMessage> {
        let mut messages = Vec::new();
        self.graph.traverse_publishable(|meta, assignment| {
            messages.push(AssignmentMessage {
                meta: meta.clone(),
                payload: bcs::to_bytes(assignment).expect("assignment serialization failed"),
            });
            ControlFlow::Continue(())
        });
        messages
    }

    /// Validate received bytes before decoding or storing an assignment. Receipt
    /// alone does not authenticate it, even if it claims our own author identity.
    pub fn receive_assignment(&mut self, message: AssignmentMessage) -> Result<(), &'static str> {
        if &BlobHash::of_contents(&message.payload) != message.meta.payload_hash() {
            return Err("assignment does not match the commit's payload hash");
        }
        let assignment =
            bcs::from_bytes(&message.payload).map_err(|_| "invalid assignment encoding")?;
        self.graph.ingest_commit(message.meta, assignment);
        self.refresh();
        Ok(())
    }

    fn refresh(&mut self) {
        let pov = self.graph.pov();
        self.state = self
            .state_at(&pov)
            .expect("current working history is available");
        if !self.local_povs.contains(&pov) {
            self.local_povs.push(pov);
        }
    }

    fn state_at(&self, pov: &Pov) -> Result<State, ResolveError> {
        let graph = &self.graph;
        let participation = graph.diff(pov, pov)?;
        let mut state = State::default();
        graph.traverse_delivered(|meta, assignment| {
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
