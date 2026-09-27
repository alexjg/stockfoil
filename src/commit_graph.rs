use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::{ControlFlow, Index, IndexMut, Range},
    sync::{Arc, OnceLock},
};

use crate::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, DiffError, ExportError,
    FragmentRange, FragmentReadError, ItemId, ResolveError, ResolvedViewPoint, SedimentreeView,
    Signature, SignatureError, SignatureEvidence, UnverifiedItem, ViewPoint, ViewPointDiff,
    WritePermission,
    ancestor_clock::{AncestorClock, ClockBuilder},
    sedimentree::MinimalCover,
    view_point::PreparedView,
};

/// An in-memory, causally closed commit DAG.
/// Every stored commit has all its ancestors available. Items with missing
/// dependencies must be buffered outside the graph.
///
/// `P` is opaque: it can be decoded operations, a storage reference, or application
/// metadata. Signatures authenticate the commit envelope and its payload hash,
/// not an arbitrary `P`. Callers must validate received payload bytes against
/// [`CommitMeta::payload_hash`] before ingestion or use.
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
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) meta: CommitMeta,
    // Immutable graph-local chain label; NodeIdx is the position on the chain.
    pub(crate) chain: usize,
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
        }
    }

    pub fn parents(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        self.nodes.parents(node_idx).iter().copied()
    }

    /// Return direct children once each, in insertion order, in O(number of children).
    pub fn children(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        self.nodes.children(node_idx).iter().copied()
    }

    pub fn roots(&self) -> impl Iterator<Item = NodeIdx> {
        self.nodes
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, node)| node.parents.is_empty().then_some(NodeIdx(i)))
    }

    /// Return hashes of commits with no children in the graph, in hash order.
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

    /// Admit verified sedimentree history. The destination must already have the
    /// item's authenticated dependencies; stateless verification does not imply
    /// that those dependencies are present here.
    ///
    /// Checks range boundary and external parents, then canonical fragment
    /// placement, before graph mutation or payload callbacks. For each newly admitted commit, `payload` supplies application
    /// data or a storage handle; duplicates preserve existing data. Neither this
    /// method nor metadata verification checks the bytes of an opaque `P`.
    /// Uses the item's evidence to maintain the published minimal cover.
    ///
    /// Payload callbacks run before mutation; if one panics, graph state is
    /// unchanged. Payload-byte validation remains the application's responsibility.
    ///
    /// ```compile_fail
    /// use stockfoil::{CommitGraph, UnverifiedItem};
    /// fn bypass(graph: &mut CommitGraph<()>, item: UnverifiedItem) {
    ///     graph.ingest(item, |_| ()).unwrap();
    /// }
    /// ```
    pub fn ingest(
        &mut self,
        item: crate::VerifiedItem,
        mut payload: impl FnMut(&CommitMeta) -> P,
    ) -> Result<(), crate::IngestError> {
        for dependency in item.dependencies() {
            if !self.has_admitted_commit(dependency) {
                return Err(crate::IngestError::MissingDependency(*dependency));
            }
        }
        self.validate_fragment_admission(&item)?;
        // Prepare all new payloads before changing state. A pending local copy
        // also owns its original payload; admission must not overwrite it.
        let prepared: Vec<_> = item
            .commits
            .iter()
            .filter(|meta| !self.by_hash.contains_key(meta.hash()))
            .map(|meta| (meta.clone(), payload(meta)))
            .collect();
        // Verification supplies parents-first members. Admitted dependencies
        // close the fragment boundary, so every new member is ready on arrival.
        for (meta, data) in prepared {
            self.insert_commit(meta, data);
        }
        self.admit_certificate(item);
        self.finish_local_commits();
        Ok(())
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
                stack.extend(self.parents(idx));
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
        let chain = self.nodes.assign_chain(parents.clone(), node_idx);
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
        if !self.has_admitted_commit(&range.head) {
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
                .parents(idx)
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
        match id {
            ItemId::Commit(hash)
                if cover.loose_commits.iter().any(|c| &c.commit == hash)
                    || cover.fragments.iter().any(|f| &f.range.head == hash) =>
            {
                let node = &self.nodes[self.by_hash[hash]];
                Ok(UnverifiedItem::Commit {
                    meta: node.meta.clone(),
                    signatures: node
                        .signature_evidence()
                        .filter(|evidence| &evidence.author == node.meta.author())
                        .collect(),
                })
            }
            ItemId::Fragment(range) if cover.fragments.iter().any(|f| &f.range == range) => {
                let head = self.by_hash[&range.head];
                let fragment = self.nodes[head].canonical_fragment().unwrap();
                let mut commits = Vec::new();
                self.traverse_fragment(range, |meta, _| {
                    commits.push(meta.clone());
                    ControlFlow::Continue(())
                })
                .expect("cover fragment is indexed");
                Ok(UnverifiedItem::Fragment {
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
                })
            }
            _ => Err(ExportError::NotInMinimalCover(id.clone())),
        }
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
    pub fn has_admitted_commit(&self, hash: &CommitHash) -> bool {
        self.by_hash.contains_key(hash) && !self.pending_local.contains(hash)
    }

    pub fn merge(&mut self, other: &Self) {
        let cover = other.minimal_cover();
        let mut pending: Vec<_> = cover
            .loose_commits
            .into_iter()
            .map(|commit| ItemId::Commit(commit.commit))
            .chain(
                cover
                    .fragments
                    .into_iter()
                    .map(|fragment| ItemId::Fragment(fragment.range)),
            )
            .map(|id| {
                other
                    .export(&id)
                    .expect("current cover entry")
                    .verify()
                    .expect("certified cover entry")
            })
            .collect();
        while let Some(position) = pending.iter().position(|item| {
            item.dependencies()
                .all(|hash| self.has_admitted_commit(hash))
        }) {
            self.ingest(pending.remove(position), |meta| {
                other.store[&other.by_hash[meta.hash()]].clone()
            })
            .expect("ready certified cover entry");
        }
        debug_assert!(
            pending.is_empty(),
            "a complete cover must be causally admissible"
        );
    }

    /// Visit each delivered commit once, parents before children, including local drafts.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
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
            stack.extend(self.parents(idx));
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
    ///
    /// # Panics
    /// Panics before mutation if any parent is missing. Commits prepared with
    /// [`Self::commit_by`] on this graph already satisfy this precondition.
    pub fn ingest_local_commit(&mut self, meta: CommitMeta, data: P) {
        if self.by_hash.contains_key(meta.hash()) {
            return;
        }
        assert!(
            meta.parents()
                .iter()
                .all(|hash| self.by_hash.contains_key(hash)),
            "local commit parents must already be delivered"
        );
        self.pending_local.insert(*meta.hash());
        self.insert_commit(meta, data);
        self.finish_local_commits();
    }

    /// Attach a signature to a known, still-pending local commit
    pub fn ingest_signature(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<(), SignatureError> {
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
        self.finish_local_commits();
        Ok(())
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

    /// Resolve a viewpoint against admitted history, not the opaque payloads `P`.
    /// Every head must be admitted: verified, parent-closed admission establishes
    /// authentication of its entire history. Pending local heads do not suffice,
    /// even for a viewpoint captured locally or an author denied permission.
    pub fn resolve(&self, view_point: &ViewPoint) -> Result<ResolvedViewPoint<'_>, ResolveError> {
        for hash in &view_point.heads {
            if !self.by_hash.contains_key(hash) {
                return Err(ResolveError::MissingCommit(*hash));
            }
            if self.pending_local.contains(hash) {
                return Err(ResolveError::PendingLocalCommit(*hash));
            }
        }
        Ok(ResolvedViewPoint {
            known: &self.by_hash,
            nodes: &self.nodes,
            visible: self.participation(view_point)?,
        })
    }

    /// Compare participation optimistically, including pending local history.
    /// Received viewpoints must be resolved before they drive a materialized view.
    pub fn diff(
        &self,
        before: &ViewPoint,
        after: &ViewPoint,
    ) -> Result<ViewPointDiff<'_>, DiffError> {
        Ok(ViewPointDiff {
            known: &self.by_hash,
            nodes: &self.nodes,
            before: self.participation(before)?,
            after: self.participation(after)?,
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
    fn admit_certificate(&mut self, item: crate::VerifiedItem) {
        for meta in &item.commits {
            self.pending_local.remove(meta.hash());
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
        }
    }

    // Only pending local items need assembly. Received fragments already carry
    // their certificate; this never rebuilds certificates for admitted history.
    fn finish_local_commits(&mut self) {
        loop {
            let ready = self
                .pending_local
                .iter()
                .map(|hash| self.by_hash[hash])
                .filter(|&idx| !self.nodes[idx].signatures.is_empty())
                .find_map(|idx| self.local_item(idx));
            let Some(item) = ready else { break };
            self.admit_certificate(item);
        }
        // Reuse shared certificates for all newly ready local items before
        // discarding covered ones. Keep topology, not historical export proofs.
        let cover = self.minimal_cover();
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
                .any(|&idx| !self.has_admitted_commit(self.nodes[idx].meta.hash()))
            {
                return None;
            }
            let members = self
                .nodes
                .fragment_members(head, &boundary.iter().copied().collect::<Vec<_>>());
            // Read only signatures attached to actual member nodes. A head
            // signature supersedes older signatures by that author. No inherited
            // arbitrary-target certificate or target-membership filter is needed.
            let signatures = node
                .signature_evidence()
                .chain(
                    members
                        .iter()
                        .filter(|&&idx| idx != head)
                        .flat_map(|&idx| self.nodes[idx].signature_evidence())
                        .filter(|evidence| !node.signatures.contains_key(&evidence.author)),
                )
                .collect();
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
                signatures,
            }
        };
        let item = item.verify().ok()?;
        if !item
            .dependencies()
            .all(|hash| self.has_admitted_commit(hash))
        {
            return None;
        }
        Some(item)
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

    fn participation(&self, view_point: &ViewPoint) -> Result<PreparedView, DiffError> {
        let history = self.history_clock(&view_point.heads)?;
        let mut bounded = HashMap::new();
        let mut unbounded = HashSet::new();
        for (&author, permission) in &view_point.write_permissions {
            match permission {
                WritePermission::Unbounded => {
                    unbounded.insert(author);
                }
                WritePermission::Bounded(frontier) => {
                    // Filter frontier *heads*, not the resulting ancestry clock:
                    // an out-of-history bound grants nothing in this snapshot.
                    let available = frontier
                        .iter()
                        .filter_map(|hash| self.by_hash.get(hash).copied())
                        .filter(|&idx| history.contains(self.nodes[idx].chain, idx))
                        .collect::<Vec<_>>();
                    bounded.insert(author, self.nodes.clock_for_heads(&available));
                }
                WritePermission::Denied => {}
            }
        }
        Ok(PreparedView {
            history,
            bounded,
            unbounded,
        })
    }
}

/// An opaque node index, valid only in the graph that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeIdx(usize);

#[derive(Debug, Clone, Default)]
pub(crate) struct NodeStore {
    // Append-only, parents before children. Each node owns a contiguous parent
    // slice, preserving the envelope's parent order and multiplicity.
    entries: Vec<Node>,
    parent_ids: Vec<NodeIdx>,
    chain_tips: Vec<NodeIdx>,
    // Sorted positions allow clock differences to enumerate just their intervals.
    chain_nodes: Vec<Vec<NodeIdx>>,
}

impl NodeStore {
    fn assign_chain(&mut self, parents: Range<usize>, position: NodeIdx) -> usize {
        let parents = &self.parent_ids[parents];
        // Ordinary sequential edits need no clock construction or chain scan.
        let direct = parents
            .iter()
            .filter_map(|&parent| {
                let chain = self[parent].chain;
                (self.chain_tips[chain] == parent).then_some(chain)
            })
            .min();
        let reusable = direct.or_else(|| {
            if parents.is_empty() {
                return None;
            }
            // Direct-parent-only reuse strands a chain after every diamond.
            // Any tip in the parents' down-set can safely precede this node.
            let history = self.clock_for_heads(parents);
            history
                .entries()
                .find_map(|(chain, tip)| (self.chain_tips[chain] == tip).then_some(chain))
        });
        if let Some(chain) = reusable {
            self.chain_tips[chain] = position;
            self.chain_nodes[chain].push(position);
            chain
        } else {
            let chain = self.chain_tips.len();
            self.chain_tips.push(position);
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
            Arc::new(builder.finish())
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
}

impl<'a> IntoIterator for &'a NodeStore {
    type Item = (NodeIdx, &'a Node);

    type IntoIter = NodeIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        NodeIter(self.entries.iter().enumerate())
    }
}

pub(crate) struct NodeIter<'a>(std::iter::Enumerate<std::slice::Iter<'a, Node>>);

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

#[derive(Clone)]
struct Fragment {
    head: NodeIdx,
    level: usize,
    boundary: Range<usize>,
    signature_boundary: BTreeMap<Author, Vec<NodeIdx>>,
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
        assert!(graph.has_admitted_commit(root.hash()));
        assert!(
            graph
                .resolve(&old_view)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert_eq!(peer.minimal_cover(), cover);
        assert!(
            peer.resolve(&old_view)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );
        let mut relay = CommitGraph::new();
        relay.merge(&peer);
        assert_eq!(relay.minimal_cover(), cover);
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
        assert!(!graph.has_admitted_commit(head.hash()));
        assert!(!graph.has_admitted_commit(tip.hash()));
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
        assert!(graph.has_admitted_commit(tip.hash()));
        assert!(graph.export(&ItemId::Commit(*root.hash())).is_err());
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert!(peer.has_admitted_commit(tip.hash()));
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
        assert!(peer.has_admitted_commit(root.hash()));
    }
}
