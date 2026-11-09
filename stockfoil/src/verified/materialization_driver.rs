//! Public materialization state, value operations and the production history-binding guard.
use super::{
    api_error::Error,
    batch_protocol::Protocol,
    checkpoint::{Checkpoint, Follow},
    commit_hash::CommitHash,
    history::History,
    materialization_events::Counts,
    materialization_feedback::Pending,
    materialization_source::Source,
    write_permissions::WritePermissions,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Incremental view planner that brings an application state in sync with a
/// [`Ledger`](crate::Ledger)'s history and [`WritePermissions`](crate::WritePermissions).
///
/// A `Materialization` owns its [`Follow`](crate::Follow) rule, permanent payload rejections,
/// and durable [`Checkpoint`](crate::Checkpoint), without owning the application state itself or
/// performing I/O.
///
/// - **In-memory states** implement [`Materializer`](crate::Materializer) and drive updates via
///   [`Replica`](crate::Replica) or [`Materialization::update`] /
///   [`Materialization::step`].
/// - **Out-of-core / async states** call [`plan`](Self::plan) to get a
///   [`Batch`](crate::Batch) of [`Event<PayloadId>`](crate::Event)s, apply them against their own storage,
///   and call [`complete`](Self::complete) until it returns `Ok(None)`, then
///   persist their state and [`checkpoint`](Self::checkpoint) atomically.
///
/// Every commit entering the graph is delivered via [`Event::Deliver`](crate::Event::Deliver), even if
/// `active` is `false` (disallowed by [`WritePermissions`](crate::WritePermissions)
/// or outside a pinned view), because later active commits may reference its
/// structural metadata. When a permission change or branch switch alters
/// whether an already-delivered commit participates, the planner emits
/// [`Event::Activate`](crate::Event::Activate) and [`Event::Retract`](crate::Event::Retract)—or [`Event::Reset`](crate::Event::Reset) followed by a
/// clean replay if `supports_visibility_changes` is `false`.
pub struct Materialization {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) supports_visibility_changes: bool,
    pub(crate) source: Option<Source>,
    pub(crate) protocol: Protocol<Pending>,
    pub(crate) owner: Option<u64>,
    pub(crate) dirty: bool,
    pub(crate) poisoned: bool,
    pub(crate) last: Counts,
}

pub(crate) open spec fn heads_known(history: &History, heads: Seq<CommitHash>) -> bool {
    forall|i: int| 0 <= i < heads.len() ==> history.known(heads[i])
}
pub(crate) open spec fn binding_references(checkpoint: &Checkpoint) -> Set<CommitHash> {
    checkpoint.cursor_view().delivery_view().to_set()
        .union(checkpoint.cursor_view().target_view().heads_view().to_set())
        .union(match checkpoint.follow_view() { Follow::Pinned(heads) => heads@.to_set(), _ => Set::empty() })
        .union(checkpoint.rejected_view().dom())
}
pub(crate) open spec fn binding_known(history: &History, checkpoint: &Checkpoint) -> bool {
    heads_known(history, checkpoint.cursor_view().delivery_view())
        && heads_known(history, checkpoint.cursor_view().target_view().heads_view())
        && (match checkpoint.follow_view() { Follow::Pinned(heads) => heads_known(history, heads@), _ => true })
        && (forall|h: CommitHash| checkpoint.rejected_view().contains_key(h) ==> history.known(h))
}

