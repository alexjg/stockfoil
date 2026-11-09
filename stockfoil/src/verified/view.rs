//! Public view and completed-cursor values used by the checked planner.
use super::{commit_hash::CommitHash, write_permissions::WritePermissions};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// History heads and write permissions. Bounds outside the view's history grant nothing.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct ViewPoint {
    pub(crate) heads: Vec<CommitHash>,
    pub(crate) write_permissions: WritePermissions,
}

impl ViewPoint {
    pub(crate) proof fn representation(&self)
        ensures self.heads_view() == self.heads@, self.permissions_view() == self.write_permissions,
    {}

    pub closed spec fn heads_view(&self) -> Seq<CommitHash> { self.heads@ }
    pub closed spec fn permissions_view(&self) -> WritePermissions { self.write_permissions }
    pub open spec fn same(&self, other: &Self) -> bool {
        self.heads_view() == other.heads_view() && self.permissions_view().same(&other.permissions_view())
    }
    pub proof fn same_authorization(&self, other: &Self, history: &super::history::History)
        requires self.same(other),
        ensures history.authorized_from(self.heads_view(), &self.permissions_view()) == history.authorized_from(other.heads_view(), &other.permissions_view()),
    {
        assert forall|h: CommitHash| history.authorized_from(self.heads_view(), &self.permissions_view()).contains(h)
            == history.authorized_from(other.heads_view(), &other.permissions_view()).contains(h) by {
            if history.known(h) {
                let author = history.owners_view()[history.read_state().labels[history.read_state().hashes[h]@ as int] as int];
                crate::permission_refinement::same_rule(history.read_state().hashes, &self.permissions_view(), &other.permissions_view(), author);
            }
        }
        assert(history.authorized_from(self.heads_view(), &self.permissions_view()) =~= history.authorized_from(other.heads_view(), &other.permissions_view()));
    }

    pub fn from_parts(heads: Vec<CommitHash>, write_permissions: WritePermissions) -> (out: Self)
        ensures out.heads_view() == heads@, out.permissions_view() == write_permissions,
    { Self { heads, write_permissions } }
    pub fn heads(&self) -> (out: &[CommitHash]) ensures out@ == self.heads_view(), { &self.heads }
    pub fn write_permissions(&self) -> (out: &WritePermissions) ensures *out == self.permissions_view(), { &self.write_permissions }
}

impl Default for ViewPoint {
    fn default() -> (out: Self)
        ensures out.heads_view() == Seq::<CommitHash>::empty(), out.permissions_view().authors().is_empty(), out.permissions_view().fallback() is Denied,
    { Self { heads: Vec::new(), write_permissions: WritePermissions::default() } }
}

impl Clone for ViewPoint {
    fn clone(&self) -> (out: Self) ensures out.same(self),
    { Self { heads: self.heads.clone(), write_permissions: self.write_permissions.clone() } }
}

/// Completed delivery and visibility progress. Persist atomically with the materialized state.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct MaterializationCursor {
    pub(crate) delivery_heads: Vec<CommitHash>,
    pub(crate) view_point: ViewPoint,
}

impl MaterializationCursor {
    pub(crate) proof fn representation(&self)
        ensures self.delivery_view() == self.delivery_heads@, self.target_view() == self.view_point,
    {}

    pub closed spec fn delivery_view(&self) -> Seq<CommitHash> { self.delivery_heads@ }
    pub closed spec fn target_view(&self) -> ViewPoint { self.view_point }
    pub open spec fn same(&self, other: &Self) -> bool {
        self.delivery_view() == other.delivery_view() && self.target_view().same(&other.target_view())
    }
    pub fn new() -> (out: Self)
        ensures out.delivery_view().len() == 0, out.target_view().heads_view().len() == 0,
            out.target_view().permissions_view().authors().is_empty(), out.target_view().permissions_view().fallback() is Denied,
    { Self::default() }
    pub(crate) fn from_parts(delivery_heads: Vec<CommitHash>, view_point: ViewPoint) -> (out: Self)
        ensures out.delivery_view() == delivery_heads@, out.target_view() == view_point,
    { Self { delivery_heads, view_point } }
    pub fn delivery_heads(&self) -> (out: &[CommitHash]) ensures out@ == self.delivery_view(), { &self.delivery_heads }
    pub fn view_point(&self) -> (out: &ViewPoint) ensures *out == self.target_view(), { &self.view_point }
}

impl Default for MaterializationCursor {
    fn default() -> (out: Self)
        ensures out.delivery_view().len() == 0, out.target_view().heads_view().len() == 0,
            out.target_view().permissions_view().authors().is_empty(), out.target_view().permissions_view().fallback() is Denied,
    { Self { delivery_heads: Vec::new(), view_point: ViewPoint::default() } }
}

impl Clone for MaterializationCursor {
    fn clone(&self) -> (out: Self) ensures out.same(self),
    { Self { delivery_heads: self.delivery_heads.clone(), view_point: self.view_point.clone() } }
}

#[cfg(verus_keep_ghost)]
impl vstd::std_specs::cmp::PartialEqSpecImpl for ViewPoint {
    open spec fn obeys_eq_spec() -> bool { true }
    open spec fn eq_spec(&self, other: &Self) -> bool { self.same(other) }
}
impl Eq for ViewPoint {}

impl PartialEq for ViewPoint {
    fn eq(&self, other: &Self) -> (out: bool) ensures out == self.same(other),
    { super::write_permission::same_heads(&self.heads, &other.heads) && self.write_permissions == other.write_permissions }
}

#[cfg(verus_keep_ghost)]
impl vstd::std_specs::cmp::PartialEqSpecImpl for MaterializationCursor {
    open spec fn obeys_eq_spec() -> bool { true }
    open spec fn eq_spec(&self, other: &Self) -> bool { self.same(other) }
}
impl Eq for MaterializationCursor {}

impl PartialEq for MaterializationCursor {
    fn eq(&self, other: &Self) -> (out: bool) ensures out == self.same(other),
    { super::write_permission::same_heads(&self.delivery_heads, &other.delivery_heads) && self.view_point == other.view_point }
}

} // verus!
