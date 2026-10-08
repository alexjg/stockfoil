//! A multi-value register map on a Stockfoil [`Graph`].
//!
//! - Assignments are local commits: drafts first ([`MvRegisterMap::prepare_set`]),
//!   admitted once signed with Ed25519 ([`MvRegisterMap::publish`]).
//! - The materialized map follows [`stockfoil::History::changes`]: newly delivered
//!   assignments are stored (even when they do not participate), and writes are
//!   added or removed as their participation changes. Write permissions are
//!   local configuration; changing them reveals or hides writes without replay.
//! - Replicas synchronize by exporting what a peer has not acknowledged
//!   ([`ExportCursor`]) and receiving the encoded items. Items that arrive
//!   before their dependencies wait as verified [`Admission`]s.
pub mod codec;

use codec::{MAX_KEY_BYTES, RegisterCodec};
use ed25519_dalek::{Signer, SigningKey};
use eyre::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use stockfoil::{
    Admission, Author, Batch, BlobHash, Change, CommitHash, Driven, Error, ExportCursor,
    ExportPlan, Graph, Heads, MaterializationCursor, MemoryStorage, PayloadLocation,
    ReceiveOptions, Signature, Storage, ViewPoint, Vis, WritePermission, WritePermissions,
};

pub type RegisterGraph = Graph<RegisterCodec, MemoryStorage>;

/// Change records requested per [`stockfoil::History::next_changes`] call.
pub const BATCH: usize = 256;

/// One register write: the canonical payload of a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub key: String,
    pub value: String,
}

impl Assignment {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    /// `uleb(key.len) key uleb(value.len) value`
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.key.len() + self.value.len() + 4);
        for field in [&self.key, &self.value] {
            codec::put_uleb(&mut out, field.len() as u64);
            out.extend_from_slice(field.as_bytes());
        }
        out
    }

    /// `None` for bytes that are not a canonical assignment. Signed history
    /// can contain such payloads; the materializer ignores them.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut rest = bytes;
        let mut field = || -> Option<String> {
            let mut len = 0u64;
            let mut shift = 0;
            loop {
                let (&byte, tail) = rest.split_first()?;
                rest = tail;
                len |= u64::from(byte & 0x7f).checked_shl(shift)?;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let len = usize::try_from(len).ok().filter(|&n| n <= rest.len())?;
            let (text, tail) = rest.split_at(len);
            rest = tail;
            String::from_utf8(text.to_vec()).ok()
        };
        let assignment = Self::new(field()?, field()?);
        (rest.is_empty() && assignment.key.len() <= MAX_KEY_BYTES).then_some(assignment)
    }
}

/// The outcome of receiving one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// Admitted; `new` commits were delivered for the first time.
    Admitted { new: usize },
    /// Stored and verified, waiting for this dependency.
    Waiting(CommitHash),
}

/// Materialized register values and the cursor they correspond to.
#[derive(Clone, Default)]
struct Materialized {
    cursor: MaterializationCursor,
    /// Every delivered assignment, including writes that do not participate
    /// now: a later view may enable them without delivering them again.
    assignments: HashMap<CommitHash, Assignment>,
    /// Delivered writes per key, parents first.
    by_key: HashMap<String, Vec<CommitHash>>,
    participating: HashSet<CommitHash>,
    values: BTreeMap<String, BTreeSet<String>>,
}

impl Materialized {
    /// Apply one change traversal to `target`. On failure the state must be
    /// discarded: it may hold part of the traversal.
    fn update(&mut self, graph: &mut RegisterGraph, target: &ViewPoint) -> Result<(), Error> {
        let mut changes = graph.history().changes(&self.cursor, target)?;
        let mut batch = Vec::with_capacity(BATCH);
        let mut affected = BTreeSet::new();
        let cursor = loop {
            batch.clear();
            let progress = graph
                .history()
                .next_changes(&mut changes, &mut batch, BATCH)?;
            let mut loaded = load(graph, batch.iter().filter(|c| c.newly_delivered))?;
            for change in &batch {
                if let Some(assignment) = loaded.remove(&change.hash) {
                    self.by_key
                        .entry(assignment.key.clone())
                        .or_default()
                        .push(change.hash);
                    self.assignments.insert(change.hash, assignment);
                }
                let Some(assignment) = self.assignments.get(&change.hash) else {
                    continue; // not a canonical assignment
                };
                match change.visibility {
                    Vis::After => self.participating.insert(change.hash),
                    Vis::Before => self.participating.remove(&change.hash),
                    Vis::Both | Vis::Neither => continue,
                };
                affected.insert(assignment.key.clone());
            }
            if let Batch::Complete(cursor) = progress {
                break cursor;
            }
        };
        // Hiding a write can reveal an older one that produced no record, so
        // recompute affected keys from all retained participating writes.
        for key in affected {
            let mut maximal: Vec<CommitHash> = Vec::new();
            for hash in &self.by_key[&key] {
                if self.participating.contains(hash) {
                    maximal.retain(|older| !graph.history().is_ancestor(older, hash));
                    maximal.push(*hash);
                }
            }
            let values: BTreeSet<_> = maximal
                .iter()
                .map(|h| self.assignments[h].value.clone())
                .collect();
            if values.is_empty() {
                self.values.remove(&key);
            } else {
                self.values.insert(key, values);
            }
        }
        self.cursor = cursor;
        Ok(())
    }

