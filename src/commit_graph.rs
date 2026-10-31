use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::ops::{ControlFlow, Index, IndexMut, Range};
#[cfg(not(feature = "std"))]
use alloc::boxed::Box;
#[cfg(not(feature = "std"))]
use once_cell::race::OnceBox as OnceLock;
#[cfg(feature = "std")]
use std::sync::OnceLock;

use crate::{
    collections::{HashMap, HashSet},
    Author, BlobHash, CommitContents, CommitDiff, CommitHash, CommitMeta, DiffError, ExportError,
    FragmentRange, FragmentReadError, IngestRejection, IngestReport, ItemId,
    MaterializationChange, MaterializationCursor, SedimentreeView, Signature, SignatureError,
    SignatureEvidence, UnverifiedItem, ViewPoint, ViewPointDiff, Vis, VisDiff, WritePermission,
    ancestor_clock::{AncestorClock, ChainTime, ClockBuilder},
    pending_ingest::{PendingIngest, PreparedItem},
    sedimentree::MinimalCover,
};

/// A single commit graph
#[derive(Clone)]
pub struct CommitGraph<P> {
    nodes: NodeStore,
    fragments: FragmentStore,
    store: HashMap<NodeIdx, P>,
    by_hash: HashMap<CommitHash, NodeIdx>,
    heads: BTreeSet<CommitHash>,
    permissions: HashMap<Author, WritePermission>,
    // Local commits which we are waiting for the caller to provide signatures
    // over
    pending_local: HashSet<CommitHash>,
    // Verified received items and owned payloads, not yet delivered history.
    pending_ingest: PendingIngest<P>,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    meta: CommitMeta,
    // Immutable graph-local, single-author chain; NodeIdx is its position.
    chain: usize,
    clock: OnceLock<Arc<AncestorClock>>,
    // Longest path since an eagerly cached clock. Query caching can only shorten it.
    uncached_depth: u8,
    depth: usize,
    parents: Range<usize>,
    // Unlike parents, children grow as later commits arrive. Unique, in insertion order.
    children: Vec<NodeIdx>,
    signatures: BTreeMap<Author, Signature>,
    // Fragments which this commit is a head of. Indexed by depth, ascending
    headed_fragments: Vec<FragmentIdx>,
}

impl Node {
    // The deepest fragment which this commit is a head of
    fn canonical_fragment(&self) -> Option<FragmentIdx> {
        self.headed_fragments.last().copied()
    }

    fn signature_evidence(&self) -> impl Iterator<Item = SignatureEvidence> + '_ {
        self.signatures
            .iter()
            .map(|(&author, &signature)| SignatureEvidence {
                author,
                commit: *self.meta.hash(),
                signature,
            })
    }

    pub(crate) fn hash(&self) -> &CommitHash {
        self.meta.hash()
    }
}

impl<P: Clone> Default for CommitGraph<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Clone> CommitGraph<P> {
    pub fn new() -> Self {
        Self {
            nodes: NodeStore::default(),
            fragments: FragmentStore::default(),
            store: HashMap::new(),
            by_hash: HashMap::new(),
            heads: BTreeSet::new(),
            permissions: HashMap::new(),
            pending_local: HashSet::new(),
            pending_ingest: PendingIngest::default(),
        }
    }

    pub fn parents(&self, commit: &CommitHash) -> impl Iterator<Item = &'_ CommitHash> + use<'_, P> {
        self.by_hash.get(commit)
            .into_iter()
            .map(|idx| self.parents_by_node_idx(*idx))
            .flatten()
            .map(|idx| self.nodes[idx].meta.hash())
    }

    pub(crate) fn parents_by_node_idx(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> + use<'_, P> {
        self.nodes.parents(node_idx).iter().copied()
    }

