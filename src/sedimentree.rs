//! Certified transport inventories and local structural sedimentree inspection.
//!
//! Encoding, blob metadata, sealing, and reconciliation are out of scope.

use std::collections::{BTreeSet, HashMap};

use crate::{CommitHash, commit_graph::NodeIdx};

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

/// An authenticated commit not introduced by an indexed fragment. It can have
/// any depth; a signed checkpoint announcement does not certify its fragment.
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

/// A deterministic structural summary for one graph.
///
/// [`SedimentreeView::summary`] includes every canonical fragment, whereas
/// [`SedimentreeView::minimized_summary`] omits structurally contained fragments.
/// Loose commits are exactly the publishable delivered commits covered by no
/// fragment, and are the same in both representations.
///
/// Independent of write permissions and viewpoints. Uses the graph's existing
/// publication rules, not item-local signature availability. Use
/// [`crate::CommitGraph::minimal_cover`] for the maintained transport inventory.
/// Entries are sorted by full commit hash
/// / fragment range; checkpoint identifiers are truncated only in metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedimentreeSummary {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

/// An owned inventory of the current certified structural minimal cover.
/// Every entry is exportable while it remains in the graph's current cover.
/// This snapshot does not retain replaced entries or their certificates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MinimalCover {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

/// Borrows maintained topology metadata, never commit payloads.
/// Creating a view computes publication eligibility without rebuilding fragments.
pub struct SedimentreeView<'graph> {
    pub(crate) nodes: &'graph crate::commit_graph::NodeStore,
    pub(crate) fragments: &'graph crate::commit_graph::FragmentStore,
    pub(crate) by_hash: &'graph HashMap<CommitHash, NodeIdx>,
    pub(crate) publishable: Vec<bool>,
}

impl SedimentreeView<'_> {
    /// Return an owned, non-minimal snapshot of publishable history.
    /// Includes every indexed canonical, maximum-level fragment, even if
    /// contained in another. Standalone signed heads do not imply fragments.
    /// Lower-level records at the same head are internal traversal structure,
    /// not separate summary entries.
    ///
    /// Walks fragment links to collect checkpoints and level-one commit parents
    /// to identify covered commits. Boundaries are read from the maintained
    /// index. No payload access, encoding, or IO is performed.
    pub fn summary(&self) -> SedimentreeSummary {
        self.fragments.summary(
            self.nodes,
            self.by_hash,
            &self.publishable,
            &self.publishable,
            false,
        )
    }

    /// Return the minimal cover under structural containment: recursively,
    /// higher-level fragments contain their lower-level constituent fragments.
    /// Boundary commits are included without including the fragments they head.
    ///
    /// Same-level overlaps (even accidental whole-range containment) are kept.
    /// This is not minimum set cover or exhaustive commit-set containment
    /// reduction. Uses the same metadata-only traversals as [`Self::summary`].
    pub fn minimized_summary(&self) -> SedimentreeSummary {
        self.fragments.summary(
            self.nodes,
            self.by_hash,
            &self.publishable,
            &self.publishable,
            true,
        )
    }
}
