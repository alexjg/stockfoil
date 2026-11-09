//! Durable checkpoint values and the production staging/replay/rejection transitions.
use super::{
    commit_hash::CommitHash,
    view::{MaterializationCursor, ViewPoint},
};
use std::collections::BTreeMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Which view of a ledger a materialization follows.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, PartialEq, Eq))]
pub enum Follow {
    /// Everything delivered, including unsigned local drafts: the working copy.
    Working,
    /// Authenticated history only. Drafts participate once signed.
    Signed,
    /// Fixed heads. A materialization can follow a branch by changing its pin;
    /// a local edit against it commits on these heads and moves the pin.
    Pinned(Vec<CommitHash>),
}

/// Durable progress of one materialization, to save atomically with its state.
///
/// The checkpoint contains the follow rule, the last completed cursor, rejected
/// hashes (and whether their descendants are tainted), and optionally a captured
/// traversal plus the number of records already processed. It contains no
/// payloads, payload IDs, or process-local indices: a history rebuilt from the
/// same stored items in a different arrival order can interpret it.
///
/// A partial traversal captures **both** its target view and its delivery heads.
/// The target alone is insufficient: change traversals report newly delivered
/// commits even outside that view, and new items may arrive before resuming.
/// Resume re-plans this fixed selection in canonical order and skips the saved
/// record count. The count is of source change records, not subsequent payload
/// requests or suppressed events.
///
/// All named heads must exist on resume: persist items in the sedimentree store
/// before materializing them. A checkpoint is metadata, not a copy of history;
/// it cannot make an unsigned draft durable. Parsing checks structure only;
/// the planner must validate heads and progress against the rebuilt history.
///
/// Serialized size is proportional to heads, permissions, and rejected commits.
/// Rejection reasons are not saved. Collection lengths are checked against the
/// input size before allocation, and unknown versions/tags are rejected.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, PartialEq, Eq))]
pub struct Checkpoint {
    pub(crate) follow: Follow,
    pub(crate) cursor: MaterializationCursor,
    pub(crate) rejected: BTreeMap<CommitHash, bool>,
    pub(crate) in_progress: Option<InProgress>,
}

/// A fixed traversal, independent of delivery order and later history growth.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, PartialEq, Eq))]
pub struct InProgress {
    pub target: ViewPoint,
    pub delivery_heads: Vec<CommitHash>,
    pub applied: u64,
}


impl Follow {
    pub open spec fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Working, Self::Working) | (Self::Signed, Self::Signed) => true,
            (Self::Pinned(a), Self::Pinned(b)) => a@ == b@,
            _ => false,
        }
    }
}
impl Default for Follow {
    fn default() -> (out: Self) ensures out is Working, { Self::Working }
}
impl Clone for Follow {
    fn clone(&self) -> (out: Self) ensures out.same(self),
    { match self { Self::Working => Self::Working, Self::Signed => Self::Signed, Self::Pinned(heads) => Self::Pinned(heads.clone()) } }
}

impl InProgress {
    pub open spec fn same(&self, other: &Self) -> bool {
        self.target.same(&other.target) && self.delivery_heads@ == other.delivery_heads@ && self.applied == other.applied
    }
}
impl Clone for InProgress {
    fn clone(&self) -> (out: Self) ensures out.same(self),
    { Self { target: self.target.clone(), delivery_heads: self.delivery_heads.clone(), applied: self.applied } }
}
pub open spec fn same_progress(a: Option<InProgress>, b: Option<InProgress>) -> bool {
    match (a, b) { (Some(x), Some(y)) => x.same(&y), (None, None) => true, _ => false }
}

impl Checkpoint {
    pub(crate) proof fn representation(&self)
        ensures self.follow_view() == self.follow, self.cursor_view() == self.cursor,
            self.rejected_view() == self.rejected@, self.progress_view() == self.in_progress,
    {}
    pub closed spec fn follow_view(&self) -> Follow { self.follow }
    pub closed spec fn cursor_view(&self) -> MaterializationCursor { self.cursor }
    pub closed spec fn rejected_view(&self) -> Map<CommitHash, bool> { self.rejected@ }
    pub closed spec fn progress_view(&self) -> Option<InProgress> { self.in_progress }
    pub open spec fn same(&self, other: &Self) -> bool {
        self.follow_view().same(&other.follow_view()) && self.cursor_view().same(&other.cursor_view())
            && self.rejected_view() == other.rejected_view() && same_progress(self.progress_view(), other.progress_view())
    }
    /// Empty state, following `follow`, before any delivery.
    pub fn new(follow: Follow) -> (out: Self)
        ensures out.follow_view() == follow, out.cursor_view().delivery_view().len() == 0,
            out.cursor_view().target_view().heads_view().len() == 0,
            out.cursor_view().target_view().permissions_view().authors().is_empty(),
            out.cursor_view().target_view().permissions_view().fallback() is Denied,
            out.rejected_view().is_empty(), out.progress_view() is None,
    { Self { follow, cursor: MaterializationCursor::new(), rejected: BTreeMap::new(), in_progress: None } }