    /// Return direct children once each, in insertion order, in O(number of children).
    pub fn children(&self, commit: &CommitHash) -> impl Iterator<Item = &'_ CommitHash> {
        self.by_hash.get(commit)
            .into_iter()
            .map(|idx| self.children_by_node_idx(*idx))
            .flatten()
            .map(|idx| self.nodes[idx].meta.hash())
    }

    pub(crate) fn children_by_node_idx(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        self.nodes.children(node_idx).iter().copied()
    }

    pub fn roots(&self) -> impl Iterator<Item = &'_ CommitHash> {
        self.nodes
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, node)| node.parents.is_empty().then_some(NodeIdx(i)))
            .map(|idx| self.nodes[idx].meta.hash())
    }

    /// Hashes of commits with no children in the graph, in hash order.
    pub fn heads(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.heads.iter().copied()
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`
    pub fn is_ancestor(&self, ancestor: &CommitHash, descendant: &CommitHash) -> bool {
        let (Some(&ancestor), Some(&descendant)) =
            (self.by_hash.get(ancestor), self.by_hash.get(descendant))
        else {
            return false;
        };

        // Parents always have smaller indices than their children.
        if ancestor >= descendant {
            return false;
        }
        let chain = self.nodes[ancestor].chain;
        // Chain members are totally ordered by ancestry, even when consecutive
        // members are not linked by a direct parent edge.
        chain == self.nodes[descendant].chain
            || self.nodes.clock(descendant).contains(chain, ancestor)
    }

    /// Admit a verified item, or queue it until all dependencies are admitted.
    ///
    /// Queued items are not delivered history: they do not affect heads, views,
    /// materialization, or export. Delivered unsigned local drafts do not satisfy
    /// dependencies. Every admission, including local signing, automatically
    /// drains the items it unblocks.
    ///
    /// A fragment's borrowed members ([`crate::VerifiedItem::unauthenticated`])
    /// need not be admitted, only authenticated by some verified item. Each
    /// queued item's own authentication counts as soon as it is queued, so
    /// queuing one item can admit others, and mutual borrowing cannot deadlock.
    ///
    /// The `payload` callback runs now, even when admission is deferred, for each
    /// member not already delivered. Its results are owned by the queue; the
    /// callback itself is never retained. Exact queued duplicates reuse their
    /// payloads. Existing delivered payloads always win, including local copies
    /// inserted after queueing. Payload-byte validation is the caller's duty.
    ///
    /// Callbacks run before mutation; a panic leaves both graph and queue
    /// unchanged. A ready invalid fragment returns an error without mutation or
    /// callbacks. A queued fragment which later proves invalid is removed and
    /// reported in [`IngestReport::rejected`] by the operation that unblocked it;
    /// valid dependencies and other ready items are not rolled back.
    ///
    /// The queue currently has no resource limits.
    pub fn ingest(
        &mut self,
        item: crate::VerifiedItem,
        mut payload: impl FnMut(&CommitMeta) -> P,
    ) -> Result<IngestReport, crate::IngestError> {
        let missing: Vec<_> = item
            .dependencies()
            .filter(|hash| !self.has_commit(hash))
            .copied()
            .collect();
        let missing_auth: Vec<_> = item
            .unauthenticated()
            .filter(|hash| !self.is_authenticated(hash))
            .copied()
            .collect();
        let queued = !missing.is_empty() || !missing_auth.is_empty();
        if missing.is_empty() {
            self.validate_fragment_admission(&item)?;
        }
        if queued && self.pending_ingest.contains(&item) {
            return Ok(self.ingest_report());
        }
        let payloads = item
            .commits
            .iter()
            .map(|meta| (!self.by_hash.contains_key(meta.hash())).then(|| payload(meta)))
            .collect();
        let prepared = PreparedItem { item, payloads };
        if queued {
            // A queued item's own authentication is usable immediately: it may
            // unblock items borrowing its members, including ones it waits on.
            let authenticated: Vec<_> = prepared
                .item
                .authenticated()
                .filter(|hash| !self.has_commit(hash))
                .copied()
                .collect();
            self.pending_ingest.push(prepared, missing, missing_auth);
            for hash in authenticated {
                self.pending_ingest.authenticate(hash);
            }
            if self.pending_ingest.has_ready() {
                return Ok(self.finish_ingest(IngestReport::default()));
            }
            return Ok(self.ingest_report());
        }
        let mut report = IngestReport::default();
        self.admit_prepared(prepared, &mut report);
        Ok(self.finish_ingest(report))
    }

    /// Number of received items waiting for admitted dependencies, excluding
    /// local drafts. Repeated copies of the same queued item count only once.
    pub fn pending_items(&self) -> usize {
        self.pending_ingest.len()
    }

    /// Distinct unadmitted dependencies of queued received items, in unspecified
    /// order. Some may already be delivered as unsigned local drafts.
    pub fn missing_dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.pending_ingest.missing_dependencies()
    }

    /// Distinct borrowed members of queued items which no verified item has
    /// authenticated yet, in unspecified order. Any item authenticating them
    /// unblocks those waiters, even if that item is itself still queued.
    pub fn missing_authentication(&self) -> impl Iterator<Item = &CommitHash> {
        self.pending_ingest.missing_authentication()
    }

    /// Admitted, or authenticated by the signatures of a verified item this
    /// graph has queued. Only the latter can be borrowed before admission.
    fn is_authenticated(&self, hash: &CommitHash) -> bool {
        self.has_commit(hash) || self.pending_ingest.is_authenticated(hash)
    }

    fn ingest_report(&self) -> IngestReport {
        IngestReport {
            pending: self.pending_items(),
            ..IngestReport::default()
        }
    }

    fn admit_prepared(&mut self, prepared: PreparedItem<P>, report: &mut IngestReport) {
        // Verified members are parents-first. Another item or a local edit may
        // have delivered a member since this item's payloads were prepared.
        for (meta, data) in prepared.item.commits.iter().zip(prepared.payloads) {
            if !self.by_hash.contains_key(meta.hash()) {
                self.insert_commit(meta.clone(), data.expect("undelivered member has a payload"));
            }
        }
        self.admit_certificate(prepared.item, report);
    }

    // Stateless verification proves member reachability and depth, and derives
    // exact external parents. Placement in boundary history and boundary stops
    // reached through omitted metadata remain graph-dependent.
    fn validate_fragment_admission(
        &self,
        item: &crate::VerifiedItem,
    ) -> Result<(), crate::IngestError> {
        let ItemId::Fragment(range) = &item.id else {
            return Ok(());
        };
        let invalid = crate::IngestError::InvalidFragment;
        let boundary_nodes = range
            .boundary
            .iter()
            .map(|h| self.by_hash[h])
            .collect::<Vec<_>>();
        let excluded = self.nodes.clock_for_heads(&boundary_nodes);
        let in_excluded = |hash: &CommitHash| {
            self.by_hash
                .get(hash)
                .is_some_and(|&idx| excluded.contains(self.nodes[idx].chain, idx))
        };
        if item.commits.iter().any(|meta| in_excluded(meta.hash())) {
            return Err(invalid);
        }
        let mut stack = Vec::new();
        for hash in item.external_parents() {
            if !in_excluded(hash) {
                return Err(invalid);
            }
            stack.push(self.by_hash[hash]);
        }
        let mut boundary = BTreeSet::new();
        let mut visited = HashSet::new();
        let depth = sedimentree_depth(&range.head);
        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            if self.nodes[idx].depth >= depth {
                boundary.insert(*self.nodes[idx].meta.hash());
            } else {
                stack.extend(self.parents_by_node_idx(idx));
            }
        }
        if boundary != range.boundary {
            return Err(invalid);
        }
        Ok(())
    }

    // Only called for a new commit once all parents are present. Received
    // members are verified and topologically sorted; local callers check first.
    fn insert_commit(&mut self, meta: CommitMeta, data: P) {
        debug_assert!(!self.by_hash.contains_key(meta.hash()));
        debug_assert!(
            meta.parents()
                .iter()
                .all(|hash| self.by_hash.contains_key(hash))
        );
        let hash = *meta.hash();
        let start = self.nodes.parent_ids.len();
        self.nodes
            .parent_ids
            .extend(meta.parents().iter().map(|hash| self.by_hash[hash]));
        let parents = start..self.nodes.parent_ids.len();
        let node_idx = NodeIdx(self.nodes.entries.len());
        let chain = self
            .nodes
            .assign_chain(*meta.author(), parents.clone(), node_idx);
        let depth = sedimentree_depth(&hash);
        let uncached_depth = self.nodes.parent_ids[parents.clone()]
            .iter()
            .map(|&idx| self.nodes[idx].uncached_depth)
            .max()
            .unwrap_or(0)
            + 1;
        // Checkpoints are useful regardless of fragment certification. Cache
        // merges too, and bound reconstruction along checkpoint-free runs.
        let cache_clock = depth > 0 || parents.len() > 1 || uncached_depth >= 128;
        self.store.insert(node_idx, data);
        for parent in meta.parents() {
            self.heads.remove(parent);
            let children = &mut self.nodes[self.by_hash[parent]].children;
            // Repeated envelope parents must not duplicate the reverse edge.
            if children.last() != Some(&node_idx) {
                children.push(node_idx);
            }
        }
        self.heads.insert(hash);
        self.nodes.entries.push(Node {
            depth,
            meta,
            chain,
            clock: OnceLock::new(),
            uncached_depth: if cache_clock { 0 } else { uncached_depth },
            parents,
            children: Vec::new(),
            signatures: BTreeMap::new(),
            headed_fragments: Vec::new(),
        });
        self.by_hash.insert(hash, node_idx);
        if cache_clock {
            self.nodes.clock(node_idx);
        }
    }

    // Build headed fragments from lower to higher levels.
    fn insert_fragments(&mut self, head: NodeIdx) {
        let depth = self.nodes[head].depth;
        for level in 1..=depth {
            let boundary = self.nodes.fragment_boundary(head, level);
            let start = self.fragments.boundary_ids.len();
            self.fragments.boundary_ids.extend(boundary);
            let idx = FragmentIdx(self.fragments.entries.len());
            self.fragments.entries.push(Fragment {
                head,
                level,
                boundary: start..self.fragments.boundary_ids.len(),
                signature_boundary: BTreeMap::new(),
                borrowed: Vec::new(),
            });
            self.nodes[head].headed_fragments.push(idx);
        }
    }

    // Return a view of this commit graph as a sedimentree
    pub fn as_sedimentree(&self) -> SedimentreeView<'_> {
        SedimentreeView {
            nodes: &self.nodes,
            fragments: &self.fragments,
            publishable: self.publishable_nodes(),
        }
    }

    /// Visit a canonical fragment's normalized members exactly once: include the
    /// head, but exclude boundaries and all their ancestors. Parents within the fragment precede their children; among
    /// currently eligible commits, the smallest full commit hash is visited first.
    /// This order is independent of ingestion order and local node indices.
    pub fn traverse_fragment<'graph>(
        &'graph self,
        range: &FragmentRange,
        mut visit: impl FnMut(&'graph CommitMeta, &'graph P) -> ControlFlow<()>,
    ) -> Result<(), FragmentReadError> {
        let head = *self
            .by_hash
            .get(&range.head)
            .ok_or(FragmentReadError::MissingCommit(range.head))?;
        if self.nodes[head].depth == 0 {
            return Err(FragmentReadError::NotFragment(range.head));
        }
        if !self.has_commit(&range.head) {
            return Err(FragmentReadError::NotPublishable(range.head));
        }
        let fragment_idx = self.nodes[head]
            .canonical_fragment()
            .ok_or(FragmentReadError::NotFragment(range.head))?;
        let boundary: BTreeSet<_> = self
            .fragments
            .boundary(fragment_idx)
            .iter()
            .map(|&idx| *self.nodes[idx].meta.hash())
            .collect();
        if boundary != range.boundary {
            return Err(FragmentReadError::BoundaryMismatch(range.head));
        }

        // Use the indexed range, not an unchecked caller-supplied boundary.
        // Clocks subtract the complete boundary history, including bypass paths.
        let members = self.fragments.members(&self.nodes, fragment_idx);
        let mut remaining = HashMap::new();
        let mut children: HashMap<NodeIdx, Vec<NodeIdx>> = HashMap::new();
        let mut ready = BTreeSet::new();
        for &idx in &members {
            // External parents are admitted dependencies, not payload members.
            // Repeated internal parent edges count once.
            let parents: HashSet<_> = self
                .parents_by_node_idx(idx)
                .filter(|parent| members.contains(parent))
                .collect();
            remaining.insert(idx, parents.len());
            if parents.is_empty() {
                ready.insert(*self.nodes[idx].meta.hash());
            }
            for parent in parents {
                children.entry(parent).or_default().push(idx);
            }
        }

        // Kahn's algorithm with a full-hash ready queue gives the lexicographically
        // smallest topological order, without dependence on hash-map iteration.
        let mut emitted = 0;
        while let Some(hash) = ready.pop_first() {
            let idx = self.by_hash[&hash];
            if visit(&self.nodes[idx].meta, &self.store[&idx]).is_break() {
                return Ok(());
            }
            emitted += 1;
            if let Some(children) = children.get(&idx) {
                for &child in children {
                    let count = remaining.get_mut(&child).unwrap();
                    *count -= 1;
                    if *count == 0 {
                        ready.insert(*self.nodes[child].meta.hash());
                    }
                }
            }
        }
        debug_assert_eq!(emitted, members.len(), "delivered history must be acyclic");
        Ok(())
    }

    /// Borrow a publishable delivered commit's metadata and payload.
    /// Returns `None` for unknown or withheld commits. The commit need
    /// not still be loose: a previously advertised entry remains readable after
    /// a fragment covers it. Does not clone payloads or supply signature evidence.
    /// Readability does not guarantee standalone exportability; see [`Self::export`].
    pub fn publishable_commit(&self, hash: &CommitHash) -> Option<(&CommitMeta, &P)> {
        let idx = *self.by_hash.get(hash)?;
        (!self.pending_local.contains(hash)).then(|| (&self.nodes[idx].meta, &self.store[&idx]))
    }

    /// Read a cover item or its signed head announcement at any depth. Fragment
    /// certificates follow stored signature-boundary references, never a search
    /// for author coverage. A head announcement is not a fragment certificate.
    pub fn export(&self, id: &ItemId) -> Result<UnverifiedItem, ExportError> {
        let cover = self.minimal_cover();
        let exportable = match id {
            ItemId::Commit(hash) => {
                cover.loose_commits.iter().any(|c| &c.commit == hash)
                    || cover.fragments.iter().any(|f| &f.range.head == hash)
            }
            ItemId::Fragment(range) => cover.fragments.iter().any(|f| &f.range == range),
        };
        if !exportable {
            return Err(ExportError::NotInMinimalCover(id.clone()));
        }
        Ok(self.export_certified(id))
    }

    // The caller has checked that this is a current cover entry or its signed
    // head announcement. Do not recompute the cover for each exported item.
    fn export_certified(&self, id: &ItemId) -> UnverifiedItem {
        match id {
            ItemId::Commit(hash) => {
                let node = &self.nodes[self.by_hash[hash]];
                UnverifiedItem::Commit {
                    meta: node.meta.clone(),
                    signatures: node
                        .signature_evidence()
                        .filter(|evidence| &evidence.author == node.meta.author())
                        .collect(),
                }
            }
            ItemId::Fragment(range) => {
                let head = self.by_hash[&range.head];
                let fragment = self.nodes[head].canonical_fragment().unwrap();
                let mut commits = Vec::new();
                self.traverse_fragment(range, |meta, _| {
                    commits.push(meta.clone());
                    ControlFlow::Continue(())
                })
                .expect("cover fragment is indexed");
                UnverifiedItem::Fragment {
                    range: range.clone(),
                    commits,
                    signatures: self.fragments[fragment]
                        .signature_boundary
                        .iter()
                        .flat_map(|(&author, targets)| {
                            targets.iter().map(move |&idx| {
                                let node = &self.nodes[idx];
                                SignatureEvidence {
                                    author,
                                    commit: *node.meta.hash(),
                                    signature: node.signatures[&author],
                                }
                            })
                        })
                        .collect(),
                    borrowed: self.fragments[fragment]
                        .borrowed
                        .iter()
                        .map(|&idx| *self.nodes[idx].meta.hash())
                        .collect(),
                }
            }
        }
    }

    /// Export the changes in the sedimentree encoding of this graph since
    /// `since`.
    pub fn export_changes(
        &self,
        since: &crate::ExportCursor,
    ) -> (crate::ExportChanges, crate::ExportCursor) {
        let cover = self.minimal_cover();
        let inventory: BTreeSet<_> = cover
            .loose_commits
            .into_iter()
            .map(|commit| ItemId::Commit(commit.commit))
            .chain(cover.fragments.into_iter().map(|fragment| ItemId::Fragment(fragment.range)))
            .collect();
        let added = inventory
            .difference(&since.inventory)
            .map(|id| self.export_certified(id))
            .collect();
        let retired = since.inventory.difference(&inventory).cloned().collect();
        (
            crate::ExportChanges { added, retired },
            crate::ExportCursor { inventory },
        )
    }

    /// Compute a complete export cover from retained fragment certificates and
    /// signed standalone commits. Structural subfragments alone do not suffice.
    pub fn minimal_cover(&self) -> MinimalCover {
        let summary = self
            .fragments
            .summary(&self.nodes, &self.publishable_nodes(), true, true);
        MinimalCover {
            loose_commits: summary.loose_commits,
            fragments: summary.fragments,
        }
    }

    /// Whether metadata is authenticated and parent-closed, not merely local.
    pub fn has_commit(&self, hash: &CommitHash) -> bool {
        self.by_hash.contains_key(hash) && !self.pending_local.contains(hash)
    }

    /// Merge only the other graph's admitted history, not its queue, drafts, or
    /// permissions. Also reports queued items unblocked at this destination.
    pub fn merge(&mut self, other: &Self) -> IngestReport {
        let cover = other.minimal_cover();
        let items = cover
            .loose_commits
            .into_iter()
            .map(|commit| ItemId::Commit(commit.commit))
            .chain(cover.fragments.into_iter().map(|fragment| ItemId::Fragment(fragment.range)));
        let mut report = self.ingest_report();
        for id in items {
            let item = other
                .export(&id)
                .expect("current cover entry")
                .verify()
                .expect("certified cover entry");
            let result = self
                .ingest(item, |meta| other.store[&other.by_hash[meta.hash()]].clone())
                .expect("certified cover entry");
            report.admitted.extend(result.admitted);
            report.rejected.extend(result.rejected);
            report.pending = result.pending;
        }
        report
    }

    /// Visit each delivered commit once, parents before children, including local drafts.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
    ///
    /// This is a raw delivery traversal. For CRDT materialization, prefer
    /// [`Self::visit_changes`] to also handle changes in participation.
    pub fn traverse<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(&self, mut traversal: F) {
        // Only ready commits are inserted, so node order is already topological.
        for (idx, node) in &self.nodes {
            if traversal(&node.meta, &self.store[&idx]).is_break() {
                break;
            }
        }
    }

    /// Visit delivered commits excluding `since` and their ancestors, parents first.
    /// An empty `since` visits all delivered commits. Unknown hashes are ignored.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
    ///
    /// This lower-level API does not report permission changes. Prefer
    /// [`Self::visit_changes`] for incremental materialization.
    pub fn traverse_since<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        since: &[CommitHash],
        mut traversal: F,
    ) {
        let mut excluded = vec![false; self.nodes.entries.len()];
        let mut stack = since
            .iter()
            .filter_map(|hash| self.by_hash.get(hash).copied())
            .collect::<Vec<_>>();
        while let Some(idx) = stack.pop() {
            if excluded[idx.0] {
                continue;
            }
            excluded[idx.0] = true;
            stack.extend(self.parents_by_node_idx(idx));
        }

        for (idx, node) in &self.nodes {
            if excluded[idx.0] {
                continue;
            }
            if traversal(&node.meta, &self.store[&idx]).is_break() {
                break;
            }
        }
    }

    /// Prepare an authored commit without signing or ingesting it.
    /// Sign the raw 32 bytes of its hash with Ed25519.
    pub fn commit_by(&self, author: Author, contents: &[u8]) -> CommitMeta {
        CommitMeta::new(CommitContents {
            parents: self.heads().collect(),
            author,
            payload: BlobHash::of_contents(contents),
        })
    }

    /// Ingest our own prepared commit for immediate local use.
    /// Parents must already be delivered, but may still be unsigned local drafts.
    /// Duplicate insertion preserves the existing payload and admission status.
    /// Reports any received items unblocked by local certificate assembly.
    ///
    /// # Panics
    /// Panics before mutation if any parent is missing. Commits prepared with
    /// [`Self::commit_by`] on this graph already satisfy this precondition.
    pub fn ingest_local_commit(&mut self, meta: CommitMeta, data: P) -> IngestReport {
        if self.by_hash.contains_key(meta.hash()) {
            return self.ingest_report();
        }
        assert!(
            meta.parents()
                .iter()
                .all(|hash| self.by_hash.contains_key(hash)),
            "local commit parents must already be delivered"
        );
        self.pending_local.insert(*meta.hash());
        self.insert_commit(meta, data);
        self.finish_ingest(IngestReport::default())
    }

    /// Attach a signature to a known, still-pending local commit.
    /// Also drains received items unblocked by any resulting admissions; inspect
    /// the report for deferred rejections even when the signature is valid.
    pub fn ingest_signature(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<IngestReport, SignatureError> {
        if !self.pending_local.contains(&hash) {
            return Err(SignatureError::NotPendingLocalCommit(hash));
        }
        let key = ed25519_dalek::VerifyingKey::from_bytes(author.as_array())
            .map_err(|_| SignatureError::InvalidPublicKey(author))?;
        let proof = ed25519_dalek::Signature::from_bytes(&signature.into());
        key.verify_strict(hash.as_bytes(), &proof).map_err(|_| {
            SignatureError::InvalidSignature {
                author,
                commit: hash,
            }
        })?;
        self.nodes[self.by_hash[&hash]]
            .signatures
            .insert(author, signature);
        Ok(self.finish_ingest(IngestReport::default()))
    }

    /// Replace the entire permission map. Missing authors are denied; captured
    /// viewpoints keep their previous permissions.
    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.permissions = permissions.clone();
    }

    /// Capture the working history, including pending local commits,
    /// and the current write permissions. Signing does not change this snapshot.
    pub fn view_point(&self) -> ViewPoint {
        ViewPoint {
            heads: self.heads().collect(),
            write_permissions: self.permissions.clone(),
        }
    }

    /// Capture only admitted history and the current write permissions.
    /// Heads are the maximal admitted commits, including ancestors beneath local
    /// drafts. Pending local commits cannot supply write-frontier proofs here.
    pub fn signed_view_point(&self) -> ViewPoint {
        let mut heads = BTreeSet::new();
        // Node order is topological and admitted history is parent-closed.
        for (_, node) in &self.nodes {
            if !self.pending_local.contains(node.meta.hash()) {
                for parent in node.meta.parents() {
                    heads.remove(parent);
                }
                heads.insert(*node.meta.hash());
            }
        }
        ViewPoint {
            heads: heads.into_iter().collect(),
            write_permissions: self.permissions.clone(),
        }
    }

    /// Compare participation over delivered history, including pending local drafts.
    /// This is a participation-only query; prefer [`Self::visit_changes`] for
    /// materialization that also needs newly delivered payloads.
    ///
    /// Returns [`DiffError::MissingCommit`] if either viewpoint names a missing
    /// history head, even when its author is denied permission. Unknown permission
    /// bounds grant nothing; they are not missing-history errors.
    ///
    /// This does not validate admission or signatures. Callers requiring admitted
    /// history can check every head with [`Self::has_commit`]; admission is
    /// parent-closed, so admitted heads imply admitted ancestors.
    pub fn diff(
        &self,
        before: &ViewPoint,
        after: &ViewPoint,
    ) -> Result<ViewPointDiff<'_, P>, DiffError> {
        Ok(ViewPointDiff {
            graph: self,
            before: self.participation(before)?,
            after: self.participation(after)?,
        })
    }

    /// Visit newly delivered commits and participation changes in one traversal.
    ///
    /// This is the standard entry point for CRDT materialization. Start with an
    /// empty [`MaterializationCursor`] and materializer, then use the same callback
    /// after local edits, ingestion/merge, signing, or permission changes.
    ///
    /// A commit is visited exactly once if it is outside `cursor`'s processed
    /// delivery history or its participation changes from `cursor.view_point()`
    /// to `after`. This includes newly delivered commits that participate in
    /// neither viewpoint. Metadata and payloads are borrowed, not cloned.
    ///
    /// Visits are topological (ancestors before descendants), for both additions
    /// and removals and including ancestry through omitted commits. Concurrent
    /// commits have unspecified relative order, which may differ between replicas.
    ///
    /// The returned cursor records all currently delivered history and `after`,
    /// even when no callbacks run. Retain it only with the successfully updated
    /// materializer state. The input cursor is never modified; replaying it
    /// repeats the changes. Callback side effects are not rolled back on panic.
    ///
    /// Like [`Self::diff`], this includes pending local work without validating
    /// admission or signatures. All history heads in the cursor and `after`
    /// must be delivered, otherwise [`DiffError::MissingCommit`] is returned
    /// before any callbacks run. Unknown permission bounds grant nothing.
    ///
    /// # Example
    ///
    /// ```
    /// use std::collections::{HashMap, HashSet};
    /// use stockfoil::{CommitGraph, MaterializationCursor, Vis};
    ///
    /// let graph = CommitGraph::<String>::new();
    /// let mut cursor = MaterializationCursor::new();
    /// let mut stored = HashMap::new();
    /// let mut participating = HashSet::new();
    /// let after = graph.view_point();
    /// cursor = graph.visit_changes(&cursor, &after, |change| {
    ///     let hash = *change.meta.hash();
    ///     if change.newly_delivered {
    ///         stored.insert(hash, change.payload.clone());
    ///     }
    ///     match change.visibility {
    ///         Vis::After => { participating.insert(hash); }
    ///         Vis::Before => { participating.remove(&hash); }
    ///         Vis::Both | Vis::Neither => {}
    ///     }
    /// })?;
    /// # Ok::<(), stockfoil::DiffError>(())
    /// ```
    pub fn visit_changes<'graph>(
        &'graph self,
        cursor: &MaterializationCursor,
        after: &ViewPoint,
        mut visit: impl FnMut(MaterializationChange<'graph, P>),
    ) -> Result<MaterializationCursor, DiffError> {
        let delivered = self.history_clock(&cursor.delivery_heads)?;
        let diff = self.diff(&cursor.view_point, after)?;
        // Delivery can extend beyond both participation clocks: do not stop at
        // their latest visible positions or omit newly delivered denied commits.
        for (idx, node) in &self.nodes {
            let time = self.nodes.chain_time(idx);
            let newly_delivered = !delivered.contains_time(time);
            let visibility = diff.visibility_at(time);
            if newly_delivered || matches!(visibility, Vis::Before | Vis::After) {
                visit(MaterializationChange {
                    meta: &node.meta,
                    payload: &self.store[&idx],
                    newly_delivered,
                    visibility,
                });
            }
        }
        Ok(MaterializationCursor {
            delivery_heads: self.heads().collect(),
            view_point: after.clone(),
        })
    }

    fn publishable_nodes(&self) -> Vec<bool> {
        self.nodes
            .entries
            .iter()
            .map(|node| !self.pending_local.contains(node.meta.hash()))
            .collect()
    }

    // Dependencies and payloads are ready. Index the authenticated members and
    // store signatures on their targets and boundary references on the fragment.
    fn admit_certificate(&mut self, item: crate::VerifiedItem, report: &mut IngestReport) {
        report.admitted.push(item.id().clone());
        for meta in &item.commits {
            self.pending_local.remove(meta.hash());
            self.pending_ingest.admitted(meta.hash());
            // Interior checkpoints are indexed for structural inspection, not
            // granted export certificates. Their normalized ranges may extend
            // outside this item's payload. Only the actual head gets evidence.
            if let ItemId::Fragment(_) = &item.id {
                let idx = self.by_hash[meta.hash()];
                if self.nodes[idx].headed_fragments.is_empty() {
                    self.insert_fragments(idx);
                }
            }
        }
        for evidence in &item.signatures {
            self.nodes[self.by_hash[&evidence.commit]]
                .signatures
                .insert(evidence.author, evidence.signature);
        }
        if let ItemId::Fragment(range) = item.id {
            let idx = self.nodes[self.by_hash[&range.head]]
                .canonical_fragment()
                .unwrap();
            self.fragments.entries[idx.0].signature_boundary = item
                .signature_boundary
                .into_iter()
                .map(|(author, targets)| {
                    (
                        author,
                        targets
                            .into_iter()
                            .map(|hash| self.by_hash[&hash])
                            .collect(),
                    )
                })
                .collect();
            self.fragments.entries[idx.0].borrowed =
                item.borrowed.iter().map(|hash| self.by_hash[hash]).collect();
        }
    }

    // Local work and received items may unblock one another. Received items
    // already have certificates; only local work needs certificate assembly.
    fn finish_ingest(&mut self, mut report: IngestReport) -> IngestReport {
        loop {
            while let Some(item) = self
                .pending_local
                .iter()
                .map(|hash| self.by_hash[hash])
                .filter(|&idx| !self.nodes[idx].signatures.is_empty())
                .find_map(|idx| self.local_item(idx))
            {
                self.admit_certificate(item, &mut report);
            }
            let Some(prepared) = self.pending_ingest.pop_ready() else { break };
            match self.validate_fragment_admission(&prepared.item) {
                Ok(()) => self.admit_prepared(prepared, &mut report),
                Err(error) => report.rejected.push(IngestRejection {
                    item: prepared.item.id().clone(),
                    error,
                }),
            }
        }
        // Reuse shared certificates for all newly ready local items before
        // discarding covered ones. Keep topology, not historical export proofs.
        let cover = self.minimal_cover();
        // A borrowing certificate depends on other items to authenticate part
        // of its history. Re-derive it from every signature we hold before any
        // are pruned: a replaced item is contained in its replacement, so this
        // carries all of its authentication forward, and borrowing persists
        // only while no evidence for that history is held inside the fragment.
        for fragment in &cover.fragments {
            let head = self.by_hash[&fragment.range.head];
            let idx = self.nodes[head].canonical_fragment().unwrap();
            if self.fragments[idx].borrowed.is_empty() {
                continue;
            }
            let members = self.fragments.members(&self.nodes, idx);
            let certificate = self.held_certificate(&members);
            if certificate.uncovered.iter().all(|hash| self.has_commit(hash))
                && !certificate.uncovered.contains(&fragment.range.head)
            {
                let entry = &mut self.fragments.entries[idx.0];
                entry.signature_boundary = certificate
                    .signature_boundary
                    .into_iter()
                    .map(|(author, targets)| {
                        (author, targets.iter().map(|hash| self.by_hash[hash]).collect())
                    })
                    .collect();
                entry.borrowed = certificate
                    .borrowed
                    .iter()
                    .map(|hash| self.by_hash[hash])
                    .collect();
            }
        }
        let heads: HashSet<_> = cover.fragments.iter().map(|f| f.range.head).collect();
        let mut needed: HashSet<_> = cover
            .loose_commits
            .iter()
            .map(|c| {
                let idx = self.by_hash[&c.commit];
                (idx, *self.nodes[idx].meta.author())
            })
            .collect();
        for fragment in &mut self.fragments.entries {
            if !heads.contains(self.nodes[fragment.head].meta.hash())
                || fragment.level != self.nodes[fragment.head].headed_fragments.len()
            {
                fragment.signature_boundary.clear();
                fragment.borrowed.clear();
            }
            for (&author, targets) in &fragment.signature_boundary {
                needed.extend(targets.iter().map(|&idx| (idx, author)));
            }
        }
        for (index, node) in self.nodes.entries.iter_mut().enumerate() {
            if !self.pending_local.contains(node.meta.hash()) {
                node.signatures
                    .retain(|author, _| needed.contains(&(NodeIdx(index), *author)));
            }
        }
        report.pending = self.pending_items();
        report
    }

    fn local_item(&self, head: NodeIdx) -> Option<crate::VerifiedItem> {
        let node = &self.nodes[head];
        let depth = node.depth;
        let item = if depth == 0 {
            UnverifiedItem::Commit {
                meta: node.meta.clone(),
                signatures: node.signature_evidence().collect(),
            }
        } else {
            // Boundary discovery is path-sensitive, but payload membership is
            // the difference of complete histories, not a stopped walk.
            let boundary = self.nodes.fragment_boundary(head, depth);
            if boundary
                .iter()
                .any(|&idx| !self.has_commit(self.nodes[idx].meta.hash()))
            {
                return None;
            }
            let members = self
                .nodes
                .fragment_members(head, &boundary.iter().copied().collect::<Vec<_>>());
            // Read only signatures attached to actual member nodes. Members no
            // held signature covers are borrowed, but only once admitted: an
            // admitted commit was authenticated by some item, whereas an
            // uncovered draft still awaits its own signature.
            let certificate = self.held_certificate(&members);
            if !certificate
                .uncovered
                .iter()
                .all(|hash| self.has_commit(hash))
            {
                return None;
            }
            let range = FragmentRange {
                head: *node.meta.hash(),
                boundary: boundary
                    .iter()
                    .map(|&idx| *self.nodes[idx].meta.hash())
                    .collect(),
            };
            let commits: Vec<_> = members
                .into_iter()
                .map(|idx| self.nodes[idx].meta.clone())
                .collect();
            UnverifiedItem::Fragment {
                range,
                commits,
                signatures: certificate.signatures,
                borrowed: certificate.borrowed,
            }
        };
        let item = item.verify().ok()?;
        if !item
            .dependencies()
            .all(|hash| self.has_commit(hash))
        {
            return None;
        }
        Some(item)
    }

    // The best certificate for these members from signatures held on them:
    // per-author signed frontiers, the evidence for exactly those targets, and
    // the frontier of the members left uncovered as the borrowed tips.
    fn held_certificate(&self, members: &HashSet<NodeIdx>) -> HeldCertificate {
        let metas: BTreeMap<_, _> = members
            .iter()
            .map(|&idx| (*self.nodes[idx].meta.hash(), self.nodes[idx].meta.clone()))
            .collect();
        let evidence: Vec<_> = members
            .iter()
            .flat_map(|&idx| self.nodes[idx].signature_evidence())
            .collect();
        let (signature_boundary, covered) = crate::ingest::coverage(&metas, &evidence);
        let signatures = evidence
            .into_iter()
            .filter(|e| {
                signature_boundary
                    .get(&e.author)
                    .is_some_and(|targets| targets.contains(&e.commit))
            })
            .collect();
        let uncovered: BTreeSet<_> = metas
            .keys()
            .filter(|hash| !covered.contains(hash))
            .copied()
            .collect();
        let borrowed = crate::ingest::frontier(&metas, uncovered.clone());
        HeldCertificate {
            signatures,
            signature_boundary,
            borrowed,
            uncovered,
        }
    }

    pub(crate) fn commit_time(&self, commit: &CommitHash) -> Option<ChainTime> {
        self.by_hash.get(commit).map(|idx| self.nodes.chain_time(*idx))
    }

    pub(crate) fn changed_commits(
        &self,
        before: &AncestorClock,
        after: &AncestorClock,
    ) -> impl Iterator<Item = CommitDiff> {
        let latest = before.latest().max(after.latest());
        self.nodes
            .into_iter()
            .take_while(move |(idx, _)| latest.is_some_and(|last| *idx <= last))
            .filter_map(|(idx, node)| {
                let time = self.nodes.chain_time(idx);
                let visibility_delta = match (
                    before.contains_time(time),
                    after.contains_time(time),
                ) {
                    (true, false) => VisDiff::Before,
                    (false, true) => VisDiff::After,
                    _ => return None,
                };
                Some(CommitDiff {
                    commit_id: *node.hash(),
                    visibility_delta,
                })
            })
    }

    fn history_clock(&self, heads: &[CommitHash]) -> Result<Arc<AncestorClock>, DiffError> {
        let mut indices = heads
            .iter()
            .map(|hash| {
                self.by_hash
                    .get(hash)
                    .copied()
                    .ok_or(DiffError::MissingCommit(*hash))
            })
            .collect::<Result<Vec<_>, _>>()?;
        indices.sort_unstable();
        indices.dedup();
        // The current working history contains every stored node. Its clock is
        // already available as the chain-tip table; no ancestor walk is needed.
        if indices.len() == self.heads.len()
            && indices
                .iter()
                .all(|&idx| self.heads.contains(self.nodes[idx].meta.hash()))
        {
            return Ok(Arc::new(AncestorClock::from_tips(&self.nodes.chain_tips)));
        }
        Ok(self.nodes.clock_for_heads(&indices))
    }

    fn participation(&self, view_point: &ViewPoint) -> Result<AncestorClock, DiffError> {
        let history = self.history_clock(&view_point.heads)?;
        // An author may own several chains. Prepare its frontier only once,
        // then retain just the masked coordinates, not the per-author clocks.
        let mut bounded = HashMap::new();
        Ok(history.mask(|chain, tip| {
            let author = self.nodes.chain_authors[chain];
            match view_point.write_permissions.get(&author) {
                Some(WritePermission::Unbounded) => Some(tip),
                Some(WritePermission::Bounded(frontier)) => bounded
                    .entry(author)
                    .or_insert_with(|| {
                        // Filter frontier *heads*, not the resulting clock:
                        // an out-of-history bound grants nothing in this snapshot.
                        let available = frontier
                            .iter()
                            .filter_map(|hash| self.by_hash.get(hash).copied())
                            .filter(|&idx| history.contains(self.nodes[idx].chain, idx))
                            .collect::<Vec<_>>();
                        self.nodes.clock_for_heads(&available)
                    })
                    .position(chain),
                Some(WritePermission::Denied) | None => None,
            }
        }))
    }
}

