//! A map of multi-value registers. Keep every assignment in the graph, then
//! select the causally maximal participating writes for each key.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, BlobHash, CommitGraph, CommitHash, CommitMeta, ItemId, ResolveError, Signature,
    SignatureError, UnverifiedItem, ViewPoint, Vis, WritePermission,
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

/// A whole certified cover item and every member's canonical payload.
#[derive(Clone)]
pub struct AssignmentMessage {
    pub(super) item: UnverifiedItem,
    pub(super) payloads: BTreeMap<CommitHash, (CommitMeta, Vec<u8>)>,
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

    /// Export whole current minimal-cover items in dependency order. A covered
    /// assignment is never requested as an independently exportable commit.
    pub fn export_assignments(&self) -> Vec<AssignmentMessage> {
        let cover = self.graph.minimal_cover();
        let mut pending: Vec<_> = cover
            .loose_commits
            .into_iter()
            .map(|entry| ItemId::Commit(entry.commit))
            .chain(
                cover
                    .fragments
                    .into_iter()
                    .map(|entry| ItemId::Fragment(entry.range)),
            )
            .map(|id| self.graph.export(&id).expect("current cover is exportable"))
            .collect();
        let mut messages = Vec::new();
        let mut exported = BTreeSet::new();
        while !pending.is_empty() {
            let index = pending
                .iter()
                .position(|item| item.dependencies().all(|hash| exported.contains(&hash)))
                .expect("cover dependencies must be closed");
            let item = pending.remove(index);
            let verified = item
                .clone()
                .verify()
                .expect("exported certificate must verify");
            let payloads = verified
                .commits()
                .map(|meta| {
                    let (_, assignment) = self
                        .graph
                        .publishable_commit(meta.hash())
                        .expect("cover member must be readable");
                    exported.insert(*meta.hash());
                    (
                        *meta.hash(),
                        (
                            meta.clone(),
                            bcs::to_bytes(assignment).expect("assignment serialization failed"),
                        ),
                    )
                })
                .collect();
            messages.push(AssignmentMessage { item, payloads });
        }
        messages
    }

    /// Authenticate the complete item and validate all payload bindings before
    /// admitting any remote history (including duplicate boundary members).
    pub fn receive_assignment(&mut self, message: AssignmentMessage) -> Result<(), &'static str> {
        let verified = message
            .item
            .verify()
            .map_err(|_| "assignment verification failed")?;
        if message.payloads.len() != verified.commits().count() {
            return Err("assignment metadata does not match the verified item");
        }
        let mut assignments = BTreeMap::new();
        for meta in verified.commits() {
            let (claimed, bytes) = message
                .payloads
                .get(meta.hash())
                .ok_or("missing assignment metadata")?;
            if meta.hash() != claimed.hash()
                || meta.parents() != claimed.parents()
                || meta.author() != claimed.author()
                || meta.payload_hash() != claimed.payload_hash()
            {
                return Err("assignment metadata does not match the verified commit");
            }
            if &BlobHash::of_contents(bytes) != meta.payload_hash() {
                return Err("assignment does not match the commit's payload hash");
            }
            let assignment: Assignment =
                bcs::from_bytes(bytes).map_err(|_| "invalid assignment encoding")?;
            if assignment.actor.0 != *meta.author().as_array() {
                return Err("assignment actor does not match the commit author");
            }
            assignments.insert(*meta.hash(), assignment);
        }
        self.graph
            .ingest(verified, |meta| assignments[meta.hash()].clone())
            .map_err(|_| "assignment admission failed")?;
        self.refresh();
        Ok(())
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
