use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::{ControlFlow, Index},
};

use crate::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, DiffError, IngestReport,
    ResolveError, ResolvedViewPoint, SedimentreeView, Signature, SignatureError, VerifiedItem,
    ViewPoint, ViewPointDiff, WritePermission,
};

/// An in-memory commit DAG with causal delivery.
/// Commits with missing parents are queued until all their ancestors are available.
/// Queued commits are not exposed by graph queries or traversal.
///
/// `P` is opaque: it can be decoded operations, a storage reference, or application
/// metadata. Signatures authenticate the commit envelope and its payload hash,
/// not an arbitrary `P`. Callers must validate received payload bytes against
/// [`CommitMeta::payload_hash`] before ingestion or use.
///
/// Cloning preserves local provenance. To create another replica without sharing
/// unsigned local trust, create a fresh graph and use [`Self::merge`].
#[derive(Clone)]
pub struct CommitGraph<P> {
    nodes: NodeStore,
    edges: EdgeStore,
    parents: Vec<Option<EdgeIdx>>,
    store: HashMap<NodeIdx, P>,
    by_hash: HashMap<CommitHash, NodeIdx>,
    heads: BTreeSet<CommitHash>,
    pending: VecDeque<(CommitMeta, P)>,
    permissions: HashMap<Author, WritePermission>,
    // Verified signatures, including ones whose commits haven't arrived yet.
    signatures: HashMap<Author, BTreeMap<CommitHash, Signature>>,
    // Local provenance never travels through merge. Only this path grants
    // optimistic authentication or holds otherwise-delivered data off the wire.
    local: HashSet<CommitHash>,
}

#[derive(Clone)]
struct Node {
    meta: CommitMeta,
}

impl<P: Clone> Default for CommitGraph<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Clone> CommitGraph<P> {
    pub fn new() -> Self {
        Self {
            nodes: NodeStore(Vec::new()),
            edges: EdgeStore(Vec::new()),
            store: HashMap::new(),
            parents: Vec::new(),
            by_hash: HashMap::new(),
            heads: BTreeSet::new(),
            pending: VecDeque::new(),
            permissions: HashMap::new(),
            signatures: HashMap::new(),
            local: HashSet::new(),
        }
    }

    pub fn parents(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        let mut edge_idx = self.parents[node_idx.0];
        std::iter::from_fn(move || {
            let this_edge_idx = edge_idx?;
            let edge = &self.edges[this_edge_idx];
            edge_idx = edge.next;
            Some(edge.target)
        })
    }

    pub fn children(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        (0..self.nodes.0.len())
            .map(NodeIdx)
            .filter(move |&child| self.parents(child).any(|parent| parent == node_idx))
    }

    pub fn roots(&self) -> impl Iterator<Item = NodeIdx> {
        self.parents
            .iter()
            .enumerate()
            .filter_map(|(i, edge)| edge.is_none().then_some(NodeIdx(i)))
    }

    /// Return hashes of ready commits with no children, in hash order.
    /// Queued commits neither appear as heads nor remove their parents from the heads.
    /// Iterating all heads takes O(h) time for h heads, without scanning the graph.
    pub fn delivered_heads(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.heads.iter().copied()
    }

    /// Whether `ancestor` is a strict ancestor of `descendant` via parent edges.
    /// Returns false for identical hashes or if either commit is unknown or queued.
    /// Walks the existing graph on demand, without storing a transitive ancestry index.
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