/// An opaque node index, valid only in the graph that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NodeIdx(usize);

#[derive(Debug, Clone, Default)]
pub(crate) struct NodeStore {
    // Append-only, parents before children. Each node owns a contiguous parent
    // slice, preserving the envelope's parent order and multiplicity.
    entries: Vec<Node>,
    parent_ids: Vec<NodeIdx>,
    chain_tips: Vec<NodeIdx>,
    // Each chain has one immutable owner; forks may give an author many chains.
    chain_authors: Vec<Author>,
    // Sorted positions allow clock differences to enumerate just their intervals.
    chain_nodes: Vec<Vec<NodeIdx>>,
}

impl NodeStore {
    fn assign_chain(&mut self, author: Author, parents: Range<usize>, position: NodeIdx) -> usize {
        let parents = &self.parent_ids[parents];
        // Ordinary sequential edits need no clock construction or chain scan.
        let direct = parents
            .iter()
            .filter_map(|&parent| {
                let chain = self[parent].chain;
                (self.chain_authors[chain] == author && self.chain_tips[chain] == parent)
                    .then_some(chain)
            })
            .min();
        let reusable = direct.or_else(|| {
            if parents.is_empty() {
                return None;
            }
            // Direct-parent-only reuse strands a chain after every diamond.
            // Any same-author tip in the parents' down-set can safely precede
            // this node, including across commits by other authors.
            let history = self.clock_for_heads(parents);
            history.entries().find_map(|(chain, tip)| {
                (self.chain_authors[chain] == author && self.chain_tips[chain] == tip)
                    .then_some(chain)
            })
        });
        if let Some(chain) = reusable {
            self.chain_tips[chain] = position;
            self.chain_nodes[chain].push(position);
            chain
        } else {
            let chain = self.chain_tips.len();
            self.chain_tips.push(position);
            self.chain_authors.push(author);
            self.chain_nodes.push(vec![position]);
            chain
        }
    }

