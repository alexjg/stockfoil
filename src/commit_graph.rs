use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::{ControlFlow, Index, IndexMut, Range},
};

use crate::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, DiffError, ExportError,
    FragmentRange, FragmentReadError, ItemId, ResolveError, ResolvedViewPoint, SedimentreeView,
    Signature, SignatureError, SignatureEvidence, UnverifiedItem, ViewPoint, ViewPointDiff,
    WritePermission, sedimentree::MinimalCover,
};

/// An in-memory, causally closed commit DAG.
/// Every stored commit has all its ancestors available. Items with missing
/// dependencies must be buffered outside the graph.
///
/// `P` is opaque: it can be decoded operations, a storage reference, or application
/// metadata. Signatures authenticate the commit envelope and its payload hash,
/// not an arbitrary `P`. Callers must validate received payload bytes against
/// [`CommitMeta::payload_hash`] before ingestion or use.
///
/// Cloning preserves local provenance. Received history must pass sedimentree
/// verification and enter through [`Self::ingest`], never raw commit insertion.
/// Local edits may await signatures, but all their parents must already be
/// delivered before insertion.
#[derive(Clone)]
pub struct CommitGraph<P> {
    nodes: NodeStore,
    fragments: FragmentStore,
    store: HashMap<NodeIdx, P>,
    by_hash: HashMap<CommitHash, NodeIdx>,
    heads: BTreeSet<CommitHash>,
    permissions: HashMap<Author, WritePermission>,
    // Optimistic local work, removed as soon as it is admitted. Every delivered
    // commit outside this set is verified and has admitted dependencies.
    pending_local: HashSet<CommitHash>,
}

#[derive(Clone)]
pub(crate) struct Node {
    meta: CommitMeta,
    depth: usize,
    parents: Range<usize>,
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

    pub fn children(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        (0..self.nodes.entries.len())
            .map(NodeIdx)
            .filter(move |&child| self.parents(child).any(|parent| parent == node_idx))
    }