/// Partial-progress heads/counts are validated later by Source::new, not by binding.
fn validate_binding(history: &History, checkpoint: &Checkpoint, permissions: &WritePermissions) -> (out: Result<(), Error>)
    ensures match out {
        Ok(()) => binding_known(history, checkpoint),
        Err(e) => e is UnknownCommit && !binding_known(history, checkpoint) && !history.known(e->UnknownCommit_0)
            && binding_references(checkpoint).contains(e->UnknownCommit_0),
    },
{
    proof { checkpoint.representation(); history.representation(); super::stdlib::commit_order_model(); }
    history.ancestry(checkpoint.cursor.delivery_heads(), permissions)?;
    history.ancestry(checkpoint.cursor.view_point().heads(), permissions)?;
    if let Follow::Pinned(heads) = checkpoint.follow() { history.ancestry(heads, permissions)?; }
    for (hash, _) in iter: checkpoint.rejected.iter()
        invariant history.read_state() == history.topology.read_state(),
            heads_known(history, checkpoint.cursor_view().delivery_view()),
            heads_known(history, checkpoint.cursor_view().target_view().heads_view()),
            (match checkpoint.follow_view() { Follow::Pinned(heads) => heads_known(history, heads@), _ => true }),
            forall|j: int| 0 <= j < iter.seq().len() ==> checkpoint.rejected_view().contains_key(*iter.seq()[j].0),
            forall|hash: CommitHash| checkpoint.rejected_view().contains_key(hash) ==> iter.seq().contains((&hash, &checkpoint.rejected_view()[hash])),
            forall|j: int| 0 <= j < iter.index() ==> history.known(*iter.seq()[j].0),
    {
        if !history.topology.by_hash().contains_key(hash) {
            assert(checkpoint.rejected_view().contains_key(*hash));
            assert(!history.known(*hash));
            assert(!binding_known(history, checkpoint));
            return Err(Error::UnknownCommit(*hash));
        }
    }
    Ok(())
}