    fn clock_for_heads(&self, heads: &[NodeIdx]) -> Arc<AncestorClock> {
        if let [head] = heads {
            return Arc::clone(self.clock(*head));
        }
        let mut builder = ClockBuilder::default();
        for &head in heads {
            builder.merge(self.clock(head));
        }
        Arc::new(builder.finish())
    }

    fn clock(&self, head: NodeIdx) -> &Arc<AncestorClock> {
        self[head].clock.get_or_init(|| {
            let mut builder = ClockBuilder::default();
            let mut stack = vec![head];
            let mut visited = HashSet::new();
            while let Some(idx) = stack.pop() {
                if !visited.insert(idx) {
                    continue;
                }
                let node = &self[idx];
                if let Some(clock) = node.clock.get() {
                    builder.merge(clock);
                } else {
                    builder.include(node.chain, idx);
                    stack.extend(self.parents(idx));
                }
            }
            // Do not prune a walk merely because builder contains a node's chain
            // time: its cross-chain parents may not have been visited yet.
            let clock = Arc::new(builder.finish());
            // No-std caches use a nonblocking, atomics-only once cell. Racing
            // readers may compute twice, but always observe the winning clock.
            #[cfg(not(feature = "std"))]
            let clock = Box::new(clock);
            clock
        })
    }

