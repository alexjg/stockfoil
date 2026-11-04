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
pub(crate) mod clock;
mod export;
mod local;
mod publish;
mod view;

pub use export::{ExportCursor, ExportItem, ExportPlan};
pub use local::{CommitBuilder, PayloadCheck, Prepared};
pub use publish::Receipt;
pub(crate) use publish::{Readiness, publish_verified, readiness};
pub use view::{Batch, Change, Changes, MaterializationCursor, ViewPoint, Vis, WritePermissions};

use crate::meta::depth;
use crate::{
    Admission, Author, CommitHash, DecodeItem, Error, ItemId, Next, PayloadId, ReceiveOptions,
    Signature,
};
use clock::{ChainId, Clock, ClockBuilder};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

pub(crate) use crate::verified::clock::NodeIdx;

#[derive(Debug)]
struct Node {
    hash: CommitHash,
    payload: PayloadId,
    parents_start: usize,
    parents_len: u32,
    chain: ChainId,
    depth: u8,
    admitted: bool,
    /// Member of some admitted fragment certificate.
    in_fragment: bool,
    /// Longest path since a node with an eagerly cached clock.
    uncached_depth: u8,
    clock: OnceLock<Arc<Clock>>,
}

/// Signature evidence retained for export: `author` signed `commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignatureEvidence {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

/// What a commit hash is associated with in this history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitInfo {
    /// The logical payload ID retained from first delivery.
    pub payload: PayloadId,
    pub author: Author,
    /// Authenticated history, as opposed to an unsigned local draft.
    pub admitted: bool,
}

/// One commit's delivery outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub hash: CommitHash,
    /// The retained payload ID. For a duplicate this is the earlier delivery's ID.
    pub payload: PayloadId,
    /// Whether this call delivered the commit to the history for the first time.
    pub newly_delivered: bool,
}

/// Which heads a view starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heads {
    /// Everything delivered, including unsigned local drafts.
    Delivered,
    /// Authenticated history only.
    Admitted,
}

static NEXT_HISTORY_ID: AtomicU64 = AtomicU64::new(1);

// History identity guards captured traversals and export plans. Exhaustion must
// leave the counter unchanged so a later allocation cannot reuse an old ID.
fn allocate_history_id(counter: &AtomicU64) -> u64 {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("history id space")
}