    /// Ingest a commit, queuing it if any parents are missing.
    /// The caller must bind `data` to [`CommitMeta::payload_hash`]; this generic
    /// container has no way to encode or verify an arbitrary `P`.
    /// Also inserts any queued commits made ready by this commit, recursively.
    /// Duplicates (including queued commits) keep their original payload.
    /// Commits whose dependencies never arrive remain queued indefinitely.
    pub fn ingest_commit(&mut self, meta: CommitMeta, data: P) {
        if self.by_hash.contains_key(meta.hash())
            || self
                .pending
                .iter()
                .any(|(pending, _)| pending.hash() == meta.hash())
        {
            return;
        }
        self.pending.push_back((meta, data));

        // Retry the queue until a complete pass makes no progress. A blocked
        // commit must not prevent unrelated, ready commits from being inserted.
        loop {
            let pending_count = self.pending.len();
            for _ in 0..pending_count {
                let (meta, data) = self.pending.pop_front().unwrap();
                if meta
                    .parents()
                    .iter()
                    .all(|hash| self.by_hash.contains_key(hash))
                {
                    self.insert_ready(meta, data);
                } else {
                    self.pending.push_back((meta, data));
                }
            }
            if self.pending.is_empty() || self.pending.len() == pending_count {
                break;
            }
        }
        self.prune_signatures();
    }

    // Only called once all parents have been inserted into the graph.
    fn insert_ready(&mut self, meta: CommitMeta, data: P) {
        let hash = *meta.hash();
        let parents = meta
            .parents()
            .iter()
            .map(|hash| self.by_hash[hash])
            .collect::<Vec<_>>();

        let mut first_edge = None;
        for parent in parents.into_iter().rev() {
            let edge_idx = EdgeIdx(self.edges.0.len());
            self.edges.0.push(Edge {
                next: first_edge,
                target: parent,
            });
            first_edge = Some(edge_idx);
        }

        let node_idx = NodeIdx(self.nodes.0.len());
        self.parents.push(first_edge);
        self.store.insert(node_idx, data);
        for parent in meta.parents() {
            self.heads.remove(parent);
        }
        self.heads.insert(hash);
        self.nodes.0.push(Node { meta });
        self.by_hash.insert(hash, node_idx);
    }