    fn parents(&self, node: NodeIdx) -> &[NodeIdx] {
        &self.parent_ids[self[node].parents.clone()]
    }

    fn children(&self, node: NodeIdx) -> &[NodeIdx] {
        &self[node].children
    }

    // Discover raw depth-based stops, including redundant ancestor boundaries.
    // This is intentionally independent of normalized payload membership.
    fn fragment_boundary(&self, head: NodeIdx, level: usize) -> BTreeSet<NodeIdx> {
        let mut seen = HashSet::new();
        let mut boundary = BTreeSet::new();
        let mut stack = vec![head];
        while let Some(idx) = stack.pop() {
            if !seen.insert(idx) {
                continue;
            }
            if idx != head && self[idx].depth >= level {
                boundary.insert(idx);
            } else {
                stack.extend(self.parents(idx));
            }
        }
        boundary
    }

    fn fragment_members(&self, head: NodeIdx, boundary: &[NodeIdx]) -> HashSet<NodeIdx> {
        let included = self.clock(head);
        let excluded = self.clock_for_heads(boundary);
        let mut members = HashSet::new();
        for (chain, tip) in included.entries() {
            let nodes = &self.chain_nodes[chain];
            let start = excluded
                .position(chain)
                .map_or(0, |pos| nodes.partition_point(|&idx| idx <= pos));
            let end = nodes.partition_point(|&idx| idx <= tip);
            if start < end {
                members.extend(nodes[start..end].iter().copied());
            }
        }
        members
    }

