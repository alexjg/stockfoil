//! A structural sedimentree summary of publishable history.
//!
//! This is not an inventory of encoded blobs or a proof of fragment-local author
//! coverage. Encoding, blob metadata, sealing, and reconciliation are out of scope.

use std::collections::BTreeSet;

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

/// A deterministic, non-minimal structural summary for one graph.
///
/// Contains one canonical fragment for every publishable delivered commit of
/// nonzero depth, including fragments wholly contained in others. Loose commits
/// are exactly the publishable delivered commits covered by no fragment.
/// Minimization is a separate, deferred operation.
///
/// Independent of write permissions and viewpoints. Uses the graph's existing
/// publication rules, not the stronger proof requirements that independently
/// verifiable blobs will eventually need. Entries are sorted by full commit hash
/// / fragment range; checkpoint identifiers are truncated only in metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedimentreeSummary {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

/// Borrows maintained topology metadata, never commit payloads.
/// Creating a view computes publication eligibility without rebuilding fragments.
pub struct SedimentreeView<'graph> {
    pub(crate) nodes: &'graph crate::commit_graph::NodeStore,
    pub(crate) fragments: &'graph crate::commit_graph::FragmentStore,
    pub(crate) publishable: Vec<bool>,
}

impl SedimentreeView<'_> {
    /// Return an owned, non-minimal snapshot of publishable history.
    /// Scans the node and fragment indexes to select uncovered loose commits and
    /// sort the output. Does not traverse parent edges, rebuild fragment ranges,
    /// access payloads, encode anything, or perform IO.
    pub fn summary(&self) -> SedimentreeSummary {
        self.fragments.summary(self.nodes, &self.publishable)
    }
}
