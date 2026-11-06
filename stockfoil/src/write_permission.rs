use crate::CommitHash;

/// Authorization rule for a single [`Author`](crate::Author) within a
/// document's [`WritePermissions`](crate::WritePermissions) map.
///
/// While Ed25519 signatures prove *who* authored a commit, `WritePermission`
/// decides whether that author's commits participate in materialized state.
/// Authors omitted from a [`WritePermissions`](crate::WritePermissions) map get
/// its unlisted rule, [`Denied`](Self::Denied) by default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WritePermission {
    /// All of this author's commits in the selected history participate.
    Unbounded,
    /// Only commits by this author in the causal past of (ancestors of, or
    /// equal to) at least one `frontier` head *within the selected history*
    /// participate.
    ///
    /// Use a bounded frontier to revoke an author after a known point in causal
    /// history while preserving their earlier work. An empty frontier permits
    /// nothing.
    Bounded(Vec<CommitHash>),
    /// None of this author's commits participate.
    #[default]
    Denied,
}