    pub fn follow(&self) -> (out: &Follow) ensures *out == self.follow_view(), { &self.follow }
    /// Heads of the last **completed** view, not a claim that a partial state
    /// already reflects the entire target. Check [`is_partial`](Self::is_partial)
    /// before using these to label a checkout.
    pub fn heads(&self) -> (out: &[CommitHash]) ensures out@ == self.cursor_view().target_view().heads_view(),
    { self.cursor.view_point().heads() }
    pub fn is_partial(&self) -> (out: bool) ensures out == (self.progress_view() is Some), { self.in_progress.is_some() }

    /// Build pending progress without changing the currently published checkpoint.
    pub(crate) fn prepare_next(&self, base: &MaterializationCursor, target: &ViewPoint,
        delivery: &[CommitHash], processed: u64, done: &Option<MaterializationCursor>) -> (out: Self)
        ensures out.follow_view().same(&self.follow_view()), out.rejected_view() == self.rejected_view(),
            match done {
                Some(cursor) => out.cursor_view().same(cursor) && out.progress_view() is None,
                None => out.cursor_view().same(base) && out.progress_view() is Some
                    && out.progress_view().unwrap().target.same(target)
                    && out.progress_view().unwrap().delivery_heads@ == delivery@
                    && out.progress_view().unwrap().applied == processed,
            },
    {
        let mut next = self.clone();
        match done {
            Some(cursor) => { next.cursor = cursor.clone(); next.in_progress = None; },
            None => {
                next.cursor = base.clone();
                let mut delivery_heads = Vec::new();
                delivery_heads.extend_from_slice(delivery);
                next.in_progress = Some(InProgress { target: target.clone(), delivery_heads, applied: processed });
            },
        }
        next
    }

    /// Reset replay progress while keeping the captured frontier, follow rule and rejections.
    pub(crate) fn restart(&mut self, target: &ViewPoint, delivery: &[CommitHash])
        ensures final(self).follow_view() == old(self).follow_view(), final(self).rejected_view() == old(self).rejected_view(),
            final(self).cursor_view().delivery_view().len() == 0,
            final(self).cursor_view().target_view().heads_view().len() == 0,
            final(self).cursor_view().target_view().permissions_view().authors().is_empty(),
            final(self).cursor_view().target_view().permissions_view().fallback() is Denied,
            final(self).progress_view() is Some && final(self).progress_view().unwrap().target.same(target)
                && final(self).progress_view().unwrap().delivery_heads@ == delivery@
                && final(self).progress_view().unwrap().applied == 0,
    {
        self.cursor = MaterializationCursor::new();
        let mut delivery_heads = Vec::new();
        delivery_heads.extend_from_slice(delivery);
        self.in_progress = Some(InProgress { target: target.clone(), delivery_heads, applied: 0 });
    }

    /// Record a permanent rejection; an existing cascade can never be weakened.
    pub(crate) fn reject(&mut self, hash: CommitHash, cascade: bool) -> (new: bool)
        ensures new == !old(self).rejected_view().contains_key(hash),
            final(self).rejected_view() == old(self).rejected_view().insert(hash,
                cascade || (old(self).rejected_view().contains_key(hash) && old(self).rejected_view()[hash])),
            final(self).follow_view() == old(self).follow_view(), final(self).cursor_view() == old(self).cursor_view(),
            final(self).progress_view() == old(self).progress_view(),
    {
        proof { super::stdlib::commit_order_model(); }
        let old = self.rejected.insert(hash, cascade);
        if old == Some(true) { self.rejected.insert(hash, true); }
        old.is_none()
    }
}

impl Default for Checkpoint {
    fn default() -> (out: Self)
        ensures out.follow_view() is Working, out.cursor_view().delivery_view().len() == 0,
            out.cursor_view().target_view().heads_view().len() == 0, out.rejected_view().is_empty(), out.progress_view() is None,
    { Self::new(Follow::Working) }
}
impl Clone for Checkpoint {
    fn clone(&self) -> (out: Self) ensures out.same(self),
    {
        let in_progress = match &self.in_progress { None => None, Some(p) => Some(p.clone()) };
        Self { follow: self.follow.clone(), cursor: self.cursor.clone(), rejected: self.rejected.clone(), in_progress }
    }
}

} // verus!