    pub(crate) fn chain_time(&self, idx: NodeIdx) -> ChainTime {
        ChainTime {
            chain: self[idx].chain,
            position: idx,
        }
    }
}

impl<'a> IntoIterator for &'a NodeStore {
    type Item = (NodeIdx, &'a Node);

    type IntoIter = NodeIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        NodeIter(self.entries.iter().enumerate())
    }
}

pub(crate) struct NodeIter<'a>(core::iter::Enumerate<core::slice::Iter<'a, Node>>);

impl<'a> Iterator for NodeIter<'a> {
    type Item = (NodeIdx, &'a Node);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(count, node)| (NodeIdx(count), node))
    }
}

impl Index<NodeIdx> for NodeStore {
    type Output = Node;

    fn index(&self, index: NodeIdx) -> &Self::Output {
        &self.entries[index.0]
    }
}

impl IndexMut<NodeIdx> for NodeStore {
    fn index_mut(&mut self, index: NodeIdx) -> &mut Self::Output {
        &mut self.entries[index.0]
    }
}

struct HeldCertificate {
    signatures: Vec<SignatureEvidence>,
    signature_boundary: BTreeMap<Author, BTreeSet<CommitHash>>,
    borrowed: BTreeSet<CommitHash>,
    uncovered: BTreeSet<CommitHash>,
}

