use std::collections::{HashMap, HashSet};

use crate::{Author, CommitHash, NodeIdx, QueryError, WritePermission};

/// An owned snapshot of delivered heads, write permissions, and authentication
/// frontiers. Describes participation, not the materialized document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pov {
    pub(crate) delivered_heads: Vec<CommitHash>,
    pub(crate) write_permissions: HashMap<Author, WritePermission>,
    pub(crate) authentication_frontiers: HashMap<Author, Vec<CommitHash>>,
}

impl Pov {
    /// Reconstruct a description, for example after transport or storage.
    /// This does not validate its claims; resolve received POVs against signatures
    /// before using them to drive a materialized view.
    pub fn from_parts(
        delivered_heads: Vec<CommitHash>,
        write_permissions: HashMap<Author, WritePermission>,
        authentication_frontiers: HashMap<Author, Vec<CommitHash>>,
    ) -> Self {
        Self {
            delivered_heads,
            write_permissions,
            authentication_frontiers,
        }
    }

    /// The captured causally complete history, including non-contributing commits.
    pub fn delivered_heads(&self) -> &[CommitHash] {
        &self.delivered_heads
    }

    /// Captured permissions, not the graph's current mutable permission map.
    pub fn write_permissions(&self) -> &HashMap<Author, WritePermission> {
        &self.write_permissions
    }

    /// Captured authentication coverage. In a working POV this may include
    /// pending local signatures; it is not itself signature evidence.
    pub fn authentication_frontiers(&self) -> &HashMap<Author, Vec<CommitHash>> {
        &self.authentication_frontiers
    }
}

/// A POV resolved against graph data and actual signature evidence.
#[derive(Debug)]
pub struct ResolvedPov<'graph> {
    pub(crate) known: &'graph HashMap<CommitHash, NodeIdx>,
    pub(crate) visible: HashSet<CommitHash>,
}

impl ResolvedPov<'_> {
    /// Whether a known commit contributes. An unknown hash is an error.
    pub fn visible(&self, commit: &CommitHash) -> Result<bool, QueryError> {
        if !self.known.contains_key(commit) {
            return Err(QueryError::UnknownCommit(*commit));
        }
        Ok(self.visible.contains(commit))
    }
}

/// A comparison of commit participation, not a document patch stream.
/// Does not certify the signature evidence for either POV.
#[derive(Debug)]
pub struct PovDiff<'graph> {
    pub(crate) known: &'graph HashMap<CommitHash, NodeIdx>,
    pub(crate) before: HashSet<CommitHash>,
    pub(crate) after: HashSet<CommitHash>,
}

impl PovDiff<'_> {
    /// Classify a known commit under the source and destination POVs.
    pub fn visibility(&self, commit: &CommitHash) -> Result<Vis, QueryError> {
        if !self.known.contains_key(commit) {
            return Err(QueryError::UnknownCommit(*commit));
        }
        Ok(
            match (self.before.contains(commit), self.after.contains(commit)) {
                (true, false) => Vis::Before,
                (false, true) => Vis::After,
                (true, true) => Vis::Both,
                (false, false) => Vis::Neither,
            },
        )
    }

    /// Each changed commit once. Does not report commits in both or neither POV.
    /// Traversal order is unspecified.
    pub fn changed_commits(&self) -> impl Iterator<Item = CommitDiff> + '_ {
        self.before
            .difference(&self.after)
            .map(|hash| CommitDiff {
                commit_id: *hash,
                visibility_delta: VisDiff::Before,
            })
            .chain(self.after.difference(&self.before).map(|hash| CommitDiff {
                commit_id: *hash,
                visibility_delta: VisDiff::After,
            }))
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

/// A commit whose participation differs between two POVs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitDiff {
    pub commit_id: CommitHash,
    pub visibility_delta: VisDiff,
}