impl Materialization {
    pub closed spec fn poisoned_view(&self) -> bool { self.poisoned }
    pub closed spec fn owner_view(&self) -> Option<u64> { self.owner }
    /// Inductive source/owner/pending relationship established by constructors
    /// and preserved by every public driver mutation on normal return.
    pub closed spec fn source_consistent(&self) -> bool {
        (self.source is Some ==> self.source.unwrap().ready()
            && self.owner == Some(self.source.unwrap().history_id()))
        && (self.protocol.pending_value() is Some ==> self.source is Some)
    }
    pub(crate) proof fn source_consistency_definition()
        ensures forall|driver:Self| #[trigger] driver.poisoned_view() == driver.poisoned,
            forall|driver:Self| #[trigger] driver.owner_view() == driver.owner,
            forall|driver:Self| #[trigger] driver.source_consistent() == (
            (driver.source is Some ==> driver.source.unwrap().ready()
                && driver.owner == Some(driver.source.unwrap().history_id()))
            && (driver.protocol.pending_value() is Some ==> driver.source is Some)),
    {}
    pub closed spec fn checkpoint_view(&self) -> Checkpoint { self.checkpoint }
    pub closed spec fn dirty_view(&self) -> bool { self.dirty }
    pub closed spec fn supports_visibility(&self) -> bool { self.supports_visibility_changes }
    pub closed spec fn fresh(&self) -> bool {
        self.source is None && self.protocol.pending_value() is None && self.protocol.sequence() == 0
            && self.owner is None && !self.dirty && !self.poisoned
            && self.last.delivered == 0 && self.last.activated == 0 && self.last.retracted == 0
            && self.last.rejected == 0 && !self.last.rebuilt
    }
    pub closed spec fn same_control(&self, before: &Self) -> bool {
        self.source == before.source && self.protocol == before.protocol && self.owner == before.owner
            && self.dirty == before.dirty && self.poisoned == before.poisoned && self.last == before.last
            && self.supports_visibility_changes == before.supports_visibility_changes
    }
    pub(crate) proof fn representation(&self)
        ensures self.checkpoint_view() == self.checkpoint, self.dirty_view() == self.dirty,
            self.supports_visibility() == self.supports_visibility_changes,
    {}
    pub(crate) proof fn control_facts(&self, before: &Self)
        requires self.same_control(before),
        ensures self.source == before.source, self.protocol == before.protocol, self.owner == before.owner,
            self.dirty == before.dirty, self.poisoned == before.poisoned, self.last == before.last,
            self.supports_visibility_changes == before.supports_visibility_changes,
    {}
    pub(crate) proof fn initial_facts(&self)
        requires self.fresh(),
        ensures self.source is None, self.protocol.pending_value() is None, self.protocol.sequence() == 0,
            self.owner is None, !self.dirty, !self.poisoned,
    {}
    pub(crate) open spec fn same_except_owner(&self, before: &Self) -> bool {
        self.checkpoint == before.checkpoint && self.source == before.source && self.protocol == before.protocol
            && self.dirty == before.dirty && self.poisoned == before.poisoned && self.last == before.last
            && self.supports_visibility_changes == before.supports_visibility_changes
    }

    pub fn new(follow: Follow, supports_visibility_changes: bool) -> (out: Self)
        ensures out.counters_consistent(), out.checkpoint_consistent(), out.source_consistent(), out.fresh(), out.supports_visibility() == supports_visibility_changes,
            out.checkpoint_view().follow_view() == follow, out.checkpoint_view().progress_view() is None,
            out.checkpoint_view().rejected_view().is_empty(), out.checkpoint_view().cursor_view().delivery_view().len() == 0,
            out.checkpoint_view().cursor_view().target_view().heads_view().len() == 0,
            out.checkpoint_view().cursor_view().target_view().permissions_view().authors().is_empty(),
            out.checkpoint_view().cursor_view().target_view().permissions_view().fallback() is Denied,
    { Self::resume(Checkpoint::new(follow), supports_visibility_changes) }

    pub fn resume(checkpoint: Checkpoint, supports_visibility_changes: bool) -> (out: Self)
        ensures out.counters_consistent(), out.checkpoint_consistent(), out.source_consistent(), out.fresh(), out.checkpoint_view() == checkpoint, out.supports_visibility() == supports_visibility_changes,
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        Self { checkpoint, supports_visibility_changes, source: None, protocol: Protocol::new(),
            owner: None, dirty: false, poisoned: false, last: Counts::default() }
    }
    pub fn checkpoint(&self) -> (out: &Checkpoint) ensures *out == self.checkpoint_view(), { &self.checkpoint }
    pub fn follow(&self) -> (out: &Follow) ensures *out == self.checkpoint_view().follow_view(), { self.checkpoint.follow() }
    /// An interrupted traversal is finished before following the new rule.
    pub fn set_follow(&mut self, follow: Follow)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).checkpoint_view().follow_view() == follow, final(self).same_control(old(self)),
            final(self).checkpoint_view().cursor_view() == old(self).checkpoint_view().cursor_view(),
            final(self).checkpoint_view().rejected_view() == old(self).checkpoint_view().rejected_view(),
            final(self).checkpoint_view().progress_view() == old(self).checkpoint_view().progress_view(),
    { proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); self.checkpoint.representation(); } self.checkpoint.follow = follow; proof { self.checkpoint.representation(); } }
    /// Last completed view; partial states must not be labelled as that target.
    pub fn heads(&self) -> (out: &[CommitHash])
        ensures out@ == self.checkpoint_view().cursor_view().target_view().heads_view(),
    { self.checkpoint.heads() }
    pub fn is_dirty(&self) -> (out: bool) ensures out == self.dirty_view(), { self.dirty }

    pub(crate) fn check_history(&mut self, history: &History, permissions: &WritePermissions) -> (out: Result<(), Error>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).same_except_owner(old(self)), match out {
            Err(e) => *final(self) == *old(self) &&
                if old(self).poisoned { e is Poisoned }
                else if old(self).owner is Some && old(self).owner.unwrap() != history.history_id() { e is WrongHistory }
                else { e is UnknownCommit && old(self).owner is None && !binding_known(history, &old(self).checkpoint) && !history.known(e->UnknownCommit_0)
                    && binding_references(&old(self).checkpoint).contains(e->UnknownCommit_0) },
            Ok(()) => !old(self).poisoned && final(self).owner == Some(history.history_id())
                && (old(self).owner is None || old(self).owner == Some(history.history_id()))
                && (old(self).owner is None ==> binding_known(history, &old(self).checkpoint)),
        },
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        if self.poisoned { return Err(Error::Poisoned); }
        let id = history.id();
        if let Some(owner) = self.owner { if owner != id { return Err(Error::WrongHistory); } }
        if self.owner.is_none() {
            validate_binding(history, &self.checkpoint, permissions)?;
            self.owner = Some(id);
        }
        Ok(())
    }
}

} // verus!
