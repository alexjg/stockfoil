//! A multi-value register driven by Stockfoil's in-memory replica.
//! The CRDT is a Materializer; history, policy, cursors and delivery are library code.
pub mod codec;
mod subduction;
pub use stockfoil_subduction::{Channel, ChannelConnector, channel_pair};
pub use subduction::SubductionReplica;

use codec::{MAX_KEY_BYTES, RegisterCodec};
use ed25519_dalek::SigningKey;
use eyre::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use stockfoil::{
    Applied, Author, Bytes, Checkpoint, CommitHash, Document, Event, ExportCursor, ExportPlan,
    Follow, History, Materializer, Replica, Signed, ViewPoint, WritePermissions,
};

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
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for field in [&self.key, &self.value] {
            codec::put_uleb(&mut out, field.len() as u64);
            out.extend(field.as_bytes());
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        codec::decode_assignment(bytes)
    }
}

/// Materialized register values: for each key, the values of the causally
/// maximal *active* writes.
///
/// Every delivered write is retained whether or not it is active, because a
/// later permission change can activate it and because supersession follows
/// ancestry through inactive writes.
#[derive(Clone, Default)]
pub struct Register {
    /// Delivered writes per key, parents first (delivery order).
    by_key: HashMap<String, Vec<CommitHash>>,
    assignments: HashMap<CommitHash, Assignment>,
    active: HashSet<CommitHash>,
    values: BTreeMap<String, BTreeSet<String>>,
}

impl Register {
    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    /// Every key with at least one value.
    pub fn values(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.values
    }

    /// Recompute `key` from all retained active writes: hiding a write can
    /// reveal an older one that produced no event of its own.
    fn recompute(&mut self, history: &History, key: &str) {
        let mut maximal: Vec<CommitHash> = Vec::new();
        for hash in self.by_key.get(key).into_iter().flatten() {
            if self.active.contains(hash) {
                maximal.retain(|older| !history.is_ancestor(older, hash));
                maximal.push(*hash);
            }
        }
        let values: BTreeSet<_> = maximal
            .iter()
            .map(|h| self.assignments[h].value.clone())
            .collect();
        if values.is_empty() {
            self.values.remove(key);
        } else {
            self.values.insert(key.to_owned(), values);
        }
    }
}

impl Materializer for Register {
    type Error = Infallible;

    fn apply(
        &mut self,
        history: &History,
        events: Vec<Event<Bytes>>,
    ) -> Result<Applied, Infallible> {
        let mut applied = Applied::default();
        let mut affected = BTreeSet::new();
        for event in events {
            match event {
                Event::Reset => *self = Self::default(),
                Event::Deliver {
                    hash,
                    payload,
                    active,
                    ..
                } => {
                    let assignment = match Assignment::decode(&payload) {
                        Ok(a) if a.key.len() <= MAX_KEY_BYTES => a,
                        Ok(_) => {
                            applied = applied.reject(hash, "key too long");
                            continue;
                        }
                        Err(e) => {
                            applied = applied.reject(hash, e);
                            continue;
                        }
                    };
                    self.by_key
                        .entry(assignment.key.clone())
                        .or_default()
                        .push(hash);
                    affected.insert(assignment.key.clone());
                    self.assignments.insert(hash, assignment);
                    if active {
                        self.active.insert(hash);
                    }
                }
                Event::Activate { hash } => {
                    self.active.insert(hash);
                    affected.insert(self.assignments[&hash].key.clone());
                }
                Event::Retract { hash } => {
                    self.active.remove(&hash);
                    affected.insert(self.assignments[&hash].key.clone());
                }
                Event::Payload { .. } => unreachable!("never wanted"),
            }
        }
        for key in affected {
            self.recompute(history, &key);
        }
        Ok(applied)
    }

    fn supports_visibility_changes(&self) -> bool {
        true
    }
}

pub type RegisterDocument = Document<RegisterCodec>;
type RegisterReplica = Replica<RegisterCodec, Register>;

