//! Inductive relationships between retained sources, pending checkpoints and counts.
use super::materialization_driver::Materialization;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use {
    super::{
        batch_protocol::EventView,
        checkpoint::Checkpoint,
        commit_hash::CommitHash,
        history::History,
        materialization_complete as complete,
        materialization_feedback::{self as feedback, Pending},
        materialization_source::Source,
    },
    vstd::prelude::*,
};
verus! {

pub(crate) open spec fn staged(source: &Source, next: &Checkpoint, restart: bool) -> bool {
    if restart {
        next.cursor_view().delivery_view().len() == 0 && next.cursor_view().target_view().heads_view().len() == 0
        && next.progress_view() is Some && next.progress_view().unwrap().applied == 0
        && next.progress_view().unwrap().target.same(&source.target())
        && next.progress_view().unwrap().delivery_heads@ == source.delivery()
    } else {
        (next.progress_view() is None) == source.finished()
        && if source.finished() {
            source.processed() == source.total() && next.cursor_view().delivery_view() == source.delivery()
                && next.cursor_view().target_view().same(&source.target())
        } else {
            next.cursor_view().same(&source.base()) && next.progress_view() is Some
                && next.progress_view().unwrap().target.same(&source.target())
                && next.progress_view().unwrap().delivery_heads@ == source.delivery()
                && next.progress_view().unwrap().applied == source.processed()
        }
    }
}

pub(crate) open spec fn pending_aligned(pending: &Pending, source: &Source, published: &Checkpoint) -> bool {
    staged(source, &pending.next, pending.restart)
        && published.rejected_view().dom().subset_of(pending.next.rejected_view().dom())
        && pending.counts.rejected == pending.next.rejected_view().dom().difference(published.rejected_view().dom()).len()
}

impl Materialization {
    pub closed spec fn checkpoint_consistent(&self) -> bool {
        self.source_consistent()
        && (self.source is Some ==> self.source.unwrap().processed() + self.source.unwrap().selected().len() == self.source.unwrap().total())
        && (self.protocol.pending_value() is Some ==>
            pending_aligned(&self.protocol.pending_value().unwrap(), &self.source.unwrap(), &self.checkpoint))
    }
    /// A completed round publishes the captured source's progress (or its reset)
    /// and counts exactly the newly persisted rejection hashes.
    pub closed spec fn completed_checkpoint_matches(&self,before:&Self) -> bool {
        before.source is Some && staged(&before.source.unwrap(),&self.checkpoint,self.source is None)
            && self.last.rejected == self.checkpoint.rejected_view().dom().difference(before.checkpoint.rejected_view().dom()).len()
    }
    pub(crate) proof fn checkpoint_consistency_definition()
        ensures forall|after:Self,before:Self| #[trigger] after.completed_checkpoint_matches(&before) == (
            before.source is Some && staged(&before.source.unwrap(),&after.checkpoint,after.source is None)
                && after.last.rejected == after.checkpoint.rejected_view().dom().difference(before.checkpoint.rejected_view().dom()).len()),
            forall|driver:Self| #[trigger] driver.checkpoint_consistent() == (
            driver.source_consistent()
            && (driver.source is Some ==> driver.source.unwrap().processed() + driver.source.unwrap().selected().len() == driver.source.unwrap().total())
            && (driver.protocol.pending_value() is Some ==>
                pending_aligned(&driver.protocol.pending_value().unwrap(), &driver.source.unwrap(), &driver.checkpoint))),
    {}
}

/// Re-reporting an existing rejection does not increment the round's cumulative count.
pub(crate) proof fn rejection_count_union(base:Set<CommitHash>, prior:Set<CommitHash>, reported:Set<CommitHash>)
    requires base.subset_of(prior),
    ensures prior.union(reported).difference(base).len() == prior.difference(base).len() + reported.difference(prior).len(),
{
    let existing = prior.difference(base);
    let added = reported.difference(prior);
    assert(prior.union(reported).difference(base) =~= existing.union(added));
    assert(existing.intersect(added) =~= Set::<CommitHash>::empty());
    vstd::set_lib::lemma_set_intersect_union_lens(existing,added);
}

