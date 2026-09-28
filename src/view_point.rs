use std::collections::HashMap;

use crate::{
    Author, CommitGraph, CommitHash, QueryError, WritePermission, ancestor_clock::AncestorClock,
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
    /// This does not validate its claims. [`CommitGraph::diff`] requires every
    /// history head to be delivered, but does not check admission or signatures.
    /// Callers requiring admitted history can check each head with
    /// [`CommitGraph::has_commit`].
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

/// A comparison of commit participation, not a document patch stream.
/// Does not require either viewpoint's history to have been admitted.
pub struct ViewPointDiff<'graph, P: Clone> {
    pub(crate) graph: &'graph CommitGraph<P>,
    // Permission-masked chain prefixes, not parent-closed ancestry summaries.
    pub(crate) before: AncestorClock,
    pub(crate) after: AncestorClock,
}

impl<'a, P: Clone> ViewPointDiff<'a, P> {
    /// Classify a known commit under the source and destination viewpoints.
    pub fn visibility(&self, commit: &CommitHash) -> Result<Vis, QueryError> {
        let time = self
            .graph
            .commit_time(commit)
            .ok_or(QueryError::UnknownCommit(*commit))?;
        Ok(match (
            self.before.contains_time(time),
            self.after.contains_time(time),
        ) {
            (true, false) => Vis::Before,
            (false, true) => Vis::After,
            (true, true) => Vis::Both,
            (false, false) => Vis::Neither,
        })
    }

    /// Each changed commit once. Does not report commits in both or neither viewpoint.
    /// Traversal order is unspecified.
    pub fn changed_commits(&self) -> impl Iterator<Item = CommitDiff> + '_ {
        self.graph.changed_commits(&self.before, &self.after)
    }
}

impl<'a, P: Clone> std::fmt::Debug for ViewPointDiff<'a, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewPointDiff")
            .field("before", &self.before)
            .field("after", &self.after)
            .finish()
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