    /// Borrow the sedimentree export view. Only sealed items with their own
    /// author proofs may be exported. The boundary index is not implemented yet.
    pub fn as_sedimentree(&self) -> SedimentreeView<'_, P> {
        todo!("sedimentree boundary indexes and export view")
    }

    /// Accept an independently verified item, retaining permanent fragment proofs.
    /// External parents can still delay causal delivery. Never grants local trust
    /// or imports write permissions. No ingestion behavior is implemented yet.
    pub fn ingest_verified(&mut self, _item: VerifiedItem<P>) -> IngestReport {
        todo!("verified sedimentree ingestion")
    }

    /// Exchange publishable history and signature evidence, never permissions or
    /// local trust. Received detached commits can be forwarded too.
    pub fn merge(&mut self, other: &Self) {
        let publishable = other.publishable_nodes();
        let mut blocked = other
            .nodes
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| !publishable[*i])
            .map(|(_, node)| *node.meta.hash())
            .collect::<HashSet<_>>();
        // Keep queued local work, and any queued descendants of withheld work,
        // off the wire regardless of the order in which they were received.
        blocked.extend(
            other
                .pending
                .iter()
                .filter(|(meta, _)| other.local.contains(meta.hash()))
                .map(|(meta, _)| *meta.hash()),
        );
        loop {
            let before = blocked.len();
            for (meta, _) in &other.pending {
                if meta.parents().iter().any(|p| blocked.contains(p)) {
                    blocked.insert(*meta.hash());
                }
            }
            if blocked.len() == before {
                break;
            }
        }
        for (author, signatures) in &other.signatures {
            for (hash, signature) in signatures {
                self.ingest_signature(*author, *hash, *signature)
                    .expect("source graph only stores verified signatures");
            }
        }
        for (idx, node) in &other.nodes {
            if publishable[idx.0] {
                self.ingest_commit(node.meta.clone(), other.store[&idx].clone());
            }
        }
        for (meta, data) in &other.pending {
            if !blocked.contains(meta.hash()) {
                self.ingest_commit(meta.clone(), data.clone());
            }
        }
    }

    /// Visit each ready commit once, parents before children, excluding queued commits.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
    pub fn traverse_delivered<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        mut traversal: F,
    ) {
        // Only ready commits are inserted, so node order is already topological.
        for (idx, node) in &self.nodes {
            if traversal(&node.meta, &self.store[&idx]).is_break() {
                break;
            }
        }
    }

    /// Visit ready commits excluding `since` and all their ancestors, parents first.
    /// An empty `since` visits all ready commits. Unknown or queued hashes are ignored.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
    pub fn traverse_delivered_since<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        since: &[CommitHash],
        mut traversal: F,
    ) {
        let mut excluded = vec![false; self.nodes.0.len()];
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

    /// Visit exportable delivered commits, parents first, withholding unsigned
    /// local work and descendants that would expose it. Returning `Break` stops
    /// traversal. Publication is separate from document participation.
    pub fn traverse_publishable<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        mut traversal: F,
    ) {
        let publishable = self.publishable_nodes();
        for (idx, node) in &self.nodes {
            if publishable[idx.0] && traversal(&node.meta, &self.store[&idx]).is_break() {
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

    /// Ingest our own prepared commit for immediate local use. Keep it out of
    /// publication until covered by verified signatures. The caller is responsible
    /// for owning the author's signing key; never use this path for received data.
    /// Duplicate ingestion does not upgrade received data to local trust.
    pub fn ingest_local_commit(&mut self, meta: CommitMeta, data: P) {
        if self.by_hash.contains_key(meta.hash())
            || self
                .pending
                .iter()
                .any(|(known, _)| known.hash() == meta.hash())
        {
            return;
        }
        self.local.insert(*meta.hash());
        self.ingest_commit(meta, data);
    }

    /// Verify a signature over the raw commit hash, possibly before the commit
    /// arrives. Coverage becomes usable after causal delivery. A signer attests
    /// only to that signer's authored ancestors, never to another author's claims.
    /// Does not grant write permission.
    pub fn ingest_signature(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<(), SignatureError> {
        let key = ed25519_dalek::VerifyingKey::from_bytes(author.as_array())
            .map_err(|_| SignatureError::InvalidPublicKey(author))?;
        let proof = ed25519_dalek::Signature::from_bytes(&signature.into());
        key.verify_strict(hash.as_bytes(), &proof).map_err(|_| {
            SignatureError::InvalidSignature {
                author,
                commit: hash,
            }
        })?;
        self.signatures
            .entry(author)
            .or_default()
            .insert(hash, signature);
        self.prune_signatures();
        Ok(())
    }

    /// Replace the entire permission map. Missing authors are denied; captured
    /// viewpoints keep their previous permissions.
    pub fn set_write_permissions(&mut self, permissions: &HashMap<Author, WritePermission>) {
        self.permissions = permissions.clone();
    }

    /// Capture the working state, including our pending-signature local commits.
    pub fn view_point(&self) -> ViewPoint {
        let mut view_point = self.signed_view_point();
        for hash in &self.local {
            if let Some(idx) = self.by_hash.get(hash) {
                let author = *self.nodes[*idx].meta.author();
                view_point
                    .authentication_frontiers
                    .entry(author)
                    .or_default()
                    .push(*hash);
            }
        }
        for frontier in view_point.authentication_frontiers.values_mut() {
            *frontier = self.frontier(frontier.iter().copied());
        }
        view_point
    }

    /// Capture the same history and permissions with signature-backed coverage.
    pub fn signed_view_point(&self) -> ViewPoint {
        let authentication_frontiers = self
            .signatures
            .iter()
            .filter_map(|(author, signatures)| {
                let heads = self.frontier(
                    signatures
                        .keys()
                        .copied()
                        .filter(|hash| self.by_hash.contains_key(hash)),
                );
                (!heads.is_empty()).then_some((*author, heads))
            })
            .collect();
        ViewPoint {
            delivered_heads: self.delivered_heads().collect(),
            write_permissions: self.permissions.clone(),
            authentication_frontiers,
        }
    }

    /// Resolve commit metadata against actual signature evidence, not the opaque
    /// payload values `P`. Pending signatures do not suffice, even for a viewpoint
    /// captured locally. Later signatures may validate an
    /// old frontier, but never expand its captured participation.
    pub fn resolve(&self, view_point: &ViewPoint) -> Result<ResolvedViewPoint<'_>, ResolveError> {
        let visible = self.participation(view_point)?;
        for (author, frontier) in &view_point.authentication_frontiers {
            for hash in frontier {
                if !self.signature_covers(author, hash) {
                    return Err(ResolveError::MissingSignature {
                        author: *author,
                        frontier: *hash,
                    });
                }
            }
        }
        Ok(ResolvedViewPoint {
            known: &self.by_hash,
            visible,
        })
    }

    /// Compare participation without certifying signature evidence. Received viewpoints
    /// must be resolved before they drive a materialized view.
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

    fn covers(&self, ancestor: &CommitHash, descendant: &CommitHash) -> bool {
        ancestor == descendant || self.is_ancestor(ancestor, descendant)
    }

    // All candidates are delivered. Walk their strict ancestors together rather
    // than testing every pair, which is costly for long local edit runs.
    fn frontier(&self, hashes: impl IntoIterator<Item = CommitHash>) -> Vec<CommitHash> {
        let hashes = hashes.into_iter().collect::<BTreeSet<_>>();
        let mut ancestors = HashSet::new();
        let mut stack = hashes
            .iter()
            .flat_map(|hash| self.parents(self.by_hash[hash]))
            .collect::<Vec<_>>();
        while let Some(idx) = stack.pop() {
            if ancestors.insert(idx) {
                stack.extend(self.parents(idx));
            }
        }
        hashes
            .into_iter()
            .filter(|hash| !ancestors.contains(&self.by_hash[hash]))
            .collect()
    }

    fn signature_covers(&self, author: &Author, hash: &CommitHash) -> bool {
        self.signatures.get(author).is_some_and(|signatures| {
            signatures
                .keys()
                .any(|tip| self.by_hash.contains_key(tip) && self.covers(hash, tip))
        })
    }

    fn publishable_nodes(&self) -> Vec<bool> {
        let mut coverage = HashMap::new();
        for (author, signatures) in &self.signatures {
            let heads = signatures
                .keys()
                .copied()
                .filter(|hash| self.by_hash.contains_key(hash))
                .collect::<Vec<_>>();
            coverage.insert(
                *author,
                self.history(&heads).expect("delivered signature targets"),
            );
        }
        let mut publishable = vec![false; self.nodes.0.len()];
        for (idx, node) in &self.nodes {
            let signed_or_received = !self.local.contains(node.meta.hash())
                || coverage
                    .get(node.meta.author())
                    .is_some_and(|covered| covered.contains(node.meta.hash()));
            publishable[idx.0] =
                signed_or_received && self.parents(idx).all(|parent| publishable[parent.0]);
        }
        publishable
    }

    fn prune_signatures(&mut self) {
        if self
            .signatures
            .values()
            .all(|signatures| signatures.len() < 2)
        {
            return;
        }
        // Don't discard the only exportable proof in favor of a signature whose
        // ancestry is still withheld. This matters for mixed-author local chains.
        let publishable = self.publishable_nodes();
        let mut obsolete = Vec::new();
        for (author, signatures) in &self.signatures {
            for hash in signatures.keys() {
                if signatures.keys().any(|newer| {
                    self.by_hash
                        .get(newer)
                        .is_some_and(|idx| publishable[idx.0])
                        && self.is_ancestor(hash, newer)
                }) {
                    obsolete.push((*author, *hash));
                }
            }
        }
        for (author, hash) in obsolete {
            self.signatures.get_mut(&author).unwrap().remove(&hash);
        }
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
        let mut authenticated = HashMap::new();
        for (author, frontier) in &view_point.authentication_frontiers {
            authenticated.insert(*author, self.history(frontier)?);
        }
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
                let authenticated = authenticated
                    .get(author)
                    .is_some_and(|covered| covered.contains(hash));
                let permitted = match view_point.write_permissions.get(author) {
                    Some(WritePermission::Unbounded) => true,
                    Some(WritePermission::Bounded(_)) => bounded[author].contains(hash),
                    Some(WritePermission::Denied) | None => false,
                };
                authenticated && permitted
            })
            .collect())
    }
}

