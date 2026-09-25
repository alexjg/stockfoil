use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    Author, CommitHash, NodeIdx, QueryError, WritePermission,
    ancestor_clock::AncestorClock,
    commit_graph::{Node, NodeStore},
};

/// An owned snapshot of history heads and write permissions.
/// Describes participation, not the materialized document or signature status.
/// Working snapshots may include local drafts; signed snapshots include only
/// admitted history. Later admission never changes a captured snapshot's meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewPoint {
    pub(crate) heads: Vec<CommitHash>,
    pub(crate) write_permissions: HashMap<Author, WritePermission>,
}

impl ViewPoint {
    /// Reconstruct a description, for example after transport or storage.
    /// This does not validate its claims; resolve received viewpoints against
    /// admitted history before using them to drive a materialized view.
    pub fn from_parts(
        heads: Vec<CommitHash>,
        write_permissions: HashMap<Author, WritePermission>,
    ) -> Self {
        Self {
            heads,
            write_permissions,
        }
    }

    /// The captured causally complete history, including non-contributing commits.
    pub fn heads(&self) -> &[CommitHash] {
        &self.heads
    }

    /// Captured permissions, not the graph's current mutable permission map.
    pub fn write_permissions(&self) -> &HashMap<Author, WritePermission> {
        &self.write_permissions
    }
}

/// Graph-local compiled membership predicates, not an enumeration of visible
/// commits. Visibility need not be parent-closed, so bounds remain per author.
#[derive(Debug)]
pub(crate) struct PreparedView {
    pub(crate) history: Arc<AncestorClock>,
    pub(crate) unbounded: HashSet<Author>,
    pub(crate) bounded: HashMap<Author, Arc<AncestorClock>>,
}

impl PreparedView {
    fn contains(&self, idx: NodeIdx, node: &Node) -> bool {
        self.history.contains(node.chain, idx)
            && (self.unbounded.contains(node.meta.author())
                || self
                    .bounded
                    .get(node.meta.author())
                    .is_some_and(|clock| clock.contains(node.chain, idx)))
    }
}

/// A viewpoint resolved against verified, causally admitted graph history.
#[derive(Debug)]
pub struct ResolvedViewPoint<'graph> {
    pub(crate) known: &'graph HashMap<CommitHash, NodeIdx>,
    pub(crate) nodes: &'graph NodeStore,
    pub(crate) visible: PreparedView,
}

impl ResolvedViewPoint<'_> {
    /// Whether a known commit contributes. An unknown hash is an error.
    pub fn visible(&self, commit: &CommitHash) -> Result<bool, QueryError> {
        let &idx = self
            .known
            .get(commit)
            .ok_or(QueryError::UnknownCommit(*commit))?;
        Ok(self.visible.contains(idx, &self.nodes[idx]))
    }
}

/// A comparison of commit participation, not a document patch stream.
/// Does not require either viewpoint's history to have been admitted.
#[derive(Debug)]
pub struct ViewPointDiff<'graph> {
    pub(crate) known: &'graph HashMap<CommitHash, NodeIdx>,
    pub(crate) nodes: &'graph NodeStore,
    pub(crate) before: PreparedView,
    pub(crate) after: PreparedView,
}

impl ViewPointDiff<'_> {
    /// Classify a known commit under the source and destination viewpoints.
    pub fn visibility(&self, commit: &CommitHash) -> Result<Vis, QueryError> {
        let &idx = self
            .known
            .get(commit)
            .ok_or(QueryError::UnknownCommit(*commit))?;
        let node = &self.nodes[idx];
        Ok(
            match (
                self.before.contains(idx, node),
                self.after.contains(idx, node),
            ) {
                (true, false) => Vis::Before,
                (false, true) => Vis::After,
                (true, true) => Vis::Both,
                (false, false) => Vis::Neither,
            },
        )
    }

    /// Each changed commit once. Does not report commits in both or neither viewpoint.
    /// Traversal order is unspecified.
    pub fn changed_commits(&self) -> impl Iterator<Item = CommitDiff> + '_ {
        let latest = self
            .before
            .history
            .latest()
            .max(self.after.history.latest());
        self.nodes
            .into_iter()
            .take_while(move |(idx, _)| latest.is_some_and(|last| *idx <= last))
            .filter_map(|(idx, node)| {
                let visibility_delta = match (
                    self.before.contains(idx, node),
                    self.after.contains(idx, node),
                ) {
                    (true, false) => VisDiff::Before,
                    (false, true) => VisDiff::After,
                    _ => return None,
                };
                Some(CommitDiff {
                    commit_id: *node.meta.hash(),
                    visibility_delta,
                })
            })
    }
}

/// Which side of a comparison a commit contributes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    Before,
    After,
    Both,
    Neither,
}

/// The direction of a change in participation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisDiff {
    Before,
    After,
}

/// A commit whose participation differs between two viewpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitDiff {
    pub commit_id: CommitHash,
    pub visibility_delta: VisDiff,
}