#[derive(Clone)]
struct Fragment {
    head: NodeIdx,
    level: usize,
    boundary: Range<usize>,
    signature_boundary: BTreeMap<Author, Vec<NodeIdx>>,
    // Tips of the certificate's borrowed region, authenticated by other items.
    borrowed: Vec<NodeIdx>,
}

#[derive(Clone, Default)]
pub(crate) struct FragmentStore {
    entries: Vec<Fragment>,
    boundary_ids: Vec<NodeIdx>,
}

impl FragmentStore {
    fn boundary(&self, fragment: FragmentIdx) -> &[NodeIdx] {
        &self.boundary_ids[self[fragment].boundary.clone()]
    }

    fn members(&self, nodes: &NodeStore, fragment: FragmentIdx) -> HashSet<NodeIdx> {
        nodes.fragment_members(self[fragment].head, self.boundary(fragment))
    }

    pub(crate) fn summary(
        &self,
        nodes: &NodeStore,
        eligible: &[bool],
        minimize: bool,
        certified_only: bool,
    ) -> crate::SedimentreeSummary {
        // A stopped walk overstates coverage when an edge bypasses a boundary.
        // Enumerate normalized chain intervals once for each indexed range.
        let candidates: Vec<_> = nodes
            .into_iter()
            .filter_map(|(idx, node)| {
                node.canonical_fragment()
                    .filter(|&fragment| {
                        eligible[idx.0]
                            && (!certified_only || !self[fragment].signature_boundary.is_empty())
                    })
                    .map(|fragment| (fragment, self.members(nodes, fragment)))
            })
            .collect();
        let mut redundant = HashSet::new();
        if minimize {
            let by_head: HashMap<_, _> = candidates
                .iter()
                .map(|(idx, members)| (self[*idx].head, (*idx, members)))
                .collect();
            for (cover_idx, cover_members) in &candidates {
                let cover_boundary: HashSet<_> =
                    self.boundary(*cover_idx).iter().copied().collect();
                // Only heads actually in this payload can be contained. Avoid
                // an all-pairs scan, especially for long same-depth chains.
                for head in cover_members {
                    let Some(&(idx, members)) = by_head.get(head) else {
                        continue;
                    };
                    // Preserve equal-depth overlaps. Head membership is only a
                    // candidate: require the whole payload and its boundaries.
                    if self[idx].level < self[*cover_idx].level
                        && !redundant.contains(&idx)
                        && members.is_subset(cover_members)
                        && self
                            .boundary(idx)
                            .iter()
                            .all(|b| cover_members.contains(b) || cover_boundary.contains(b))
                    {
                        redundant.insert(idx);
                    }
                }
            }
        }
        let mut covered = HashSet::new();
        let mut fragments = Vec::new();
        for (idx, members) in &candidates {
            covered.extend(members.iter().copied());
            let fragment = &self[*idx];
            if redundant.contains(idx) {
                continue;
            }
            let checkpoints = members
                .iter()
                .filter(|&&idx| idx != fragment.head && nodes[idx].depth > 0)
                .map(|&idx| {
                    crate::Checkpoint(nodes[idx].meta.hash().as_bytes()[..12].try_into().unwrap())
                })
                .collect();
            fragments.push(crate::Fragment {
                range: crate::FragmentRange {
                    head: *nodes[fragment.head].meta.hash(),
                    boundary: self
                        .boundary(*idx)
                        .iter()
                        .map(|&idx| *nodes[idx].meta.hash())
                        .collect(),
                },
                checkpoints,
            });
        }
        let mut loose_commits = Vec::new();
        for (idx, node) in nodes {
            if eligible[idx.0] && !covered.contains(&idx) {
                loose_commits.push(crate::LooseCommit {
                    commit: *node.meta.hash(),
                    parents: node.meta.parents().iter().copied().collect(),
                });
            }
        }
        fragments.sort_by(|a, b| a.range.cmp(&b.range));
        loose_commits.sort_by_key(|commit| commit.commit);
        crate::SedimentreeSummary {
            loose_commits,
            fragments,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FragmentIdx(usize);

impl Index<FragmentIdx> for FragmentStore {
    type Output = Fragment;

    fn index(&self, index: FragmentIdx) -> &Self::Output {
        &self.entries[index.0]
    }
}

fn sedimentree_depth(hash: &CommitHash) -> usize {
    hash.as_bytes().iter().take_while(|b| **b == 0).count()
}

#[cfg(test)]
mod ancestry_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn author(key: &SigningKey) -> Author {
        Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
    }

    fn mined(author: Author, parents: Vec<CommitHash>, depth: usize) -> CommitMeta {
        (0u64..)
            .map(|nonce| {
                CommitMeta::new(CommitContents {
                    author,
                    parents: parents.clone(),
                    payload: BlobHash::of_contents(&nonce.to_le_bytes()),
                })
            })
            .find(|meta| sedimentree_depth(meta.hash()) == depth)
            .unwrap()
    }

    fn sign(graph: &mut CommitGraph<u8>, key: &SigningKey, meta: &CommitMeta) {
        graph
            .ingest_signature(
                author(key),
                *meta.hash(),
                Signature::from(key.sign(meta.hash().as_bytes()).to_bytes()),
            )
            .unwrap();
    }

    #[test]
    fn replacement_prunes_certificates_but_preserves_authentication_and_viewpoints() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let alice = author(&key);
        let mut graph = CommitGraph::new();
        graph.set_write_permissions(&HashMap::from([(alice, WritePermission::Unbounded)]));
        let root = mined(alice, vec![], 0);
        graph.ingest_local_commit(root.clone(), 1);
        sign(&mut graph, &key, &root);
        let old_view = graph.signed_view_point();
        let stale = ItemId::Commit(*root.hash());
        assert!(graph.export(&stale).unwrap().verify().is_ok());

        let head = mined(alice, vec![*root.hash()], 1);
        graph.ingest_local_commit(head.clone(), 2);
        sign(&mut graph, &key, &head);
        let cover = graph.minimal_cover();
        assert!(cover.loose_commits.is_empty());
        assert_eq!(cover.fragments.len(), 1);
        assert!(
            matches!(graph.export(&stale), Err(ExportError::NotInMinimalCover(id)) if id == stale)
        );
        assert!(
            graph.nodes[graph.by_hash[root.hash()]]
                .signatures
                .is_empty()
        );
        let fragment = graph.nodes[graph.by_hash[head.hash()]]
            .canonical_fragment()
            .unwrap();
        assert_eq!(
            graph.fragments[fragment].signature_boundary[&alice],
            vec![graph.by_hash[head.hash()]]
        );
        assert_eq!(graph.signed_view_point().heads(), &[*head.hash()]);
        assert!(graph.has_commit(root.hash()));
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert_eq!(peer.minimal_cover(), cover);
        let mut relay = CommitGraph::new();
        relay.merge(&peer);
        assert_eq!(relay.minimal_cover(), cover);

        // Pruning export certificates must not change captured membership,
        // including on replicas that only received the replacement fragment.
        for graph in [&graph, &peer, &relay] {
            let membership = graph.diff(&old_view, &old_view).unwrap();
            assert_eq!(membership.visibility(root.hash()), Ok(crate::Vis::Both));
            assert_eq!(membership.visibility(head.hash()), Ok(crate::Vis::Neither));
        }
    }

    #[test]
    fn pending_local_checkpoint_does_not_enter_the_index_before_admission() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let alice = author(&key);
        let mut graph = CommitGraph::new();
        let root = mined(alice, vec![], 0);
        graph.ingest_local_commit(root.clone(), 1);
        sign(&mut graph, &key, &root);
        let old_cover = graph.minimal_cover();
        let head = mined(alice, vec![*root.hash()], 1);
        let tip = mined(alice, vec![*head.hash()], 0);
        // Local work is delivered parents-first, but signatures can arrive in
        // the opposite order without publishing unsigned dependencies.
        graph.ingest_local_commit(head.clone(), 2);
        graph.ingest_local_commit(tip.clone(), 3);
        sign(&mut graph, &key, &tip);
        assert!(!graph.has_commit(head.hash()));
        assert!(!graph.has_commit(tip.hash()));
        assert!(
            graph.nodes[graph.by_hash[head.hash()]]
                .headed_fragments
                .is_empty()
        );
        // Neither pending item has entered the authenticated index.
        assert_eq!(graph.minimal_cover(), old_cover);
        assert!(graph.export(&ItemId::Commit(*root.hash())).is_ok());
        assert!(graph.export(&ItemId::Commit(*tip.hash())).is_err());
        assert_eq!(graph.nodes[graph.by_hash[tip.hash()]].signatures.len(), 1);

        sign(&mut graph, &key, &head);
        assert_ne!(graph.minimal_cover(), old_cover);
        assert!(graph.has_commit(tip.hash()));
        assert!(graph.export(&ItemId::Commit(*root.hash())).is_err());
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert!(peer.has_commit(tip.hash()));
        assert_eq!(peer.minimal_cover(), graph.minimal_cover());
    }

