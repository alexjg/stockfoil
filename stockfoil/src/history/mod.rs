//! Payload-independent authenticated history.
//!
//! [`History`] owns topology, authorship, logical payload IDs, admission state,
//! and the signature evidence needed to re-export what it admitted. It never
//! holds payload bytes, codecs, storage handles, callbacks, or a runtime, and it
//! never calls application code.
//!
//! Nodes are append-only and stored parents-first, so a node index is also a
//! topological position. Each node belongs to one single-author *chain*;
//! ancestry, views, and change sets are chain-prefix [`clock`]s.
mod admit;
mod ancestry;
pub(crate) mod checkpoint;
pub(crate) mod clock;
mod export;
mod local;
mod publish;
mod traversal;
pub(crate) mod view;

pub use ancestry::{Ancestry, Commits};
pub use checkpoint::{Checkpoint, Follow};
pub use export::{ExportCursor, ExportItem, ExportPlan};
pub use local::{CommitBuilder, PayloadCheck, Prepared};
pub use publish::Receipt;
pub(crate) use publish::{Readiness, publish_verified, readiness};
#[cfg(any(test, feature = "fuzzing"))]
pub use view::MaterializationCursor;
#[cfg(any(test, feature = "fuzzing"))]
pub use view::Vis;
#[cfg(any(test, feature = "fuzzing"))]
pub use view::{Batch, Change, Changes};
pub use view::{ViewPoint, WritePermissions};

use crate::meta::depth;
use crate::{
    Admission, Author, CommitHash, DecodeItem, Error, ItemId, Next, PayloadId, ReceiveOptions,
    Signature,
};
use clock::Clock;
use std::collections::BTreeSet;
use std::sync::Arc;

pub(crate) use crate::verified::clock::NodeIdx;

type Node =
    crate::verified::storage::Node<CommitHash, PayloadId, crate::verified::ancestry_cache::Cache>;

pub(crate) use crate::verified::history_data::SignatureEvidence;

/// What a commit hash is associated with in this history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitInfo {
    /// The logical payload ID retained from first delivery.
    pub payload: PayloadId,
    pub author: Author,
    /// Authenticated history, as opposed to an unsigned local draft.
    pub admitted: bool,
}

pub use crate::verified::publication::Delivered;

/// Which heads a view starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heads {
    /// Everything delivered, including unsigned local drafts.
    Delivered,
    /// Authenticated history only.
    Admitted,
}

pub use crate::verified::history::History;

impl History {
    // ---- topology queries ----

    /// Number of delivered commits, drafts included.
    pub fn len(&self) -> usize {
        self.topology.nodes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.topology.nodes().is_empty()
    }

    pub fn lookup(&self, hash: &CommitHash) -> Option<CommitInfo> {
        let node = &self.topology.nodes()[self.topology.by_hash().get(hash)?.index()];
        Some(CommitInfo {
            payload: node.payload,
            author: self.topology.chain_authors()[node.chain as usize],
            admitted: node.admitted,
        })
    }

    /// Whether `hash` is authenticated history (not merely a local draft).
    pub fn is_admitted(&self, hash: &CommitHash) -> bool {
        self.topology
            .by_hash()
            .get(hash)
            .is_some_and(|&idx| self.topology.nodes()[idx.index()].admitted)
    }

    /// Ordered parents, including repeats.
    pub fn parents(&self, hash: &CommitHash) -> Option<impl ExactSizeIterator<Item = CommitHash>> {
        let idx = *self.topology.by_hash().get(hash)?;
        Some(
            self.parent_nodes(idx)
                .iter()
                .map(|&p| self.topology.nodes()[p.index()].hash),
        )
    }

