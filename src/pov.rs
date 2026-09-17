use std::{collections::HashMap, marker::PhantomData};

use crate::{Author, CommitHash, QueryError, WritePermission};

/// An owned snapshot of delivered heads, write permissions, and authentication
/// frontiers. Describes participation, not the materialized document.
///
/// The representation and accessors are not implemented yet.
#[derive(Debug, Clone)]
pub struct Pov {
    _private: (),
}

impl Pov {
    /// The captured causally complete history, including non-contributing commits.
    pub fn delivered_heads(&self) -> &[CommitHash] {
        todo!("POV delivered heads")
    }

    /// Captured permissions, not the graph's current mutable permission map.
    pub fn write_permissions(&self) -> &HashMap<Author, WritePermission> {
        todo!("POV write permissions")
    }

    /// Captured authentication coverage. In a working POV this may include
    /// pending local signatures; it is not itself signature evidence.
    pub fn authentication_frontiers(&self) -> &HashMap<Author, Vec<CommitHash>> {
        todo!("POV authentication frontiers")
    }
}

/// A POV resolved against graph data and actual signature evidence.
#[derive(Debug)]
pub struct ResolvedPov<'graph> {
    _private: PhantomData<&'graph ()>,
}

impl ResolvedPov<'_> {
    /// Whether a known commit contributes. An unknown hash is an error.
    pub fn visible(&self, _commit: &CommitHash) -> Result<bool, QueryError> {
        todo!("resolved POV participation")
    }
}

/// A comparison of commit participation, not a document patch stream.
#[derive(Debug)]
pub struct PovDiff<'graph> {
    _private: PhantomData<&'graph ()>,
}

impl PovDiff<'_> {
    /// Classify a known commit under the source and destination POVs.
    pub fn visibility(&self, _commit: &CommitHash) -> Result<Vis, QueryError> {
        todo!("POV diff participation")
    }

    /// Each changed commit once. Does not report commits in both or neither POV.
    /// Traversal order is unspecified.
    pub fn changed_commits(&self) -> impl Iterator<Item = CommitDiff> + '_ {
        std::iter::from_fn(|| todo!("changed commits"))
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