/// App-specific surface over one working replica. Per-peer cursor bookkeeping
/// is a repository concern, intentionally not part of Stockfoil.
pub struct MvRegisterMap {
    doc: RegisterReplica,
    signing_key: SigningKey,
    sent: HashMap<Author, ExportCursor>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Admitted {
        new: usize,
    },
    Waiting(CommitHash),
    Evicted {
        waiting: CommitHash,
        evicted: stockfoil::ItemId,
    },
}
impl Default for MvRegisterMap {
    fn default() -> Self {
        Self::new()
    }
}
impl MvRegisterMap {
    pub fn new() -> Self {
        Self::with_key(SigningKey::from_bytes(&rand::random()))
    }
    pub fn with_key(signing_key: SigningKey) -> Self {
        let doc = Replica::new(
            RegisterCodec::default(),
            Author::from(&signing_key),
            Register::default(),
        );
        Self {
            doc,
            signing_key,
            sent: HashMap::new(),
        }
    }
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }
    pub fn author(&self) -> Author {
        Author::from(&self.signing_key)
    }
    pub fn document(&self) -> &RegisterDocument {
        self.doc.document()
    }
    pub fn document_mut(&mut self) -> &mut RegisterDocument {
        self.doc.document_mut()
    }
    pub fn prepare_set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        ensure!(key.len() <= MAX_KEY_BYTES, "key longer than MAX_KEY_BYTES");
        Ok(self.doc.commit(&Assignment::new(key, value).encode())?.hash)
    }
    pub fn publish(&mut self) -> Result<Signed> {
        Ok(self.doc.sign_drafts(&self.signing_key)?)
    }
    pub fn set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        let hash = self.prepare_set(key, value)?;
        self.publish()?;
        Ok(hash)
    }
    pub fn write_permissions(&self) -> &WritePermissions {
        self.document().permissions()
    }
    pub fn set_write_permissions(&mut self, permissions: WritePermissions) -> Result<()> {
        self.doc.set_permissions(permissions)?;
        Ok(())
    }
    pub fn view_point(&self) -> ViewPoint {
        self.document().ledger().view(&Follow::Working)
    }
    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.doc.state().get(key)
    }
    pub fn values(&self) -> &BTreeMap<String, BTreeSet<String>> {
        self.doc.state().values()
    }
    /// Historical heads under this document's current permissions.
    pub fn get_at(&self, heads: &[CommitHash], key: &str) -> Result<BTreeSet<String>> {
        let (at, _) = self
            .doc
            .checkout(Register::default(), Follow::Pinned(heads.to_vec()))?;
        Ok(at.get(key))
    }
    pub fn checkpoint(&self) -> &Checkpoint {
        self.doc.materialization().checkpoint()
    }
    pub fn rematerialize(&mut self) -> Result<()> {
        self.doc.rebuild(Register::default())?;
        Ok(())
    }
    pub(crate) fn materialize(&mut self) -> Result<()> {
        self.doc.update()?;
        Ok(())
    }
    pub fn export_items(&self, cursor: &ExportCursor) -> Result<(ExportPlan, Vec<Vec<u8>>)> {
        let export = self.doc.export(cursor);
        let items = (0..export.len())
            .map(|i| export.item(i).map(|(_, bytes)| bytes))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((self.document().ledger().export(cursor), items))
    }
    pub fn receive(&mut self, item: &[u8]) -> Result<Received> {
        let (outcome, _) = self.doc.receive(item)?;
        Ok(match outcome {
            stockfoil::Received::Admitted {
                delivered, failed, ..
            } => {
                ensure!(failed.is_empty(), "parked item placement failed");
                Received::Admitted {
                    new: delivered.len(),
                }
            }
            stockfoil::Received::Waiting(h) => Received::Waiting(h),
            stockfoil::Received::Evicted { waiting, evicted } => {
                Received::Evicted { waiting, evicted }
            }
        })
    }
    pub fn waiting(&self) -> usize {
        self.document().ledger().parked()
    }
    pub fn sync_to(&mut self, peer: &mut Self) -> Result<usize> {
        let cursor = self.sent.get(&peer.author()).cloned().unwrap_or_default();
        let mut export = self.doc.export(&cursor);
        let mut delivered = HashSet::new();
        let mut sent = 0;
        for i in 0..export.len() {
            let (item, bytes) = export.item(i)?;
            peer.receive(&bytes)?;
            export.acknowledge(i)?;
            delivered.insert(item.id);
            sent += 1;
        }
        self.sent.insert(peer.author(), export.cursor());
        // Do not echo items the other replica just received.
        let back = peer.sent.get(&self.author()).cloned().unwrap_or_default();
        let mut export = peer.doc.export(&back);
        for i in 0..export.len() {
            if export.plan().id(i).is_some_and(|id| delivered.contains(id)) {
                export.acknowledge(i)?;
            }
        }
        peer.sent.insert(self.author(), export.cursor());
        Ok(sent)
    }
    pub fn merge(&mut self, other: &mut Self) -> Result<()> {
        other.sync_to(self).map(drop)
    }
    pub fn fork(&mut self) -> Result<Self> {
        let mut fork = Self::new();
        fork.set_write_permissions(self.write_permissions().clone())?;
        self.sync_to(&mut fork)?;
        Ok(fork)
    }
}