/// An opaque node index, valid only in the graph that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeIdx(usize);

#[derive(Clone)]
struct NodeStore(Vec<Node>);

impl<'a> IntoIterator for &'a NodeStore {
    type Item = (NodeIdx, &'a Node);

    type IntoIter = NodeIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        NodeIter(self.0.iter().enumerate())
    }
}

struct NodeIter<'a>(std::iter::Enumerate<std::slice::Iter<'a, Node>>);

impl<'a> Iterator for NodeIter<'a> {
    type Item = (NodeIdx, &'a Node);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(count, node)| (NodeIdx(count), node))
    }
}

impl Index<NodeIdx> for NodeStore {
    type Output = Node;

    fn index(&self, index: NodeIdx) -> &Self::Output {
        &self.0[index.0]
    }
}

#[derive(Clone)]
struct Edge {
    next: Option<EdgeIdx>,
    target: NodeIdx,
}

#[derive(Clone)]
struct EdgeStore(Vec<Edge>);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EdgeIdx(usize);

impl Index<EdgeIdx> for EdgeStore {
    type Output = Edge;

    fn index(&self, index: EdgeIdx) -> &Self::Output {
        &self.0[index.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn pruning_keeps_exportable_evidence_until_newer_proof_is_exportable() {
        let alice_key = SigningKey::from_bytes(&[1; 32]);
        let bob_key = SigningKey::from_bytes(&[2; 32]);
        let alice = Author::try_from(alice_key.verifying_key().as_bytes().as_slice()).unwrap();
        let bob = Author::try_from(bob_key.verifying_key().as_bytes().as_slice()).unwrap();
        let mut graph = CommitGraph::new();
        graph.set_write_permissions(&HashMap::from([(alice, WritePermission::Unbounded)]));
        let root = graph.commit_by(alice, &[1]);
        graph.ingest_local_commit(root.clone(), 1u8);
        graph
            .ingest_signature(
                alice,
                *root.hash(),
                Signature::from(alice_key.sign(root.hash().as_bytes()).to_bytes()),
            )
            .unwrap();
        let old_view_point = graph.view_point();

        let middle = graph.commit_by(bob, &[2]);
        graph.ingest_local_commit(middle.clone(), 2);
        let tip = graph.commit_by(alice, &[3]);
        graph.ingest_local_commit(tip.clone(), 3);
        graph
            .ingest_signature(
                alice,
                *tip.hash(),
                Signature::from(alice_key.sign(tip.hash().as_bytes()).to_bytes()),
            )
            .unwrap();
        assert_eq!(graph.signatures[&alice].len(), 2);
        let mut peer = CommitGraph::new();
        peer.merge(&graph);
        assert_eq!(
            peer.delivered_heads().collect::<Vec<_>>(),
            vec![*root.hash()]
        );
        assert!(
            peer.resolve(&old_view_point)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );

        graph
            .ingest_signature(
                bob,
                *middle.hash(),
                Signature::from(bob_key.sign(middle.hash().as_bytes()).to_bytes()),
            )
            .unwrap();
        assert_eq!(graph.signatures[&alice].len(), 1);
        assert!(graph.signatures[&alice].contains_key(tip.hash()));
        assert!(
            graph
                .resolve(&old_view_point)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );
        peer.merge(&graph);
        assert_eq!(peer.signatures[&alice].len(), 1);
        assert!(
            peer.resolve(&old_view_point)
                .unwrap()
                .visible(root.hash())
                .unwrap()
        );
    }
}