    fn get(&self, key: &str) -> BTreeSet<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }
}

/// Fetch and decode the payloads of `changes`. Members of one stored item are
/// read with a single decoding pass.
fn load<'a>(
    graph: &mut RegisterGraph,
    changes: impl Iterator<Item = &'a Change>,
) -> Result<HashMap<CommitHash, Assignment>, Error> {
    let mut loaded = HashMap::new();
    let mut members: BTreeMap<BlobHash, BTreeMap<usize, CommitHash>> = BTreeMap::new();
    for change in changes {
        let location = graph
            .storage()
            .location(change.payload)
            .map_err(|e| Error::Storage(e.to_string()))?;
        match location {
            PayloadLocation::Canonical(_) => {
                if let Some(a) = Assignment::decode(&graph.payload(&change.hash)?) {
                    loaded.insert(change.hash, a);
                }
            }
            PayloadLocation::Member { item, index } => {
                members.entry(item).or_default().insert(index, change.hash);
            }
        }
    }
    for (item, wanted) in members {
        let mut bytes: HashMap<usize, Vec<u8>> = HashMap::new();
        graph.decode_item(&item, &wanted, |member, chunk| {
            bytes.entry(member).or_default().extend_from_slice(chunk);
            Ok(())
        })?;
        for (member, hash) in wanted {
            if let Some(a) = bytes.get(&member).and_then(|b| Assignment::decode(b)) {
                loaded.insert(hash, a);
            }
        }
    }
    Ok(loaded)
}

/// One replica: a graph, its author key, permissions, and materialized values.
pub struct MvRegisterMap {
    graph: RegisterGraph,
    signing_key: SigningKey,
    permissions: WritePermissions,
    materialized: Materialized,
    /// Received items that are verified but wait for history they build on,
    /// with their storage keys.
    waiting: Vec<(BlobHash, Admission)>,
    /// What each peer, named by its author key, has acknowledged.
    sent: HashMap<Author, ExportCursor>,
}

impl Default for MvRegisterMap {
    fn default() -> Self {
        Self::new()
    }
}

impl MvRegisterMap {
    /// A replica with a fresh random key that permits only its own writes.
    pub fn new() -> Self {
        Self::with_key(SigningKey::from_bytes(&rand::random()))
    }

    pub fn with_key(signing_key: SigningKey) -> Self {
        let mut doc = Self {
            graph: Graph::new(RegisterCodec::default(), MemoryStorage::new()),
            signing_key,
            permissions: WritePermissions::new(),
            materialized: Materialized::default(),
            waiting: Vec::new(),
            sent: HashMap::new(),
        };
        doc.permissions
            .insert(doc.author(), WritePermission::Unbounded);
        doc.materialize().expect("empty history");
        doc
    }

    pub fn author(&self) -> Author {
        Author::try_from(self.signing_key.verifying_key().as_bytes().as_slice()).unwrap()
    }

    pub fn graph(&self) -> &RegisterGraph {
        &self.graph
    }