    /// Delivered or admitted heads, in hash order.
    pub fn heads(&self, which: Heads) -> impl Iterator<Item = CommitHash> + '_ {
        match which {
            Heads::Delivered => self.topology.heads().iter(),
            Heads::Admitted => self.admitted_heads.iter(),
        }
        .copied()
    }

    /// Unadmitted local commits, parents first.
    pub fn drafts(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.drafts
            .iter()
            .map(|idx| self.topology.nodes()[idx.index()].hash)
    }

    /// Every delivered commit, parents first, drafts included.
    pub fn traverse(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.topology.nodes().iter().map(|n| n.hash)
    }

    // ---- local commits, signing, and payload checks ----

    /// Attach `author`'s signature to an unadmitted local commit, then admit
    /// every signed draft whose certificate is now complete. Returns what was
    /// admitted, possibly nothing (for example while a parent is still a draft).
    pub fn sign(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<Vec<ItemId>, Error> {
        let idx = *self
            .topology
            .by_hash()
            .get(&hash)
            .ok_or(Error::UnknownCommit(hash))?;
        if self.node(idx).admitted {
            return Err(Error::NotDraft(hash));
        }
        crate::evidence::verify_signature(&author, &hash, &signature)?;
        self.signatures
            .entry(idx)
            .or_default()
            .insert(author, signature);
        self.signed_drafts.insert(idx);
        let admitted = admit::admit_ready_drafts(self);
        if !admitted.is_empty() {
            self.bump_epoch();
        }
        Ok(admitted)
    }

    /// Start checking a delivered commit's payload, using the parents and
    /// author recorded in history. Storage needs no integrity guarantees of its
    /// own: whatever it returns is checked here before it is trusted.
    pub fn check_payload(&self, commit: &CommitHash) -> Result<PayloadCheck, Error> {
        let info = self.lookup(commit).ok_or(Error::UnknownCommit(*commit))?;
        let parents = self.parents(commit).ok_or(Error::UnknownCommit(*commit))?;
        Ok(PayloadCheck::new(*commit, info.author, parents))
    }

    // ---- receiving and decoding stored items ----

    /// Start receiving one stored item (header and body, as one blob). The
    /// host that answers the operation's requests decides which item that is.
    pub fn receive(&self, options: ReceiveOptions) -> Admission {
        Admission::new(self.id(), options)
    }

    /// Do the history-dependent work of an admission: check dependencies and
    /// placement, or publish atomically once payloads are bound. No application
    /// code runs here.
    pub fn advance<'a>(&mut self, op: &'a mut Admission) -> Next<'a> {
        op.advance(self)
    }

    /// Start decoding the canonical payloads of `wanted` members (member
    /// index to expected commit hash) from a stored wire item.
    ///
    /// This supports hosts whose payload IDs resolve to `(item, member index)`:
    /// retain compressed items rather than separate expanded canonical payloads,
    /// and extract selected members through caller-sized buffers on demand.
    ///
    /// Fails immediately with [`Error::UnknownCommit`] if any wanted commit
    /// has not been delivered to this history. Once created, the operation
    /// owns its [`PayloadCheck`]s and buffers and never borrows or needs
    /// [`History`] again.
    pub fn decode_item(
        &self,
        options: ReceiveOptions,
        wanted: impl IntoIterator<Item = (usize, CommitHash)>,
    ) -> Result<DecodeItem, Error> {
        DecodeItem::new(self, options, wanted)
    }

    // ---- export ----

    /// Plan an export relative to what a destination has acknowledged.
    pub fn export(&self, cursor: &ExportCursor) -> ExportPlan {
        export::plan_export(self, cursor)
    }

    /// Build the metadata and member list for one planned item.
    pub fn export_item(&self, plan: &ExportPlan, index: usize) -> Result<ExportItem, Error> {
        export::build_export_item(self, plan, index)
    }

    // ---- views and changes ----

    /// The working view: every delivered commit, drafts included.
    pub fn view_point(&self, permissions: &WritePermissions) -> ViewPoint {
        view::view(self, Heads::Delivered, permissions)
    }

    /// Authenticated history only.
    pub fn signed_view_point(&self, permissions: &WritePermissions) -> ViewPoint {
        view::view(self, Heads::Admitted, permissions)
    }

    /// Start a change traversal from a materializer's cursor to `target`.
    ///
    /// It captures the currently delivered history. A commit produces one record
    /// if it was not delivered to this materializer before, or if its
    /// participation differs between the cursor's view and `target`. Records
    /// come ancestors first, with the smallest hash first among ready records,
    /// independent of delivery order and batching. All heads named by the
    /// cursor and target must be delivered; this is checked before any record
    /// is produced.
    #[cfg(all(test, not(feature = "fuzzing")))]
    pub(crate) fn changes(
        &self,
        cursor: &MaterializationCursor,
        target: &ViewPoint,
    ) -> Result<Changes, Error> {
        view::plan_changes(self, cursor, target)
    }

    /// Raw traversal entry point for the independent fuzz model only.
    #[cfg(feature = "fuzzing")]
    #[doc(hidden)]
    pub fn changes(
        &self,
        cursor: &MaterializationCursor,
        target: &ViewPoint,
    ) -> Result<Changes, Error> {
        view::plan_changes(self, cursor, target)
    }

    /// Append up to `max` records to `out`.
    #[cfg(all(test, not(feature = "fuzzing")))]
    pub(crate) fn next_changes(
        &self,
        changes: &mut Changes,
        out: &mut Vec<Change>,
        max: usize,
    ) -> Result<Batch, Error> {
        view::emit_next_changes(self, changes, out, max)
    }

    /// Raw traversal entry point for the independent fuzz model only.
    #[cfg(feature = "fuzzing")]
    #[doc(hidden)]
    pub fn next_changes(
        &self,
        changes: &mut Changes,
        out: &mut Vec<Change>,
        max: usize,
    ) -> Result<Batch, Error> {
        view::emit_next_changes(self, changes, out, max)
    }

    // ---- internal topology ----

    fn node(&self, idx: NodeIdx) -> &Node {
        &self.topology.nodes()[idx.index()]
    }

    fn author_of(&self, idx: NodeIdx) -> Author {
        self.topology.chain_authors()[self.node(idx).chain as usize]
    }

    fn parent_nodes(&self, idx: NodeIdx) -> &[NodeIdx] {
        let node = self.node(idx);
        crate::verified::storage::parent_slice(node, self.topology.parent_ids())
    }

    fn bump_epoch(&mut self) {
        self.epoch = self.epoch.checked_add(1).expect("epoch space");
    }

    /// Append a new node whose parents are all delivered.
    fn insert(
        &mut self,
        hash: CommitHash,
        author: Author,
        parents: &[CommitHash],
        payload: PayloadId,
        admitted: bool,
    ) -> NodeIdx {
        let idx = self
            .topology
            .insert(hash, author, parents, payload, depth(&hash) as u8)
            .unwrap_or_else(|error| match error {
                crate::verified::topology::InsertError::Missing(parent) => {
                    panic!("undelivered parent: {parent:?}")
                }
                crate::verified::topology::InsertError::Duplicate => {
                    panic!("commit already delivered")
                }
                crate::verified::topology::InsertError::Capacity => panic!("topology capacity"),
            });
        if admitted {
            self.mark_admitted(idx);
        } else {
            self.drafts.insert(idx);
        }
        idx
    }

    fn mark_admitted(&mut self, idx: NodeIdx) {
        if self.node(idx).admitted {
            return;
        }
        debug_assert!(
            self.parent_nodes(idx)
                .iter()
                .all(|&p| self.node(p).admitted)
        );
        self.topology.mark_admitted(idx);
        self.drafts.remove(&idx);
        self.signed_drafts.remove(&idx);
        for i in 0..self.node(idx).parents_len as usize {
            let parent = self.topology.parent_ids()[self.node(idx).parents_start + i];
            self.admitted_heads
                .remove(&self.topology.nodes()[parent.index()].hash);
        }
        self.admitted_heads.insert(self.node(idx).hash);
    }

    /// The ancestry (inclusive) of one node, cached on nodes that are merges,
    /// checkpoints, or the end of a long uncached run.
    fn clock(&self, head: NodeIdx) -> &Arc<Clock> {
        self.topology.clock(head)
    }

    fn clock_for_heads(&self, heads: &[NodeIdx]) -> Arc<Clock> {
        self.topology.clock_for_heads(heads)
    }

    fn contains(&self, clock: &Clock, idx: NodeIdx) -> bool {
        clock.contains(self.node(idx).chain, idx)
    }

    /// Depth-based stops reached from `head` without passing through another stop.
    fn fragment_boundary(&self, head: NodeIdx, level: usize) -> BTreeSet<NodeIdx> {
        let mut seen = std::collections::HashSet::new();
        let mut boundary = BTreeSet::new();
        let mut stack = vec![head];
        while let Some(idx) = stack.pop() {
            if !seen.insert(idx) {
                continue;
            }
            if idx != head && self.node(idx).depth as usize >= level {
                boundary.insert(idx);
            } else {
                stack.extend(self.parent_nodes(idx));
            }
        }
        boundary
    }

    /// Ancestry of `head` minus the ancestry of `boundary`, in node order.
    fn fragment_members(&self, head: NodeIdx, boundary: &[NodeIdx]) -> Vec<NodeIdx> {
        let included = self.clock(head);
        let excluded = self.clock_for_heads(boundary);
        let mut members = Vec::new();
        clock::difference(
            self.topology.chain_nodes(),
            included,
            &excluded,
            &mut members,
        );
        members.sort_unstable();
        members
    }
}

#[cfg(test)]
mod canonical_changes_tests;
#[cfg(test)]
mod reference_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod authorized_visibility;