pub(crate) proof fn round_preserves_alignment(before:&Pending,published:&Checkpoint,source:Source,history:&History,
    applied:&feedback::Applied,supports:bool,after:&Pending,emitted:Seq<EventView<u64>>,snapshot:&Checkpoint)
    requires pending_aligned(before,&source,published),
        complete::round(before,published,Some(source),history,applied,supports,after,emitted,snapshot),
    ensures pending_aligned(after,&source,published),
{
    rejection_count_union(published.rejected_view().dom(),before.next.rejected_view().dom(),feedback::rejection_hashes(applied.rejected@).to_set());
    assert(after.next.rejected_view().dom() =~= before.next.rejected_view().dom().union(feedback::rejection_hashes(applied.rejected@).to_set()));
    assert(published.rejected_view().dom().subset_of(after.next.rejected_view().dom()));
}

/// Active deliveries are a subset of delivered hashes for any event sequence.
pub(crate) proof fn active_inventory(events:Seq<EventView<u64>>)
    ensures super::materialization_events::active_deliveries(events).to_set().subset_of(super::materialization_events::deliveries(events)),
    decreases events.len(),
{
    if events.len() > 0 {
        active_inventory(events.drop_last());
        let active = super::materialization_events::active_deliveries(events.drop_last());
        if events.last() matches EventView::Deliver { hash,active:true,.. } {
            active.lemma_push_to_set_commute(events.last()->Deliver_hash);
        }
    }
}

/// Unique captured records yield distinct delivery hashes; each record emits at most one counted event.
pub(crate) proof fn translated_inventory(history:&History,checkpoint:&Checkpoint,records:Seq<super::change_plan::Change>,reset:bool)
    ensures {
        let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + super::materialization_events::translated(history,checkpoint,records);
        &&& super::materialization_events::deliveries(events).subset_of(super::change_plan::hashes(records).to_set())
        &&& super::change_plan::hashes(records).no_duplicates() ==>
            super::materialization_events::count(events,1) == super::materialization_events::deliveries(events).len()
        &&& super::materialization_events::count(events,1) + super::materialization_events::count(events,2)
            + super::materialization_events::count(events,3) <= records.len()
    },
    decreases records.len(),
{
    if records.len() > 0 {
        translated_inventory(history,checkpoint,records.drop_last(),reset);
        let hashes = super::change_plan::hashes(records);
        let prior_hashes = super::change_plan::hashes(records.drop_last());
        assert(hashes =~= prior_hashes.push(records.last().hash));
        prior_hashes.lemma_push_to_set_commute(records.last().hash);
        if hashes.no_duplicates() {
            assert forall|i:int,j:int| 0 <= i < j < prior_hashes.len() implies prior_hashes[i] != prior_hashes[j] by {
                assert(hashes[i] == prior_hashes[i] && hashes[j] == prior_hashes[j]);
            }
            assert(prior_hashes.no_duplicates());
            assert(!prior_hashes.contains(records.last().hash)) by {
                if prior_hashes.contains(records.last().hash) {
                    let i = choose|i:int| 0 <= i < prior_hashes.len() && prior_hashes[i] == records.last().hash;
                    assert(hashes[i] == hashes[hashes.len()-1]);
                }
            }
        }
        let prefix = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + super::materialization_events::translated(history,checkpoint,records.drop_last());
        let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + super::materialization_events::translated(history,checkpoint,records);
        let added = super::materialization_events::record_event(history,checkpoint,records.last());
        assert(added.len() <= 1);
        if added.len() == 0 { assert(events =~= prefix); }
        else {
            assert(events.drop_last() =~= prefix);
            assert(events.last() == added[0]);
            if records.last().newly_delivered && hashes.no_duplicates() {
                assert(!super::materialization_events::deliveries(prefix).contains(records.last().hash));
            }
        }
        let prior_delivered = super::materialization_events::deliveries(prefix);
        let delivered = super::materialization_events::deliveries(events);
        assert(prior_delivered.subset_of(prior_hashes.to_set()));
        if records.last().newly_delivered {
            assert(delivered =~= prior_delivered.insert(records.last().hash));
            assert(super::materialization_events::count(events,1) == super::materialization_events::count(prefix,1) + 1);
            if hashes.no_duplicates() {
                prior_hashes.to_set_ensures();
                assert(!prior_delivered.contains(records.last().hash));
                assert(delivered.len() == prior_delivered.len() + 1);
            }
        } else {
            assert(delivered =~= prior_delivered);
            assert(super::materialization_events::count(events,1) == super::materialization_events::count(prefix,1));
        }
        assert(delivered.subset_of(hashes.to_set()));
        assert(hashes.no_duplicates() ==> super::materialization_events::count(events,1) == delivered.len());
    } else {
        assert(records =~= Seq::empty());
        assert(super::change_plan::hashes(records) =~= Seq::<CommitHash>::empty());
        let empty_events:Seq<EventView<u64>> = if reset { seq![EventView::Reset] } else { Seq::empty() };
        assert(super::materialization_events::translated(history,checkpoint,records) =~= Seq::empty());
        reveal_with_fuel(super::materialization_events::deliveries,2);
        reveal_with_fuel(super::materialization_events::count,2);
        assert(super::materialization_events::deliveries(empty_events) =~= Set::<CommitHash>::empty());
        assert(super::materialization_events::count(empty_events,1) == 0);
    }
    let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + super::materialization_events::translated(history,checkpoint,records);
    assert(super::materialization_events::deliveries(events).subset_of(super::change_plan::hashes(records).to_set()));
    assert(super::change_plan::hashes(records).no_duplicates() ==> super::materialization_events::count(events,1) == super::materialization_events::deliveries(events).len());
    assert(super::materialization_events::count(events,1) + super::materialization_events::count(events,2) + super::materialization_events::count(events,3) <= records.len());
}