    /// Commit an unsigned draft on top of every delivered head. Drafts are
    /// visible locally but never exported.
    pub fn prepare_set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        ensure!(
            key.len() <= MAX_KEY_BYTES,
            "key longer than {MAX_KEY_BYTES} bytes"
        );
        let parents = self.graph.history().heads(Heads::Delivered).collect();
        let payload = Assignment::new(key, value).encode();
        let prepared = self
            .graph
            .commit(self.author(), parents, payload.as_slice())?;
        self.materialize()?;
        Ok(prepared.hash)
    }

    /// Sign every draft, parents first, admitting them.
    pub fn publish(&mut self) -> Result<()> {
        let drafts: Vec<_> = self.graph.history().drafts().collect();
        for hash in drafts {
            if !self.graph.history().is_admitted(&hash) {
                let signature = Signature::from(self.signing_key.sign(hash.as_bytes()).to_bytes());
                self.graph.sign(self.author(), hash, signature)?;
            }
        }
        self.materialize()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        let hash = self.prepare_set(key, value)?;
        self.publish()?;
        Ok(hash)
    }

    pub fn write_permissions(&self) -> &WritePermissions {
        &self.permissions
    }

    pub fn set_write_permissions(&mut self, permissions: WritePermissions) -> Result<()> {
        self.permissions = permissions;
        self.materialize()
    }

    /// Everything delivered (drafts included) under the current permissions.
    pub fn view_point(&self) -> ViewPoint {
        self.graph.history().view_point(&self.permissions)
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.materialized.get(key)
    }

    /// Every key with at least one value.
    pub fn values(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.materialized.values
    }

    /// Values at another view, computed on a copy of the live state.
    pub fn get_at(&mut self, view: &ViewPoint, key: &str) -> Result<BTreeSet<String>> {
        let mut historical = self.materialized.clone();
        historical.update(&mut self.graph, view)?;
        Ok(historical.get(key))
    }

    /// The cursor of the live materialization. Persist it with the values.
    pub fn materialization_cursor(&self) -> &MaterializationCursor {
        &self.materialized.cursor
    }

    /// Discard the materialized state and rebuild it from an empty cursor.
    pub fn rematerialize(&mut self) -> Result<()> {
        self.materialized = Materialized::default();
        self.materialize()
    }

    fn materialize(&mut self) -> Result<()> {
        let target = self.view_point();
        if let Err(e) = self.materialized.update(&mut self.graph, &target) {
            // Partial effects are discarded; the next update starts over.
            self.materialized = Materialized::default();
            return Err(e.into());
        }
        Ok(())
    }

    /// Plan and encode every item `cursor` has not acknowledged.
    pub fn export_items(&mut self, cursor: &ExportCursor) -> Result<(ExportPlan, Vec<Vec<u8>>)> {
        let plan = self.graph.export(cursor);
        let mut items = Vec::with_capacity(plan.len());
        for index in 0..plan.len() {
            let mut bytes = Vec::new();
            self.graph.write_item(&plan, index, &mut bytes)?;
            items.push(bytes);
        }
        Ok((plan, items))
    }

    /// Store and admit one encoded item, then update the materialized values.
    pub fn receive(&mut self, item: &[u8]) -> Result<Received> {
        let received = self.receive_item(item);
        self.materialize()?;
        received
    }

    /// Items stored and verified but waiting for dependencies.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    fn receive_item(&mut self, item: &[u8]) -> Result<Received> {
        let storage = self.graph.storage_mut();
        let mut writer = storage.create()?;
        writer.write_all(item)?;
        let blob = storage.finish(writer)?;
        let options = ReceiveOptions::new(self.graph.codec().format_id().clone());
        let mut op = self.graph.history().receive(options);
        match self.graph.drive(&blob, &mut op)? {
            Driven::Done(receipt) => {
                self.retry_waiting()?;
                let new = receipt.members.iter().filter(|m| m.newly_delivered).count();
                Ok(Received::Admitted { new })
            }
            Driven::Waiting(dependency) => {
                self.waiting.push((blob, op));
                Ok(Received::Waiting(dependency))
            }
        }
    }

    /// Re-drive waiting admissions until history stops changing. Verification
    /// is not repeated; only placement and publication remain.
    fn retry_waiting(&mut self) -> Result<()> {
        let mut first_error = None;
        let mut epoch = None;
        while epoch != Some(self.graph.history().epoch()) {
            epoch = Some(self.graph.history().epoch());
            for (blob, mut op) in std::mem::take(&mut self.waiting) {
                match self.graph.drive(&blob, &mut op) {
                    Ok(Driven::Done(_)) => {}
                    Ok(Driven::Waiting(_)) => self.waiting.push((blob, op)),
                    Err(e) => {
                        first_error.get_or_insert(e);
                    }
                }
            }
        }
        match first_error {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }

    /// Send `peer` everything it has not acknowledged. Returns the number of
    /// items sent. The cursor advances only over items the peer stored.
    pub fn sync_to(&mut self, peer: &mut Self) -> Result<usize> {
        let cursor = self.sent.get(&peer.author()).cloned().unwrap_or_default();
        let (mut plan, items) = self.export_items(&cursor)?;
        let mut result = Ok(items.len());
        let mut delivered = HashSet::new();
        for (index, item) in items.iter().enumerate() {
            match peer.receive_item(item) {
                Ok(_) => {
                    plan.acknowledge(index).unwrap();
                    delivered.extend(plan.id(index).cloned());
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.sent.insert(peer.author(), plan.cursor());
        // The peer need not send these items back.
        let back = peer.sent.get(&self.author()).cloned().unwrap_or_default();
        let mut back = peer.graph.export(&back);
        for index in 0..back.len() {
            if back.id(index).is_some_and(|id| delivered.contains(id)) {
                back.acknowledge(index).unwrap();
            }
        }
        peer.sent.insert(self.author(), back.cursor());
        peer.materialize()?;
        result
    }

    /// Receive everything `other` has that this replica has not been sent.
    pub fn merge(&mut self, other: &mut Self) -> Result<()> {
        other.sync_to(self).map(|_| ())
    }

    /// A new replica (fresh key) with this one's permissions and history.
    pub fn fork(&mut self) -> Result<Self> {
        let mut fork = Self::new();
        fork.set_write_permissions(self.permissions.clone())?;
        self.sync_to(&mut fork)?;
        Ok(fork)
    }
}
