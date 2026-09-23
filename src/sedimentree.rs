//! A structural sedimentree summary of publishable history.
//!
//! This is not an inventory of encoded blobs or a proof of fragment-local author
//! coverage. Encoding, blob metadata, sealing, and reconciliation are out of scope.

use std::{collections::BTreeSet, marker::PhantomData};

use crate::CommitHash;

/// Inclusive head and boundary commits of a canonical fragment.
/// A walk stops at ancestors at least as deep as the head, or at roots.
/// Roots below the head's depth are included without becoming boundaries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FragmentRange {
    pub head: CommitHash,
    pub boundary: BTreeSet<CommitHash>,
}

/// Truncated interior checkpoint identifier. Never used as an internal index key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Checkpoint(pub [u8; 12]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseCommit {
    pub commit: CommitHash,
    pub parents: BTreeSet<CommitHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub range: FragmentRange,
    /// Nonzero-depth interior commits, excluding the head and boundaries.
    pub checkpoints: BTreeSet<Checkpoint>,
}

/// A deterministic, reduced structural cover for one graph.
///
/// Includes publishable delivered history, independently of write permissions
/// and viewpoints. Uses the graph's existing publication rules, not the stronger
/// proof requirements that independently verifiable blobs will eventually need.
/// Entries are sorted by full commit hash / fragment range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedimentreeSummary {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

/// Borrows maintained metadata indexes, never commit payloads.
/// Creating a view and reading its summary do not decompose the graph again.
pub struct SedimentreeView<'graph> {
    _graph: PhantomData<&'graph ()>,
}

impl SedimentreeView<'_> {
    /// Copy the current cover's metadata, with work proportional to summary size.
    /// No payload access, graph traversal, encoding, or IO is performed.
    pub fn summary(&self) -> SedimentreeSummary {
        todo!()
    }
}