pub(crate) open spec fn counted(pending:&Pending,source:&Source) -> bool {
    &&& pending.original_active@.no_duplicates()
    &&& pending.original_active@.to_set().subset_of(pending.delivered@)
    &&& pending.corrected@.subset_of(pending.original_active@.to_set())
    &&& pending.active@ == if pending.restart { Seq::empty() } else { pending.original_active@ }
    &&& pending.origin@.rejected == 0
    &&& pending.origin@.delivered == pending.delivered@.len()
    &&& pending.origin@.delivered + pending.origin@.activated + pending.origin@.retracted <= source.processed()
    &&& pending.counts.delivered == pending.origin@.delivered
    &&& pending.counts.activated == pending.origin@.activated
    &&& pending.counts.retracted == pending.origin@.retracted + pending.corrected@.len()
    &&& pending.counts.rebuilt == (pending.origin@.rebuilt || pending.restart)
}

impl Materialization {
    pub closed spec fn counters_consistent(&self) -> bool {
        self.checkpoint_consistent() && (self.protocol.pending_value() is Some ==>
            counted(&self.protocol.pending_value().unwrap(),&self.source.unwrap()))
    }
    pub closed spec fn completed_counters_match(&self,before:&Self) -> bool {
        let original = before.protocol.pending_value().unwrap();
        &&& self.last.delivered == original.origin@.delivered
        &&& self.last.activated == original.origin@.activated
        &&& original.origin@.retracted <= self.last.retracted <= original.origin@.retracted + original.original_active@.len()
        &&& self.last.rebuilt == (original.origin@.rebuilt || self.source is None)
        &&& self.last.delivered + self.last.activated + self.last.retracted <= before.source.unwrap().processed() + original.original_active@.len()
    }
    pub(crate) proof fn counter_consistency_definition()
        ensures forall|after:Self,before:Self| #[trigger] after.completed_counters_match(&before) == (
            after.last.delivered == before.protocol.pending_value().unwrap().origin@.delivered
            && after.last.activated == before.protocol.pending_value().unwrap().origin@.activated
            && before.protocol.pending_value().unwrap().origin@.retracted <= after.last.retracted <= before.protocol.pending_value().unwrap().origin@.retracted + before.protocol.pending_value().unwrap().original_active@.len()
            && after.last.rebuilt == (before.protocol.pending_value().unwrap().origin@.rebuilt || after.source is None)
            && after.last.delivered + after.last.activated + after.last.retracted <= before.source.unwrap().processed() + before.protocol.pending_value().unwrap().original_active@.len()),
            forall|driver:Self| #[trigger] driver.counters_consistent() == (
            driver.checkpoint_consistent() && (driver.protocol.pending_value() is Some ==>
                counted(&driver.protocol.pending_value().unwrap(),&driver.source.unwrap()))),
    {}
}