/// An authenticated commit history. See the [module docs](self).
#[derive(Debug)]
pub struct History {
    id: u64,
    nodes: Vec<Node>,
    parent_ids: Vec<NodeIdx>,
    by_hash: HashMap<CommitHash, NodeIdx>,
    chain_tips: Vec<NodeIdx>,
    chain_authors: Vec<Author>,
    chain_nodes: Vec<Vec<NodeIdx>>,
    author_chains: HashMap<Author, Vec<ChainId>>,
    heads: BTreeSet<CommitHash>,
    admitted_heads: BTreeSet<CommitHash>,
    /// Delivered local commits that are not yet admitted.
    drafts: BTreeSet<NodeIdx>,
    signed_drafts: BTreeSet<NodeIdx>,
    signatures: HashMap<NodeIdx, BTreeMap<Author, Signature>>,
    inventory: BTreeMap<export::Key, Arc<export::Certificate>>,
    epoch: u64,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            id: allocate_history_id(&NEXT_HISTORY_ID),
            nodes: Vec::new(),
            parent_ids: Vec::new(),
            by_hash: HashMap::new(),
            chain_tips: Vec::new(),
            chain_authors: Vec::new(),
            chain_nodes: Vec::new(),
            author_chains: HashMap::new(),
            heads: BTreeSet::new(),
            admitted_heads: BTreeSet::new(),
            drafts: BTreeSet::new(),
            signed_drafts: BTreeSet::new(),
            signatures: HashMap::new(),
            inventory: BTreeMap::new(),
            epoch: 0,
        }
    }

    // ---- topology queries ----

    /// Number of delivered commits, drafts included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Increments whenever anything is delivered or admitted. An operation
    /// waiting for a dependency only needs retrying after this changes.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn lookup(&self, hash: &CommitHash) -> Option<CommitInfo> {
        let node = &self.nodes[self.by_hash.get(hash)?.index()];
        Some(CommitInfo {
            payload: node.payload,
            author: self.chain_authors[node.chain as usize],
            admitted: node.admitted,
        })
    }

    /// Whether `hash` is authenticated history (not merely a local draft).
    pub fn is_admitted(&self, hash: &CommitHash) -> bool {
        self.by_hash
            .get(hash)
            .is_some_and(|&idx| self.nodes[idx.index()].admitted)
    }

    /// Ordered parents, including repeats.
    pub fn parents(&self, hash: &CommitHash) -> Option<impl ExactSizeIterator<Item = CommitHash>> {
        let idx = *self.by_hash.get(hash)?;
        Some(
            self.parent_nodes(idx)
                .iter()
                .map(|&p| self.nodes[p.index()].hash),
        )
    }

    /// Delivered or admitted heads, in hash order.
    pub fn heads(&self, which: Heads) -> impl Iterator<Item = CommitHash> + '_ {
        match which {
            Heads::Delivered => self.heads.iter(),
            Heads::Admitted => self.admitted_heads.iter(),
        }
        .copied()
    }

    /// Unadmitted local commits, parents first.
    pub fn drafts(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.drafts.iter().map(|idx| self.nodes[idx.index()].hash)
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    pub fn is_ancestor(&self, ancestor: &CommitHash, descendant: &CommitHash) -> bool {
        let (Some(&a), Some(&d)) = (self.by_hash.get(ancestor), self.by_hash.get(descendant))
        else {
            return false;
        };
        if a >= d {
            return false;
        }
        let chain = self.nodes[a.index()].chain;
        chain == self.nodes[d.index()].chain || self.clock(d).contains(chain, a)
    }

    /// Every delivered commit, parents first, drafts included.
    pub fn traverse(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.nodes.iter().map(|n| n.hash)
    }

    // ---- local commits, signing, and payload checks ----

    /// Start a local commit. Every parent must already be delivered (drafts
    /// included). Parent order and repeats are preserved in the identity.
    pub fn begin_commit(
        &self,
        author: Author,
        parents: Vec<CommitHash>,
    ) -> Result<CommitBuilder, Error> {
        if let Some(missing) = parents.iter().find(|p| !self.by_hash.contains_key(*p)) {
            return Err(Error::MissingParent(*missing));
        }
        Ok(CommitBuilder::new(author, parents))
    }

    /// Deliver an unadmitted local commit (see [`CommitBuilder`]).
    pub fn publish_commit(
        &mut self,
        prepared: Prepared,
        payload: PayloadId,
    ) -> Result<Delivered, Error> {
        if let Some(info) = self.lookup(&prepared.hash) {
            return Ok(Delivered {
                hash: prepared.hash,
                payload: info.payload,
                newly_delivered: false,
            });
        }
        if let Some(missing) = prepared
            .parents
            .iter()
            .find(|p| !self.by_hash.contains_key(*p))
        {
            return Err(Error::MissingParent(*missing));
        }
        self.insert(
            prepared.hash,
            prepared.author,
            &prepared.parents,
            payload,
            false,
        );
        self.bump_epoch();
        Ok(Delivered {
            hash: prepared.hash,
            payload,
            newly_delivered: true,
        })
    }

    /// Attach `author`'s signature to an unadmitted local commit, then admit
    /// every signed draft whose certificate is now complete. Returns what was
    /// admitted, possibly nothing (for example while a parent is still a draft).
    pub fn sign(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<Vec<ItemId>, Error> {
        let idx = *self.by_hash.get(&hash).ok_or(Error::UnknownCommit(hash))?;
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
        Admission::new(self.id, options)
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
    /// come parents first. All heads named by the cursor and target must be
    /// delivered; this is checked before any record is produced.
    pub fn changes(
        &self,
        cursor: &MaterializationCursor,
        target: &ViewPoint,
    ) -> Result<Changes, Error> {
        view::plan_changes(self, cursor, target)
    }

    /// Append up to `max` records to `out`.
    pub fn next_changes(
        &self,
        changes: &mut Changes,
        out: &mut Vec<Change>,
        max: usize,
    ) -> Result<Batch, Error> {
        view::emit_next_changes(self, changes, out, max)
    }

    // ---- internal topology ----

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    fn node(&self, idx: NodeIdx) -> &Node {
        &self.nodes[idx.index()]
    }

    fn author_of(&self, idx: NodeIdx) -> Author {
        self.chain_authors[self.node(idx).chain as usize]
    }

    fn parent_nodes(&self, idx: NodeIdx) -> &[NodeIdx] {
        let node = self.node(idx);
        &self.parent_ids[node.parents_start..node.parents_start + node.parents_len as usize]
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
        debug_assert!(!self.by_hash.contains_key(&hash));
        let idx = NodeIdx::new(u32::try_from(self.nodes.len()).expect("node index space"));
        let parents_start = self.parent_ids.len();
        self.parent_ids
            .extend(parents.iter().map(|p| self.by_hash[p]));
        let parent_nodes = self.parent_ids[parents_start..].to_vec();
        let chain = self.assign_chain(author, &parent_nodes, idx);
        let depth = depth(&hash);
        let uncached_depth = parent_nodes
            .iter()
            .map(|&p| self.node(p).uncached_depth)
            .max()
            .map_or(1, |d| d.saturating_add(1));
        let cache_clock = depth > 0 || parent_nodes.len() > 1 || uncached_depth >= 128;
        for p in parents {
            self.heads.remove(p);
        }
        self.heads.insert(hash);
        self.nodes.push(Node {
            hash,
            payload,
            parents_start,
            parents_len: u32::try_from(parent_nodes.len()).expect("parent count"),
            chain,
            depth: depth as u8,
            admitted: false,
            in_fragment: false,
            uncached_depth: if cache_clock { 0 } else { uncached_depth },
            clock: OnceLock::new(),
        });
        self.by_hash.insert(hash, idx);
        if cache_clock {
            self.clock(idx);
        }
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
        self.nodes[idx.index()].admitted = true;
        self.drafts.remove(&idx);
        self.signed_drafts.remove(&idx);
        for i in 0..self.node(idx).parents_len as usize {
            let parent = self.parent_ids[self.node(idx).parents_start + i];
            self.admitted_heads.remove(&self.nodes[parent.index()].hash);
        }
        self.admitted_heads.insert(self.node(idx).hash);
    }

    fn assign_chain(&mut self, author: Author, parents: &[NodeIdx], position: NodeIdx) -> ChainId {
        // Ordinary sequential edits extend a direct parent's chain.
        let direct = parents
            .iter()
            .filter_map(|&p| {
                let chain = self.node(p).chain;
                (self.chain_authors[chain as usize] == author
                    && self.chain_tips[chain as usize] == p)
                    .then_some(chain)
            })
            .min();
        // Any same-author tip in the parents' down-set can precede this node.
        let reusable = direct.or_else(|| {
            if parents.is_empty() {
                return None;
            }
            // Check only this author's chains, not every chain in the history.
            let history = self.clock_for_heads(parents);
            self.author_chains
                .get(&author)?
                .iter()
                .copied()
                .find(|&chain| history.contains(chain, self.chain_tips[chain as usize]))
        });
        if let Some(chain) = reusable {
            self.chain_tips[chain as usize] = position;
            self.chain_nodes[chain as usize].push(position);
            chain
        } else {
            let chain = ChainId::try_from(self.chain_tips.len()).expect("chain space");
            self.chain_tips.push(position);
            self.chain_authors.push(author);
            self.chain_nodes.push(vec![position]);
            self.author_chains.entry(author).or_default().push(chain);
            chain
        }
    }

    /// The ancestry (inclusive) of one node, cached on nodes that are merges,
    /// checkpoints, or the end of a long uncached run.
    fn clock(&self, head: NodeIdx) -> &Arc<Clock> {
        self.node(head).clock.get_or_init(|| {
            let mut builder = ClockBuilder::default();
            let mut stack = vec![head];
            let mut visited = std::collections::HashSet::new();
            while let Some(idx) = stack.pop() {
                if !visited.insert(idx) {
                    continue;
                }
                let node = self.node(idx);
                if idx != head
                    && let Some(clock) = node.clock.get()
                {
                    builder.merge(clock);
                } else {
                    builder.include(node.chain, idx);
                    stack.extend(self.parent_nodes(idx));
                }
            }
            Arc::new(builder.finish())
        })
    }

    fn clock_for_heads(&self, heads: &[NodeIdx]) -> Arc<Clock> {
        if let [head] = heads {
            return Arc::clone(self.clock(*head));
        }
        let mut builder = ClockBuilder::default();
        for &head in heads {
            builder.merge(self.clock(head));
        }
        Arc::new(builder.finish())
    }

    /// The ancestry of `heads`; every head must be delivered.
    fn history_clock(&self, heads: &[CommitHash]) -> Result<Arc<Clock>, Error> {
        let mut indices = heads
            .iter()
            .map(|h| self.by_hash.get(h).copied().ok_or(Error::UnknownCommit(*h)))
            .collect::<Result<Vec<_>, _>>()?;
        indices.sort_unstable();
        indices.dedup();
        if indices.len() == self.heads.len()
            && indices
                .iter()
                .all(|&i| self.heads.contains(&self.node(i).hash))
        {
            return Ok(Arc::new(Clock::from_tips(&self.chain_tips)));
        }
        Ok(self.clock_for_heads(&indices))
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
        clock::difference(&self.chain_nodes, included, &excluded, &mut members);
        members.sort_unstable();
        members
    }
}

#[cfg(test)]
mod tests;
