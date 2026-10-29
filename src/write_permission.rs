use alloc::vec::Vec;
use crate::CommitHash;

/// Which commits an author may contribute. Missing authors are denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritePermission {
    /// No write-frontier bound; authentication and causal delivery still apply.
    Unbounded,
    /// Commits at or before at least one head. An empty frontier permits nothing.
    Bounded(Vec<CommitHash>),
    /// No commits may contribute.
    Denied,
}