pub(crate) proof fn correction_budget(pending:&Pending,source:&Source)
    requires counted(pending,source),
    ensures pending.corrected@.len() <= pending.original_active@.len(),
        pending.original_active@.len() <= pending.delivered@.len(),
        pending.origin@.retracted <= pending.counts.retracted <= pending.origin@.retracted + pending.original_active@.len(),
        pending.counts.delivered + pending.counts.activated + pending.counts.retracted <= source.processed() + pending.original_active@.len(),
{
    pending.original_active@.unique_seq_to_set();
    vstd::set_lib::lemma_len_subset(pending.corrected@,pending.original_active@.to_set());
    vstd::set_lib::lemma_len_subset(pending.original_active@.to_set(),pending.delivered@);
}

pub(crate) proof fn round_preserves_counts(before:&Pending,published:&Checkpoint,source:Source,history:&History,
    applied:&feedback::Applied,supports:bool,after:&Pending,emitted:Seq<EventView<u64>>,snapshot:&Checkpoint)
    requires counted(before,&source),
        complete::round(before,published,Some(source),history,applied,supports,after,emitted,snapshot),
    ensures counted(after,&source), before.corrected@.subset_of(after.corrected@),
        after.counts.retracted == before.counts.retracted + after.corrected@.difference(before.corrected@).len(),
{
    let affected = super::materialization_corrections::affected(history,snapshot,before.corrected@,before.active@);
    super::materialization_corrections::affected_facts(history,snapshot,before.corrected@,before.active@);
    assert(before.active@.no_duplicates());
    assert(affected.no_duplicates());
    affected.unique_seq_to_set();
    assert(affected.to_set().subset_of(before.original_active@.to_set()));
    assert(affected.to_set().intersect(before.corrected@) =~= Set::<CommitHash>::empty());
    assert(before.corrected@.intersect(affected.to_set()) =~= Set::<CommitHash>::empty());
    vstd::set_lib::lemma_set_intersect_union_lens(before.corrected@,affected.to_set());
    assert(after.original_active@ == before.original_active@ && after.origin@ == before.origin@);
    assert(after.corrected@.subset_of(after.original_active@.to_set()));
    assert(after.active@ == if after.restart { Seq::empty() } else { after.original_active@ });
    assert(after.counts.retracted == after.origin@.retracted + after.corrected@.len());
    assert(after.counts.rebuilt == (after.origin@.rebuilt || after.restart));
    assert(before.corrected@.subset_of(after.corrected@));
    rejection_count_union(Set::empty(),before.corrected@,after.corrected@);
    assert(before.corrected@.union(after.corrected@) =~= after.corrected@);
    assert(after.corrected@.difference(Set::empty()) =~= after.corrected@);
    assert(before.corrected@.difference(Set::empty()) =~= before.corrected@);
}

}
