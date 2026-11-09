use super::commit_hash::CommitHash;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) fn same_heads(a: &[CommitHash], b: &[CommitHash]) -> (out: bool)
    ensures out == (a@ == b@),
{
    if a.len() != b.len() { return false; }
    let mut i = 0usize;
    while i < a.len()
        invariant i <= a.len(), a.len() == b.len(),
            forall|j: int| 0 <= j < i ==> a[j] == b[j],
        decreases a.len() - i,
    {
        if a[i].0 != b[i].0 { return false; }
        assert(a[i as int].0 =~= b[i as int].0);
        i += 1;
    }
    assert(a@ =~= b@);
    true
}

/// Authorization rule for a single [`Author`](crate::Author) within a
/// document's [`WritePermissions`](crate::WritePermissions) map.
///
/// While Ed25519 signatures prove *who* authored a commit, `WritePermission`
/// decides whether that author's commits participate in materialized state.
/// Authors omitted from a [`WritePermissions`](crate::WritePermissions) map get
/// its unlisted rule, [`Denied`](Self::Denied) by default.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
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
    Denied,
}

impl WritePermission {
    pub open spec fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unbounded, Self::Unbounded) | (Self::Denied, Self::Denied) => true,
            (Self::Bounded(a), Self::Bounded(b)) => a@ == b@,
            _ => false,
        }
    }
}

impl Clone for WritePermission {
    fn clone(&self) -> (out: Self)
        ensures out.same(self),
    {
        match self {
            Self::Unbounded => Self::Unbounded,
            Self::Denied => Self::Denied,
            Self::Bounded(heads) => Self::Bounded(heads.clone()),
        }
    }
}

impl Default for WritePermission {
    fn default() -> (out: Self)
        ensures out is Denied,
    { Self::Denied }
}

#[cfg(verus_keep_ghost)]
impl vstd::std_specs::cmp::PartialEqSpecImpl for WritePermission {
    open spec fn obeys_eq_spec() -> bool { true }
    open spec fn eq_spec(&self, other: &Self) -> bool { self.same(other) }
}
impl Eq for WritePermission {}

impl PartialEq for WritePermission {
    fn eq(&self, other: &Self) -> (out: bool)
        ensures out == self.same(other),
    {
        match (self, other) {
            (Self::Unbounded, Self::Unbounded) | (Self::Denied, Self::Denied) => true,
            (Self::Bounded(a), Self::Bounded(b)) => same_heads(a.as_slice(), b.as_slice()),
            _ => false,
        }
    }
}

} // verus!