    #[test]
    fn pending_local_siblings_share_evidence_before_constituents_are_discarded() {
        let alice_key = SigningKey::from_bytes(&[1; 32]);
        let bob_key = SigningKey::from_bytes(&[2; 32]);
        let root = mined(author(&alice_key), vec![], 0);
        let left = mined(author(&bob_key), vec![*root.hash()], 1);
        let right = mined(author(&bob_key), vec![*root.hash(), *root.hash()], 1);
        let mut graph = CommitGraph::new();
        for meta in [&root, &left, &right] {
            graph.ingest_local_commit(meta.clone(), 1);
        }
        sign(&mut graph, &bob_key, &left);
        sign(&mut graph, &bob_key, &right);
        assert!(graph.minimal_cover().fragments.is_empty());
        sign(&mut graph, &alice_key, &root);
        assert_eq!(graph.minimal_cover().fragments.len(), 2);
        assert!(graph.pending_local.is_empty());
        assert!(
            graph.nodes[graph.by_hash[root.hash()]]
                .signatures
                .contains_key(&author(&alice_key))
        );
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert_eq!(peer.minimal_cover(), graph.minimal_cover());
    }

    #[test]
    fn shared_signature_survives_on_its_target_for_each_current_entry() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let bob_key = SigningKey::from_bytes(&[2; 32]);
        let alice = author(&key);
        let mut graph = CommitGraph::new();
        let root = mined(author(&bob_key), vec![], 0);
        graph.ingest_local_commit(root.clone(), 1);
        sign(&mut graph, &bob_key, &root);
        let left = mined(alice, vec![*root.hash()], 1);
        // Change the ancestry so mining cannot return the same head.
        let middle = mined(alice, vec![*root.hash()], 0);
        let right = mined(alice, vec![*middle.hash()], 1);
        graph.ingest_local_commit(left.clone(), 2);
        graph.ingest_local_commit(middle.clone(), 3);
        graph.ingest_local_commit(right.clone(), 4);
        sign(&mut graph, &key, &left);
        // The second received fragment carries its own certificate, including
        // evidence shared with the first. No historical signature insertion.
        let incoming = UnverifiedItem::Fragment {
            borrowed: BTreeSet::new(),
            range: FragmentRange {
                head: *right.hash(),
                boundary: BTreeSet::new(),
            },
            commits: vec![root.clone(), middle.clone(), right.clone()],
            signatures: vec![
                SignatureEvidence {
                    author: author(&bob_key),
                    commit: *root.hash(),
                    signature: Signature::from(bob_key.sign(root.hash().as_bytes()).to_bytes()),
                },
                SignatureEvidence {
                    author: alice,
                    commit: *right.hash(),
                    signature: Signature::from(key.sign(right.hash().as_bytes()).to_bytes()),
                },
            ],
        }
        .verify()
        .unwrap();
        graph.ingest(incoming, |_| 0).unwrap();
        assert_eq!(graph.minimal_cover().fragments.len(), 2);
        for fragment in graph.minimal_cover().fragments {
            let idx = graph.nodes[graph.by_hash[&fragment.range.head]]
                .canonical_fragment()
                .unwrap();
            assert!(
                graph.fragments[idx].signature_boundary[&author(&bob_key)]
                    .contains(&graph.by_hash[root.hash()])
            );
            assert!(
                graph.nodes[graph.by_hash[root.hash()]]
                    .signatures
                    .contains_key(&author(&bob_key))
            );
            assert!(
                graph
                    .export(&ItemId::Fragment(fragment.range))
                    .unwrap()
                    .verify()
                    .is_ok()
            );
        }
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert!(peer.has_commit(root.hash()));
    }
}
