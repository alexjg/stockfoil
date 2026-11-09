//! Actual rejection correction selection, follow-up events and restart transition.
#[cfg(verus_keep_ghost)]
use super::batch_protocol::{EventView, event_views};
use super::{
    api_error::Error, batch_protocol::Event, checkpoint::Checkpoint, commit_hash::CommitHash,
    history::History, history_data::PayloadId, materialization_events,
    materialization_feedback::Pending, materialization_source::Source,
};
use std::collections::{BTreeSet, HashSet};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    prelude::*,
    std_specs::{cmp::OrdSpec, iter::IteratorSpec},
};

verus! {

pub(crate) open spec fn needs_correction(history: &History, checkpoint: &Checkpoint, corrected: Set<CommitHash>, hash: CommitHash) -> bool {
    !checkpoint.rejected_view().contains_key(hash) && !corrected.contains(hash)
        && materialization_events::rejected(checkpoint, history, hash)
}
pub(crate) open spec fn affected(history: &History, checkpoint: &Checkpoint, corrected: Set<CommitHash>, active: Seq<CommitHash>) -> Seq<CommitHash>
    decreases active.len(),
{
    if active.len() == 0 { Seq::empty() }
    else if needs_correction(history, checkpoint, corrected, active.last()) {
        affected(history, checkpoint, corrected, active.drop_last()).push(active.last())
    } else { affected(history, checkpoint, corrected, active.drop_last()) }
}

proof fn unique_tail<T>(items: Seq<T>)
    requires items.len() > 0, items.no_duplicates(),
    ensures items.drop_last().no_duplicates(), !items.drop_last().contains(items.last()),
{
    if items.drop_last().contains(items.last()) {
        let i = choose|i: int| 0 <= i < items.len() - 1 && items.drop_last()[i] == items.last();
        assert(items[i] == items[items.len() - 1]);
    }
}

pub(crate) proof fn affected_facts(history: &History, checkpoint: &Checkpoint, corrected: Set<CommitHash>, active: Seq<CommitHash>)
    ensures affected(history, checkpoint, corrected, active).to_set() == active.to_set().filter(|h: CommitHash| needs_correction(history, checkpoint, corrected, h)),
        active.no_duplicates() ==> affected(history, checkpoint, corrected, active).no_duplicates(),
    decreases active.len(),
{
    if active.len() > 0 {
        affected_facts(history, checkpoint, corrected, active.drop_last());
        let prior = affected(history, checkpoint, corrected, active.drop_last());
        assert(active =~= active.drop_last().push(active.last()));
        active.drop_last().lemma_push_to_set_commute(active.last());
        prior.lemma_push_to_set_commute(active.last());
        assert(prior.to_set().subset_of(active.drop_last().to_set()));
        if active.no_duplicates() {
            unique_tail(active);
            prior.to_set_ensures(); active.drop_last().to_set_ensures();
            assert(!active.drop_last().to_set().contains(active.last()));
            assert(!prior.to_set().contains(active.last()));
            assert(!prior.contains(active.last()));
        }
    }
    assert(affected(history, checkpoint, corrected, active).to_set() =~= active.to_set().filter(|h: CommitHash| needs_correction(history, checkpoint, corrected, h)));
}

pub(crate) proof fn remembered_is_quiet(history: &History, checkpoint: &Checkpoint, corrected: Set<CommitHash>, active: Seq<CommitHash>)
    ensures affected(history, checkpoint, corrected.union(affected(history, checkpoint, corrected, active).to_set()), active).len() == 0,
{
    affected_facts(history, checkpoint, corrected, active);
    let remembered = corrected.union(affected(history, checkpoint, corrected, active).to_set());
    affected_facts(history, checkpoint, remembered, active);
    let again = affected(history, checkpoint, remembered, active);
    assert(again.to_set() =~= Set::<CommitHash>::empty());
    if again.len() > 0 { assert(again.to_set().contains(again[0])); }
}

proof fn filtered_push<T>(input: Seq<T>, retained: Seq<T>, last: T, keep: bool)
    requires retained.to_set().subset_of(input.to_set()), input.no_duplicates() ==> retained.no_duplicates(),
    ensures (if keep { retained.push(last) } else { retained }).to_set().subset_of(input.push(last).to_set()),
        input.push(last).no_duplicates() ==> (if keep { retained.push(last) } else { retained }).no_duplicates(),
{
    input.lemma_push_to_set_commute(last); retained.lemma_push_to_set_commute(last);
    if input.push(last).no_duplicates() {
        unique_tail(input.push(last));
        assert(input.push(last).drop_last() =~= input);
        input.to_set_ensures(); retained.to_set_ensures();
        assert(!input.to_set().contains(last));
        assert(!retained.to_set().contains(last));
        assert(!retained.contains(last));
    }
}

/// Source emission has unique hashes; retaining only active new deliveries preserves that property.
pub(crate) proof fn translated_active_facts(history: &History, checkpoint: &Checkpoint, records: Seq<super::change_plan::Change>, reset: bool)
    ensures {
        let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + materialization_events::translated(history, checkpoint, records);
        let active = materialization_events::active_deliveries(events);
        active.to_set().subset_of(super::change_plan::hashes(records).to_set())
            && (super::change_plan::hashes(records).no_duplicates() ==> active.no_duplicates())
    },
    decreases records.len(),
{
    if records.len() > 0 {
        translated_active_facts(history, checkpoint, records.drop_last(), reset);
        let prefix = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + materialization_events::translated(history, checkpoint, records.drop_last());
        let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + materialization_events::translated(history, checkpoint, records);
        let prior = materialization_events::active_deliveries(prefix);
        let record = records.last();
        if materialization_events::record_event(history, checkpoint, record).len() > 0 {
            assert(events.drop_last() =~= prefix);
        } else { assert(events =~= prefix); }
        assert(materialization_events::active_deliveries(events) == if record.newly_delivered
            && super::events::after(record.visibility) && !materialization_events::rejected(checkpoint, history, record.hash) {
                prior.push(record.hash)
            } else { prior });
        assert(super::change_plan::hashes(records) =~= super::change_plan::hashes(records.drop_last()).push(record.hash));
        filtered_push(super::change_plan::hashes(records.drop_last()), prior, record.hash,
            record.newly_delivered && super::events::after(record.visibility)
                && !materialization_events::rejected(checkpoint, history, record.hash));
    } else {
        assert(super::change_plan::hashes(records) =~= Seq::<CommitHash>::empty());
        let events = (if reset { seq![EventView::Reset] } else { Seq::empty() }) + materialization_events::translated(history, checkpoint, records);
        assert(events =~= if reset { seq![EventView::Reset] } else { Seq::empty() });
        reveal_with_fuel(materialization_events::active_deliveries, 2);
        assert(materialization_events::active_deliveries(events) =~= Seq::<CommitHash>::empty());
    }
}

pub(crate) fn select(history: &History, checkpoint: &Checkpoint, corrected: &HashSet<CommitHash>, active: &[CommitHash]) -> (out: Vec<CommitHash>)
    ensures out@ == affected(history, checkpoint, corrected@, active@), out.len() <= active.len(),
{
    proof { checkpoint.representation(); super::stdlib::commit_key_model(); super::stdlib::commit_order_model(); }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < active.len()
        invariant i <= active.len(), out.len() <= i,
            out@ == affected(history, checkpoint, corrected@, active@.take(i as int)),
            checkpoint.rejected@ == checkpoint.rejected_view(),
            vstd::std_specs::hash::obeys_key_model::<CommitHash>(), vstd::laws_cmp::obeys_cmp::<CommitHash>(),
        decreases active.len() - i,
    {
        let hash = active[i];
        if !checkpoint.rejected.contains_key(&hash) && !corrected.contains(&hash)
            && materialization_events::is_rejected(checkpoint, history, &hash) { out.push(hash); }
        assert(active@.take(i + 1).drop_last() =~= active@.take(i as int));
        i += 1;
    }
    assert(active@.take(i as int) =~= active@);
    out
}

impl History {
    pub(crate) open spec fn correction_payload(&self, hash: CommitHash) -> PayloadId {
        self.topology.nodes_view()[self.read_state().hashes[hash]@ as int].payload
    }
    pub(crate) fn payload_for_correction(&self, hash: &CommitHash) -> (out: PayloadId)
        requires self.known(*hash),
        ensures out == self.correction_payload(*hash),
    {
        proof { self.representation(); self.topology.metadata_facts(); }
        let idx = self.topology.by_hash().get(hash).unwrap();
        self.topology.nodes()[idx.index()].payload
    }
}
pub(crate) open spec fn retractions(hashes: Seq<CommitHash>) -> Seq<EventView<PayloadId>> {
    hashes.map(|i: int, h: CommitHash| EventView::Retract { hash: h })
}
pub(crate) open spec fn payload_events(history: &History, hashes: Seq<CommitHash>) -> Seq<EventView<PayloadId>> {
    hashes.map(|i: int, h: CommitHash| EventView::Payload { hash: h, payload: history.correction_payload(h) })
}
pub(crate) open spec fn payload_hashes(events: Seq<EventView<PayloadId>>) -> Seq<CommitHash>
    decreases events.len(),
{
    if events.len() == 0 { Seq::empty() }
    else { match events.last() {
        EventView::Payload { hash, .. } => payload_hashes(events.drop_last()).push(hash),
        _ => payload_hashes(events.drop_last()),
    } }
}
proof fn payload_projection(history: &History, retract: Seq<CommitHash>, wanted: Seq<CommitHash>)
    ensures payload_hashes(retractions(retract) + payload_events(history, wanted)) == wanted,
    decreases wanted.len(), retract.len(),
{
    if wanted.len() > 0 {
        payload_projection(history, retract, wanted.drop_last());
        assert((retractions(retract) + payload_events(history, wanted)).drop_last() =~= retractions(retract) + payload_events(history, wanted.drop_last()));
    } else if retract.len() > 0 {
        payload_projection(history, retract.drop_last(), wanted);
        assert((retractions(retract) + payload_events(history, wanted)).drop_last() =~= retractions(retract.drop_last()) + payload_events(history, wanted));
    }
}

pub(crate) struct Correction {
    pub events: Vec<Event<PayloadId>>,
    pub retracted: usize,
    pub reset: bool,
}
impl Correction {
    pub(crate) open spec fn requested(&self) -> Seq<CommitHash> { payload_hashes(event_views(self.events@)) }
}

/// Select once against the old corrected set, preserving source order, then remember the emitted retractions.
pub(crate) fn plan(history: &History, checkpoint: &Checkpoint, active: &[CommitHash], corrected: &mut HashSet<CommitHash>,
    wanted: &BTreeSet<CommitHash>, supports_visibility_changes: bool) -> (out: Correction)
    requires forall|h: CommitHash| wanted@.contains(h) ==> history.known(h),
    ensures out.reset == (!supports_visibility_changes && affected(history, checkpoint, old(corrected)@, active@).len() > 0),
        if out.reset {
            event_views(out.events@) == seq![EventView::Reset] && out.retracted == 0 && final(corrected)@ == old(corrected)@
        } else {
            out.retracted == affected(history, checkpoint, old(corrected)@, active@).len()
                && final(corrected)@ == old(corrected)@.union(affected(history, checkpoint, old(corrected)@, active@).to_set())
                && out.requested().to_set() == wanted@ && out.requested().no_duplicates()
                && vstd::std_specs::btree::increasing_seq(out.requested())
                && event_views(out.events@) == retractions(affected(history, checkpoint, old(corrected)@, active@)) + payload_events(history, out.requested())
        },
{
    let affected = select(history, checkpoint, corrected, active);
    if !affected.is_empty() && !supports_visibility_changes {
        return Correction { events: vec![Event::Reset], retracted: 0, reset: true };
    }
    proof { super::stdlib::commit_key_model(); super::stdlib::commit_order_model(); }
    let ghost previous = corrected@;
    let mut events = Vec::new();
    let mut i = 0usize;
    while i < affected.len()
        invariant i <= affected.len(),
            event_views(events@) == retractions(affected@.take(i as int)),
            corrected@ == previous.union(affected@.take(i as int).to_set()),
            vstd::std_specs::hash::obeys_key_model::<CommitHash>(),
        decreases affected.len() - i,
    {
        let hash = affected[i];
        let ghost prior_events = event_views(events@);
        corrected.insert(hash);
        events.push(Event::Retract { hash });
        assert(affected@.take(i + 1) =~= affected@.take(i as int).push(hash));
        proof { affected@.take(i as int).lemma_push_to_set_commute(hash); }
        assert(event_views(events@) =~= prior_events.push(EventView::Retract { hash }));
        assert(retractions(affected@.take(i + 1)) =~= retractions(affected@.take(i as int)).push(EventView::Retract { hash }));
        i += 1;
    }
    assert(affected@.take(i as int) =~= affected@);
    proof { super::stdlib::commit_order_model(); }
    let iterator = wanted.iter();
    let ghost requests = iterator.remaining().unref();
    proof {
        broadcast use vstd::laws_cmp::lemma_ref_obeys_cmp_spec;
        assert(vstd::laws_cmp::obeys_cmp::<&CommitHash>());
        assert(vstd::std_specs::btree::increasing_seq(iterator.remaining()));
        vstd::std_specs::btree::axiom_increasing_seq_meaning::<&CommitHash>(iterator.remaining());
        assert forall|i: int, j: int| 0 <= i < j < requests.len() implies requests[i].cmp_spec(&requests[j]) is Less by {
            assert(<&CommitHash as OrdSpec>::cmp_spec(&iterator.remaining()[i], &iterator.remaining()[j]) is Less);
        }
        assert(vstd::std_specs::btree::increasing_seq(requests));
    }
    for hash in iter: iterator
        invariant event_views(events@) == retractions(affected@) + payload_events(history, iter.seq().take(iter.index()).unref()),
            iter.seq().unref() == requests, requests.to_set() == wanted@,
            requests.no_duplicates(), vstd::std_specs::btree::increasing_seq(requests),
            forall|h: CommitHash| wanted@.contains(h) ==> history.known(h),
    {
        assert(requests[iter.index()] == *hash);
        assert(wanted@.contains(*hash));
        let ghost prior_events = event_views(events@);
        let payload = history.payload_for_correction(hash);
        events.push(Event::Payload { hash: *hash, payload });
        assert(iter.seq().take(iter.index() + 1).unref() =~= iter.seq().take(iter.index()).unref().push(*hash));
        assert(event_views(events@) =~= prior_events.push(EventView::Payload { hash: *hash, payload }));
        assert(payload_events(history, iter.seq().take(iter.index() + 1).unref()) =~=
            payload_events(history, iter.seq().take(iter.index()).unref()).push(EventView::Payload { hash: *hash, payload }));
    }
    assert(event_views(events@) == retractions(affected@) + payload_events(history, requests));
    proof { payload_projection(history, affected@, requests); }
    let out = Correction { events, retracted: affected.len(), reset: false };
    assert(out.requested() == requests);
    out
}

impl Pending {
    pub(crate) fn restart_from(&mut self, source: &Source) -> (out: Result<(), Error>)
        ensures final(self).origin == old(self).origin, final(self).original_active == old(self).original_active,
            match out {
            Err(e) => e is Poisoned && !source.ready() && *final(self) == *old(self),
            Ok(()) => source.ready() && final(self).restart && final(self).active.len() == 0 && final(self).counts.rebuilt
                && final(self).next.follow_view() == old(self).next.follow_view()
                && final(self).next.rejected_view() == old(self).next.rejected_view()
                && final(self).next.cursor_view().delivery_view().len() == 0 && final(self).next.cursor_view().target_view().heads_view().len() == 0
                && final(self).next.progress_view() is Some && final(self).next.progress_view().unwrap().applied == 0
                && final(self).next.progress_view().unwrap().target.same(&source.target()) && final(self).next.progress_view().unwrap().delivery_heads@ == source.delivery()
                && final(self).delivered@ == old(self).delivered@ && final(self).corrected@ == old(self).corrected@
                && final(self).counts.delivered == old(self).counts.delivered && final(self).counts.activated == old(self).counts.activated
                && final(self).counts.retracted == old(self).counts.retracted && final(self).counts.rejected == old(self).counts.rejected,
        },
    {
        source.restart_checkpoint(&mut self.next)?;
        self.restart = true;
        self.active.clear();
        self.counts.rebuilt = true;
        Ok(())
    }
}

} // verus!