    pub fn roots(&self) -> impl Iterator<Item = NodeIdx> {
        self.nodes
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, node)| node.parents.is_empty().then_some(NodeIdx(i)))
    }

    /// Return hashes of delivered commits with no children, in hash order.
    /// Includes local drafts awaiting signatures.
    /// Iterating all heads takes O(h) time for h heads, without scanning the graph.
    pub fn delivered_heads(&self) -> impl Iterator<Item = CommitHash> + '_ {
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
        let mut stack = vec![descendant];
        let mut visited = HashSet::new();
        while let Some(idx) = stack.pop() {
            if idx == ancestor {
                return true;
            }
            if idx < ancestor || !visited.insert(idx) {
                continue;
            }
            stack.extend(self.parents(idx));
        }
        false
    }

    /// Admit verified sedimentree history. The destination must already have the
    /// item's authenticated dependencies; stateless verification does not imply
    /// that those dependencies are present here.
    ///
    /// Checks dependencies before graph mutation or payload
    /// callbacks. For each newly admitted commit, `payload` supplies application
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
        for dependency in &item.dependencies {
            if !self.has_admitted_commit(dependency) {
                return Err(crate::IngestError::MissingDependency(*dependency));
            }
        }
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
        self.store.insert(node_idx, data);
        for parent in meta.parents() {
            self.heads.remove(parent);
        }
        self.heads.insert(hash);
        self.nodes.entries.push(Node {
            depth: sedimentree_depth(&hash),
            meta,
            parents,
            signatures: BTreeMap::new(),
            headed_fragments: Vec::new(),
        });
        self.by_hash.insert(hash, node_idx);
    }

    // Build headed fragments from lower to higher levels.
    fn insert_fragments(&mut self, head: NodeIdx) {
        let depth = self.nodes[head].depth;
        for level in 1..=depth {
            let boundary: BTreeSet<_> = if level == 1 {
                self.nodes
                    .level_one_members(head)
                    .into_iter()
                    .filter(|&idx| idx != head && self.nodes[idx].depth > 0)
                    .collect()
            } else {
                // The same head's lower-level fragment was just inserted.
                // Every other constituent belongs to an older commit.
                self.fragments
                    .lower_fragments(&self.nodes, head, level)
                    .into_iter()
                    .flat_map(|idx| self.fragments.boundary(idx).iter().copied())
                    .filter(|&idx| self.nodes[idx].depth >= level)
                    .collect()
            };
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

    /// Visit a canonical fragment's commits, including its head and boundaries,
    /// exactly once. Parents within the fragment precede their children; among
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

        // Derive membership through the fragment hierarchy, not from the caller's
        // boundary set. A publishable head guarantees publishable ancestors.
        let members = self.fragments.members(&self.nodes, fragment_idx);
        let mut remaining = HashMap::new();
        let mut children: HashMap<NodeIdx, Vec<NodeIdx>> = HashMap::new();
        let mut ready = BTreeSet::new();
        for &idx in &members {
            // Use ALL edges between members, even outgoing parent edges of a
            // boundary reached through another path. Repeated parents count once.
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
        self.publishable_nodes()[idx.0].then(|| (&self.nodes[idx].meta, &self.store[&idx]))
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

    /// Compute the structural minimal cover using the authenticated fragment index.
    pub fn minimal_cover(&self) -> MinimalCover {
        let summary = self.as_sedimentree().minimized_summary();
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
            parents: self.delivered_heads().collect(),
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

    /// Capture the delivered working history, including pending local commits,
    /// and the current write permissions. Signing does not change this snapshot.
    pub fn view_point(&self) -> ViewPoint {
        ViewPoint {
            delivered_heads: self.delivered_heads().collect(),
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
            delivered_heads: heads.into_iter().collect(),
            write_permissions: self.permissions.clone(),
        }
    }

    /// Resolve a viewpoint against admitted history, not the opaque payloads `P`.
    /// Every head must be admitted: verified, parent-closed admission establishes
    /// authentication of its entire history. Pending local heads do not suffice,
    /// even for a viewpoint captured locally or an author denied permission.
    pub fn resolve(&self, view_point: &ViewPoint) -> Result<ResolvedViewPoint<'_>, ResolveError> {
        for hash in &view_point.delivered_heads {
            if !self.by_hash.contains_key(hash) {
                return Err(ResolveError::MissingCommit(*hash));
            }
            if self.pending_local.contains(hash) {
                return Err(ResolveError::PendingLocalCommit(*hash));
            }
        }
        Ok(ResolvedViewPoint {
            known: &self.by_hash,
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
            // A signed commit at a checkpoint is still just a commit. Only a
            // verified fragment supplies its canonical range's certificate.
            if let ItemId::Fragment(range) = &item.id {
                let idx = self.by_hash[meta.hash()];
                if !range.boundary.contains(meta.hash())
                    && self.nodes[idx].headed_fragments.is_empty()
                {
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
            // The pending head is not indexed yet. Read its canonical range
            // from working metadata, stopping at already-admitted boundaries.
            let mut members = BTreeSet::new();
            let mut boundary = BTreeSet::new();
            let mut stack = vec![head];
            while let Some(idx) = stack.pop() {
                if !members.insert(idx) {
                    continue;
                }
                let meta = &self.nodes[idx].meta;
                if idx != head && self.nodes[idx].depth >= depth {
                    boundary.insert(*meta.hash());
                } else {
                    stack.extend(self.parents(idx));
                }
            }
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
            UnverifiedItem::Fragment {
                range: FragmentRange {
                    head: *node.meta.hash(),
                    boundary,
                },
                commits: members
                    .into_iter()
                    .map(|idx| self.nodes[idx].meta.clone())
                    .collect(),
                signatures,
            }
        };
        if !item
            .dependencies()
            .all(|hash| self.has_admitted_commit(&hash))
        {
            return None;
        }
        item.verify().ok()
    }

    fn history(&self, heads: &[CommitHash]) -> Result<HashSet<CommitHash>, DiffError> {
        let mut history = HashSet::new();
        let mut stack = Vec::new();
        for hash in heads {
            stack.push(
                *self
                    .by_hash
                    .get(hash)
                    .ok_or(DiffError::MissingCommit(*hash))?,
            );
        }
        while let Some(idx) = stack.pop() {
            if history.insert(*self.nodes[idx].meta.hash()) {
                stack.extend(self.parents(idx));
            }
        }
        Ok(history)
    }

    fn participation(&self, view_point: &ViewPoint) -> Result<HashSet<CommitHash>, DiffError> {
        let history = self.history(&view_point.delivered_heads)?;
        let mut bounded = HashMap::new();
        for (author, permission) in &view_point.write_permissions {
            if let WritePermission::Bounded(frontier) = permission {
                // A bound outside the captured history supplies no proof in
                // this viewpoint, even if that commit has since arrived locally.
                let available = frontier
                    .iter()
                    .copied()
                    .filter(|hash| history.contains(hash))
                    .collect::<Vec<_>>();
                bounded.insert(*author, self.history(&available)?);
            }
        }
        Ok(history
            .into_iter()
            .filter(|hash| {
                let author = self.nodes[self.by_hash[hash]].meta.author();
                match view_point.write_permissions.get(author) {
                    Some(WritePermission::Unbounded) => true,
                    Some(WritePermission::Bounded(_)) => bounded[author].contains(hash),
                    Some(WritePermission::Denied) | None => false,
                }
            })
            .collect())
    }
}

/// An opaque node index, valid only in the graph that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeIdx(usize);

#[derive(Clone, Default)]
pub(crate) struct NodeStore {
    // Append-only, parents before children. Each node owns a contiguous parent
    // slice, preserving the envelope's parent order and multiplicity.
    entries: Vec<Node>,
    parent_ids: Vec<NodeIdx>,
}

impl NodeStore {
    fn parents(&self, node: NodeIdx) -> &[NodeIdx] {
        &self.parent_ids[self[node].parents.clone()]
    }

    // Inclusive membership of the level-one fragment at `head`. Stop at any
    // other checkpoint, including one received only as a standalone commit.
    fn level_one_members(&self, head: NodeIdx) -> HashSet<NodeIdx> {
        let mut members = HashSet::new();
        let mut stack = vec![head];
        while let Some(idx) = stack.pop() {
            if !members.insert(idx) {
                continue;
            }
            if idx == head || self[idx].depth == 0 {
                stack.extend(self.parents(idx));
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

    // Expand constituent links to level zero. Inclusive boundary commits may
    // appear in multiple level-one fragments; union their membership by node ID.
    fn members(&self, nodes: &NodeStore, fragment: FragmentIdx) -> HashSet<NodeIdx> {
        let mut members = HashSet::new();
        let mut visited = HashSet::new();
        let mut stack = vec![fragment];
        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            let fragment = &self[idx];
            if fragment.level == 1 {
                members.extend(nodes.level_one_members(fragment.head));
            } else {
                stack.extend(self.lower_fragments(nodes, fragment.head, fragment.level));
            }
        }
        members
    }

    // Derive the immediate constituents of a level > 1 fragment. Stop at
    // boundaries of that level, following only level-(n-1) fragment links.
    // No contents are stored, and overlapping paths are visited just once.
    //
    // Includes the same head's level-(n-1) fragment, then follows ancestors at
    // that level, stopping at boundaries of depth at least n.
    fn lower_fragments(&self, nodes: &NodeStore, head: NodeIdx, level: usize) -> Vec<FragmentIdx> {
        debug_assert!(level > 1);
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![nodes[head].headed_fragments[level - 2]];
        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            result.push(idx);
            for &boundary in self.boundary(idx) {
                let headed = &nodes[boundary].headed_fragments;
                if nodes[boundary].depth < level {
                    // A level-(n-1) boundary is at least that deep, so this
                    // boundary has depth exactly n-1 and heads that fragment.
                    stack.push(headed[level - 2]);
                }
            }
        }
        result
    }

    pub(crate) fn summary(
        &self,
        nodes: &NodeStore,
        eligible: &[bool],
        minimize: bool,
    ) -> crate::SedimentreeSummary {
        // Coverage needs only the deepest fragment reaching each node. Greater
        // levels dominate smaller ones; equal/deeper ancestors are boundaries.
        // Parent indices precede children, so all incoming coverage is available
        // when we visit a node. No fragment member lists are needed.
        let mut covering_level = vec![0; nodes.entries.len()];
        let mut selected = Vec::new();
        for (index, node) in nodes.entries.iter().enumerate().rev() {
            if let Some(fragment) = node.canonical_fragment().filter(|_| eligible[index]) {
                if !minimize || covering_level[index] <= node.depth {
                    selected.push(fragment);
                }
                covering_level[index] = covering_level[index].max(node.depth);
            }
            let level = covering_level[index];
            if level == 0 {
                continue;
            }
            for &parent in nodes.parents(NodeIdx(index)) {
                if nodes[parent].depth < level {
                    covering_level[parent.0] = covering_level[parent.0].max(level);
                }
            }
        }

        // Level-one boundaries are the edges of the checkpoint DAG. Enumerate
        // only checkpoints, never depth-zero contents or auxiliary levels. Reuse
        // a visitation array, stamped by output fragment, across overlapping walks.
        let mut visited = vec![0; nodes.entries.len()];
        let mut stack = Vec::new();
        let mut fragments = Vec::with_capacity(selected.len());
        for (epoch, idx) in selected.into_iter().enumerate() {
            let fragment = &self[idx];
            let mut checkpoints = BTreeSet::new();
            stack.extend_from_slice(self.boundary(nodes[fragment.head].headed_fragments[0]));
            while let Some(checkpoint) = stack.pop() {
                if visited[checkpoint.0] == epoch + 1 {
                    continue;
                }
                visited[checkpoint.0] = epoch + 1;
                let node = &nodes[checkpoint];
                if node.depth >= fragment.level {
                    continue;
                }
                checkpoints.insert(crate::Checkpoint(
                    node.meta.hash().as_bytes()[..12].try_into().unwrap(),
                ));
                // An interior checkpoint is indexed by admission of its enclosing
                // fragment. Unindexed standalone checkpoints can only be boundaries.
                stack.extend_from_slice(self.boundary(node.headed_fragments[0]));
            }
            fragments.push(crate::Fragment {
                range: crate::FragmentRange {
                    head: *nodes[fragment.head].meta.hash(),
                    boundary: self
                        .boundary(idx)
                        .iter()
                        .map(|&node| *nodes[node].meta.hash())
                        .collect(),
                },
                checkpoints,
            });
        }
        let mut loose_commits = Vec::new();
        for (idx, node) in nodes {
            if eligible[idx.0] && covering_level[idx.0] == 0 {
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
        assert_eq!(graph.signed_view_point().delivered_heads(), &[*head.hash()]);
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
