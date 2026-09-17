use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    ops::{ControlFlow, Index},
};

use crate::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, DiffError, Pov, PovDiff,
    ResolveError, ResolvedPov, Signature, SignatureError, WritePermission,
};

/// An in-memory commit DAG with causal delivery.
/// Commits with missing parents are queued until all their ancestors are available.
/// Queued commits are not exposed by graph queries or traversal.
#[derive(Clone)]
pub struct CommitGraph<P> {
    nodes: NodeStore,
    edges: EdgeStore,
    parents: Vec<Option<EdgeIdx>>,
    store: HashMap<NodeIdx, P>,
    by_hash: HashMap<CommitHash, NodeIdx>,
    heads: BTreeSet<CommitHash>,
    pending: VecDeque<(CommitMeta, P)>,
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
    pub fn heads(&self) -> impl Iterator<Item = CommitHash> {
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

    pub fn merge(&mut self, other: &Self) {
        for (idx, node) in &other.nodes {
            if !self.by_hash.contains_key(node.meta.hash()) {
                let data = other.store[&idx].clone();
                self.ingest_commit(node.meta.clone(), data);
            }
        }
    }

    /// Visit each ready commit once, parents before children, excluding queued commits.
    /// Returning `ControlFlow::Break(())` stops the entire traversal.
    pub fn traverse<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(&self, mut traversal: F) {
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
    pub fn traverse_since<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
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

    /// Heads of the causally delivered history, regardless of participation.
    /// This new API is a stub; `heads` remains available for existing callers.
    pub fn delivered_heads(&self) -> impl Iterator<Item = CommitHash> + '_ {
        std::iter::from_fn(|| todo!("delivered heads"))
    }

    /// Visit delivered commits, parents first, regardless of authentication or
    /// write permission. Returning `Break` stops traversal.
    pub fn traverse_delivered<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(&self, _traversal: F) {
        todo!("delivered traversal")
    }

    /// Visit delivered commits excluding `since` and their ancestors.
    pub fn traverse_delivered_since<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        _since: &[CommitHash],
        _traversal: F,
    ) {
        todo!("incremental delivered traversal")
    }

    /// Visit exportable commits, withholding unsigned local work and descendants
    /// that would expose it. Publication is separate from document participation.
    pub fn traverse_publishable<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(
        &self,
        _traversal: F,
    ) {
        todo!("publishable traversal")
    }

    /// Prepare an authored commit without signing or ingesting it.
    pub fn commit_by(&self, _author: Author, _contents: &[u8]) -> CommitMeta {
        todo!("authored commit preparation")
    }

    /// Ingest our own prepared commit for immediate local use. Keep it out of
    /// publication until covered by verified signatures.
    pub fn ingest_local_commit(&mut self, _meta: CommitMeta, _data: P) {
        todo!("local commit ingestion")
    }

    /// Verify and record signature evidence, possibly before its commit arrives.
    /// Does not grant write permission.
    pub fn ingest_signature(
        &mut self,
        _author: Author,
        _hash: CommitHash,
        _signature: Signature,
    ) -> Result<(), SignatureError> {
        todo!("signature ingestion")
    }

    /// Replace the entire permission map. Missing authors are denied; captured
    /// POVs keep their previous permissions.
    pub fn set_write_permissions(&mut self, _permissions: &HashMap<Author, WritePermission>) {
        todo!("write permissions")
    }

    /// Capture the working state, including our pending-signature local commits.
    pub fn pov(&self) -> Pov {
        todo!("working POV capture")
    }

    /// Capture the same history and permissions with signature-backed coverage.
    pub fn signed_pov(&self) -> Pov {
        todo!("signed POV capture")
    }

    /// Resolve against actual signature evidence. Pending signatures do not
    /// suffice, even for a POV captured locally.
    pub fn resolve(&self, _pov: &Pov) -> Result<ResolvedPov<'_>, ResolveError> {
        todo!("POV resolution")
    }

    /// Compare participation without certifying signature evidence. Received POVs
    /// must be resolved before they drive a materialized view.
    pub fn diff(&self, _before: &Pov, _after: &Pov) -> Result<PovDiff<'_>, DiffError> {
        todo!("POV comparison")
    }

    pub fn commit(&self, contents: &[u8]) -> CommitMeta {
        let commit_contents = CommitContents {
            parents: self.heads().collect(),
            signatures: Vec::new(),
            author: None,
            payload: BlobHash::of_contents(contents),
        };
        CommitMeta::new(commit_contents)
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
