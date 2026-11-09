//! Actual in-memory orchestration; callback contracts come from the methods invoked.
use super::{
    api_error::Error,
    batch_protocol::{Batch, Event},
    checkpoint::Checkpoint,
    document::Document,
    drive_error::DriveError,
    history::History,
    materialization_driver::Materialization,
    materialization_events::Counts,
    materialization_feedback::Applied,
    materializer::{Materializer, Step},
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// A call to the *actual* method, including any stronger checked implementation contract.
#[verifier::prophetic]
#[verifier::deprecated_postcondition_mut_ref_style(false)]
pub open spec fn apply_effect<M: Materializer>(before: M, after: M, history: &History,
    events: Vec<Event<Arc<[u8]>>>, out: Result<Applied, M::Error>) -> bool {
    exists|r: &mut M| *r == before && *final(r) == after
        && call_ensures(M::apply, (r, history, events), out)
}

#[verifier::prophetic]
#[verifier::deprecated_postcondition_mut_ref_style(false)]
pub open spec fn checkpoint_effect<M: Materializer>(before: M, after: M,
    checkpoint: &Checkpoint, out: Result<(), M::Error>) -> bool {
    exists|r: &mut M| *r == before && *final(r) == after
        && call_ensures(M::checkpoint, (r, checkpoint), out)
}

fn apply<M: Materializer>(state: &mut M, history: &History, events: Vec<Event<Arc<[u8]>>>) -> (out: Result<Applied, M::Error>)
    ensures apply_effect(*old(state), *final(state), history, events, out),
{
    let call = M::apply;
    call(state, history, events)
}

fn checkpoint<M: Materializer>(state: &mut M, value: &Checkpoint) -> (out: Result<(), M::Error>)
    ensures checkpoint_effect(*old(state), *final(state), value, out),
{
    let call = M::checkpoint;
    call(state, value)
}


pub(crate) open spec fn pending_matches(driver: &Materialization, batch: Option<Batch>) -> bool {
    match batch { Some(b) => driver.protocol.accepts(&b), None => driver.protocol.pending_value() is None }
}

pub(crate) open spec fn resolved<C,O,Q>(document: &Document<C,O,Q>, batch: &Batch, events: Vec<Event<Arc<[u8]>>>) -> bool {
    document.resolvable(batch.events_view()) && events.len() == batch.events_view().len()
        && forall|i:int| 0 <= i < events.len() ==> document.resolves(batch.events_view()[i], #[trigger] events[i].view())
}


pub(crate) open spec fn activation_set(batch:Option<Batch>) -> Set<super::commit_hash::CommitHash> {
    match batch {
        None => Set::empty(),
        Some(b) => b.events_view().filter(|e:super::batch_protocol::EventView<u64>| super::materialization_events::activation(e) is Some)
            .map(|i:int,e:super::batch_protocol::EventView<u64>| super::materialization_events::activation(e).unwrap()).to_set(),
    }
}
pub(crate) open spec fn allowed_batch(batch:Option<Batch>, allowed:Set<super::commit_hash::CommitHash>) -> bool {
    match batch { None => true, Some(b) => forall|i:int| 0 <= i < b.events_view().len()
        && super::materialization_events::activation(b.events_view()[i]) is Some ==>
        allowed.contains(super::materialization_events::activation(#[trigger] b.events_view()[i]).unwrap()) }
}
pub(crate) open spec fn allowed_events(events:Vec<Event<Arc<[u8]>>>, allowed:Set<super::commit_hash::CommitHash>) -> bool {
    forall|i:int| 0 <= i < events.len() && super::materialization_events::activation(events[i].view()) is Some ==>
        allowed.contains(super::materialization_events::activation(#[trigger] events[i].view()).unwrap())
}

proof fn initial_allowed(batch:Option<Batch>)
    ensures allowed_batch(batch,activation_set(batch)),
{
    if let Some(b) = batch {
        let pred = |e:super::batch_protocol::EventView<u64>| super::materialization_events::activation(e) is Some;
        let filtered = b.events_view().filter(pred);
        let mapped = filtered.map(|i:int,e:super::batch_protocol::EventView<u64>| super::materialization_events::activation(e).unwrap());
        assert forall|i:int| 0 <= i < b.events_view().len() && super::materialization_events::activation(b.events_view()[i]) is Some implies
            activation_set(batch).contains(super::materialization_events::activation(#[trigger] b.events_view()[i]).unwrap()) by {
            b.events_view().lemma_filter_contains(pred,i);
            let j = choose|j:int| 0 <= j < filtered.len() && filtered[j] == b.events_view()[i];
            assert(mapped[j] == super::materialization_events::activation(b.events_view()[i]).unwrap());
            assert(mapped.contains(mapped[j]));
        }
    }
}

/// Each edge is an actual successful apply followed by actual successful completion.
#[verifier::prophetic]
pub(crate) open spec fn rounds<C,M:Materializer,O,Q>(document: &Document<C,O,Q>,
    first: Materialization, initial: M, batch: Option<Batch>,
    last: Materialization, state: M, next: Option<Batch>, count: nat) -> bool
    decreases count,
{
    if count == 0 { first == last && initial == state && batch == next }
    else {
        exists|previous: Materialization, prior: M, current: Batch, events: Vec<Event<Arc<[u8]>>>, report: Applied|
            rounds(document, first, initial, batch, previous, prior, Some(current), (count - 1) as nat)
            && resolved(document, &current, events) && allowed_events(events, activation_set(batch))
            && apply_effect(prior, state, &document.ledger_view().history_view(), events, Ok(report))
            && last.completion_outcome(&previous, &document.ledger_view(), &current, &report, Ok(next))
            && last.completion_errors(&previous, &document.ledger_view(), &current, &report, Ok(next))
    }
}

#[verifier::prophetic]
pub(crate) open spec fn tail_error<C,M:Materializer,O,Q>(document: &Document<C,O,Q>,
    before: Materialization, prior: M, current: Batch, after: Materialization, state: M, error: DriveError<M::Error>, allowed:Set<super::commit_hash::CommitHash>) -> bool {
    match error {
        DriveError::Stockfoil(e) =>
            (after == before && state == prior && !document.resolvable(current.events_view()) && e is Storage)
            || exists|events: Vec<Event<Arc<[u8]>>>, report: Applied|
                resolved(document, &current, events) && allowed_events(events,allowed)
                && apply_effect(prior, state, &document.ledger_view().history_view(), events, Ok(report))
                && after.completion_outcome(&before, &document.ledger_view(), &current, &report, Err(e))
                && after.completion_errors(&before, &document.ledger_view(), &current, &report, Err(e)),
        DriveError::Apply(e) => after == before && exists|events: Vec<Event<Arc<[u8]>>>|
            resolved(document, &current, events) && allowed_events(events,allowed)
            && apply_effect(prior, state, &document.ledger_view().history_view(), events, Err(e)),
        _ => false,
    }
}

#[verifier::prophetic]
pub(crate) open spec fn batches_outcome<C,M:Materializer,O,Q>(document: &Document<C,O,Q>,
    before: Materialization, prior: M, batch: Option<Batch>, after: Materialization, state: M, out: Result<(), DriveError<M::Error>>) -> bool {
    exists|count:nat, tail:Materialization, last:M, next:Option<Batch>|
        rounds(document, before, prior, batch, tail, last, next, count)
        && match out {
            Ok(()) => next is None && after == tail && state == last,
            Err(e) => next is Some && tail_error(document, tail, last, next.unwrap(), after, state, e, activation_set(batch)),
        }
}

impl Materialization {
    /// Every follow-up consumes a fresh ticket; exhaustion returns an error.
    fn drive_batches<C,M:Materializer,O,Q>(&mut self, document:&Document<C,O,Q>, state:&mut M, first_batch_input:Option<Batch>)
        -> (out:Result<(),DriveError<M::Error>>)
        requires pending_matches(old(self), first_batch_input),
        ensures final(self).protocol.pending_value() is Some ==> final(self).checkpoint == old(self).checkpoint,
            old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            batches_outcome(document, *old(self), *old(state), first_batch_input, *final(self), *final(state), out),
            final(self).dirty == old(self).dirty, final(self).poisoned == old(self).poisoned,
            final(self).supports_visibility_changes == old(self).supports_visibility_changes,
            final(self).protocol.planner_id() == old(self).protocol.planner_id(),
            out is Ok ==> final(self).protocol.pending_value() is None && final(self).protocol.sequence() >= old(self).protocol.sequence(),
            first_batch_input is None ==> *final(self) == *old(self) && *final(state) == *old(state) && out is Ok,
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost first = *self; let ghost initial = *state; let ghost first_batch = first_batch_input;
        let mut batch = first_batch_input;
        let ghost mut count:nat = 0;
        proof { initial_allowed(batch); }
        loop
            invariant first == *old(self), initial == *old(state), first_batch == first_batch_input,
                first.source_consistent() ==> self.source_consistent(),
                first.checkpoint_consistent() ==> self.checkpoint_consistent(),
                first.counters_consistent() ==> self.counters_consistent(),
                self.protocol.pending_value() is Some ==> self.checkpoint == first.checkpoint,
                pending_matches(self, batch), allowed_batch(batch, activation_set(first_batch)),
                self.dirty == first.dirty, self.poisoned == first.poisoned,
                self.supports_visibility_changes == first.supports_visibility_changes,
                self.protocol.planner_id() == first.protocol.planner_id(), self.protocol.sequence() >= first.protocol.sequence(),
                first_batch is None ==> batch is None && *self == first && *state == initial,
                rounds(document, first, initial, first_batch, *self, *state, batch, count),
            decreases u64::MAX - self.protocol.sequence(), if batch is Some { 1int } else { 0int },
        {
            let current = match batch { None => { return Ok(()); }, Some(current) => current };
            let ghost previous = *self; let ghost prior = *state;
            let events = match document.resolve_batch(&current) {
                Ok(events) => events,
                Err(e) => { return Err(DriveError::Stockfoil(e)); },
            };
            let ghost sent = events;
            assert forall|i:int| 0 <= i < events.len() implies
                super::materialization_events::activation(#[trigger] events[i].view()) ==
                    super::materialization_events::activation(current.events_view()[i]) by {}
            assert(allowed_events(events, activation_set(first_batch)));
            let report = match apply(state, document.history(), events) {
                Ok(report) => report,
                Err(e) => {
                    assert(resolved(document, &current, sent));
                    assert(apply_effect(prior, *state, &document.ledger_view().history_view(), sent, Err(e)));
                    assert(tail_error(document, previous, prior, current, *self, *state, DriveError::Apply(e), activation_set(first_batch)));
                    return Err(DriveError::Apply(e));
                },
            };
            let ghost applied = report;
            let next = self.complete(document.ledger(), current, report);
            proof { self.completion_facts(&previous, &document.ledger_view(), &current, &applied, next); }
            match next {
                Err(e) => { return Err(DriveError::Stockfoil(e)); },
                Ok(next) => { batch = next; },
            }
            proof {
                assert(rounds(document, first, initial, first_batch, previous, prior, Some(current), count));
                assert(resolved(document, &current, sent));
                assert(apply_effect(prior, *state, &document.ledger_view().history_view(), sent, Ok(applied)));
                assert(rounds(document, first, initial, first_batch, *self, *state, batch, count + 1));
                count = count + 1;
            }
        }
    }
}


impl Materialization {
    pub(crate) open spec fn same_except_dirty(&self, before: &Self) -> bool {
        self.checkpoint == before.checkpoint && self.source == before.source && self.protocol == before.protocol
            && self.owner == before.owner && self.poisoned == before.poisoned && self.last == before.last
            && self.supports_visibility_changes == before.supports_visibility_changes
    }
}

#[verifier::prophetic]
pub(crate) open spec fn finish_outcome<C,M:Materializer,O,Q>(document:&Document<C,O,Q>, before:Materialization,
    initial:M, did_work:bool, after:Materialization, state:M, out:Result<Step,DriveError<M::Error>>) -> bool {
    &&& (out matches Ok(step) ==> (step is Current) == after.current(&document.ledger_view()))
    &&& if did_work || before.dirty {
        match out {
            Ok(_) => !after.dirty && after.same_except_dirty(&before)
                && checkpoint_effect(initial, state, &before.checkpoint, Ok(())),
            Err(DriveError::Checkpoint(e)) => after == before && checkpoint_effect(initial, state, &before.checkpoint, Err(e)),
            _ => false,
        }
    } else { after == before && state == initial && out is Ok }
}

#[verifier::prophetic]
pub(crate) open spec fn body_outcome<C,M:Materializer,O,Q>(document:&Document<C,O,Q>, before:Materialization,
    initial:M, budget:usize, after:Materialization, state:M, out:Result<Step,DriveError<M::Error>>) -> bool {
    exists|planned:Materialization, plan:Result<Option<Batch>,Error>|
        planned.plan_outcome(&before, &document.ledger_view(), budget, plan)
        && planned.plan_errors(&before, &document.ledger_view(), budget, plan)
        && match plan {
            Err(e) => after == planned && state == initial && out == Err(DriveError::Stockfoil(e)),
            Ok(batch) => exists|completed:Materialization, applied:M, result:Result<(),DriveError<M::Error>>|
                batches_outcome(document, planned, initial, batch, completed, applied, result)
                && match result {
                    Err(e) => after == completed && state == applied && out == Err(e),
                    Ok(()) => finish_outcome(document, completed, applied, batch is Some, after, state, out),
                },
        }
}

impl Materialization {
    fn drive_step<C,M:Materializer,O,Q>(&mut self, document:&Document<C,O,Q>, state:&mut M, budget:usize)
        -> (out:Result<Step,DriveError<M::Error>>)
        requires !old(self).poisoned, old(self).protocol.pending_value() is None,
        ensures final(self).protocol.pending_value() is Some ==> final(self).checkpoint == old(self).checkpoint,
            old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            body_outcome(document, *old(self), *old(state), budget, *final(self), *final(state), out),
            !final(self).poisoned, final(self).supports_visibility_changes == old(self).supports_visibility_changes,
            final(self).protocol.planner_id() == old(self).protocol.planner_id(),
            match out {
                Ok(step) => !final(self).dirty && final(self).protocol.pending_value() is None
                    && ((step is Current) == final(self).current(&document.ledger_view()))
                    && final(self).protocol.sequence() >= old(self).protocol.sequence()
                    && (step is More ==> final(self).protocol.sequence() > old(self).protocol.sequence()),
                Err(_) => final(self).dirty == old(self).dirty,
            },
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self; let ghost initial = *state;
        let plan = self.plan(document.ledger(), budget);
        proof { self.plan_facts(&before, &document.ledger_view(), budget, plan); }
        let ghost planned = *self;
        let batch = match plan { Err(e) => return Err(DriveError::Stockfoil(e)), Ok(batch) => batch };
        let did_work = batch.is_some();
        proof { self.current_facts(&document.ledger_view()); }
        let batches = self.drive_batches(document, state, batch);
        let ghost completed = *self; let ghost applied = *state;
        match batches { Err(e) => return Err(e), Ok(()) => {} }
        if did_work || self.dirty {
            assert(*state == applied && self.checkpoint == completed.checkpoint);
            let persisted = checkpoint(state, &self.checkpoint);
            assert(checkpoint_effect(applied, *state, &completed.checkpoint, persisted));
            match persisted {
                Err(e) => { return Err(DriveError::Checkpoint(e)); },
                Ok(()) => {},
            }
            assert(persisted is Ok);
            assert(persisted->Ok_0 == ());
            assert(persisted == Ok(()));
            assert(checkpoint_effect(applied, *state, &completed.checkpoint, Ok(())));
            self.dirty = false;
        }
        proof { self.current_facts(&document.ledger_view()); planned.current_facts(&document.ledger_view()); }
        let result = if self.is_current(document.ledger()) { Step::Current } else { Step::More };
        assert(self.same_except_dirty(&completed));
        assert((result is Current) == self.current(&document.ledger_view()));
        assert(did_work || completed.dirty ==> checkpoint_effect(applied, *state, &completed.checkpoint, Ok(())));
        assert(finish_outcome(document, completed, applied, did_work, *self, *state, Ok(result)));
        Ok(result)
    }

    pub(crate) open spec fn prepared_from<C,O,Q>(&self, before:&Self, document:&Document<C,O,Q>) -> bool {
        self.checkpoint == before.checkpoint && self.source == before.source && self.protocol == before.protocol
            && self.owner == Some(document.ledger_view().history_view().history_id())
            && self.dirty == before.dirty && !self.poisoned && self.supports_visibility_changes == before.supports_visibility_changes
            && self.last.delivered == 0 && self.last.activated == 0 && self.last.retracted == 0 && self.last.rejected == 0 && !self.last.rebuilt
    }
    pub(crate) open spec fn rolled_back(&self, failed:&Self, saved:&Checkpoint) -> bool {
        self.checkpoint.same(saved) && self.source == failed.source && self.protocol == failed.protocol
            && self.owner == failed.owner && self.dirty == failed.dirty && self.last == failed.last
            && self.supports_visibility_changes == failed.supports_visibility_changes && self.poisoned
    }
    pub(crate) open spec fn preflight<C,O,Q>(&self, document:&Document<C,O,Q>, budget:usize) -> bool {
        self.guard_valid(&document.ledger_view().history_view()) && budget > 0 && self.protocol.pending_value() is None
    }
    #[verifier::prophetic]
    pub closed spec fn step_outcome<C,M:Materializer,O,Q>(&self, before:&Self, document:&Document<C,O,Q>, initial:M,
        state:M, budget:usize, out:Result<Step,DriveError<M::Error>>) -> bool {
        self.step_details(before, document, initial, state, budget, out)
    }
    #[verifier::prophetic]
    pub(crate) open spec fn step_details<C,M:Materializer,O,Q>(&self, before:&Self, document:&Document<C,O,Q>, initial:M,
        state:M, budget:usize, out:Result<Step,DriveError<M::Error>>) -> bool {
        let history = document.ledger_view().history_view();
        if !before.guard_valid(&history) {
            state == initial && *self == *before && (out matches Err(DriveError::Stockfoil(e)) && before.guard_error(&history,e))
        } else if budget == 0 || before.protocol.pending_value() is Some {
            state == initial && self.same_except_owner(before) && self.owner == Some(history.history_id())
                && if budget == 0 { out == Err(DriveError::Stockfoil(Error::Limit("materialization budget must be positive"))) }
                   else { out == Err(DriveError::Stockfoil(Error::Protocol("complete the outstanding batch before driving state"))) }
        } else {
            exists|prepared:Materialization, ended:Materialization|
                prepared.prepared_from(before, document)
                && body_outcome(document, prepared, initial, budget, ended, state, out)
                && match out { Ok(_) => *self == ended, Err(_) => self.rolled_back(&ended, &before.checkpoint) }
        }
    }
    pub(crate) open spec fn step_status_details<C,E,O,Q>(&self, before:&Self, document:&Document<C,O,Q>, budget:usize, out:Result<Step,DriveError<E>>) -> bool {
        match out {
                Ok(step) => !self.poisoned && !self.dirty && self.protocol.pending_value() is None
                    && ((step is Current) == self.current(&document.ledger_view()))
                    && self.protocol.sequence() >= before.protocol.sequence()
                    && (step is More ==> self.protocol.sequence() > before.protocol.sequence()),
                Err(_) => self.checkpoint.same(&before.checkpoint)
                    && (before.preflight(document,budget) ==> self.poisoned),
            }
    }
    pub closed spec fn step_status<C,E,O,Q>(&self, before:&Self, document:&Document<C,O,Q>, budget:usize, out:Result<Step,DriveError<E>>) -> bool {
        self.step_status_details(before,document,budget,out)
    }
    pub(crate) proof fn step_facts<C,M:Materializer,O,Q>(&self, before:&Self, document:&Document<C,O,Q>, initial:M,
        state:M, budget:usize, out:Result<Step,DriveError<M::Error>>)
        requires self.step_outcome(before, document, initial, state, budget, out), self.step_status(before,document,budget,out),
        ensures self.step_details(before, document, initial, state, budget, out), self.step_status_details(before,document,budget,out),
    {}

    /// In-memory convenience: apply one planned source batch and all its
    /// follow-ups, then checkpoint the state. On a returned error after preflight the planner is
    /// poisoned; restore state and checkpoint together before continuing.
    /// [`DriveError::Apply`] and [`DriveError::Checkpoint`] preserve the
    /// materializer's error value and distinguish the failing phase.
    pub fn step<C, M: Materializer>(&mut self, document: &Document<C, impl Sized, impl Sized>,
        state:&mut M, budget:usize) -> (out:Result<Step,DriveError<M::Error>>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).step_outcome(old(self), document, *old(state), *final(state), budget, out),
            final(self).step_status(old(self),document,budget,out),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self; let ghost initial = *state;
        self.check_history(document.history(), document.ledger().permissions())?;
        if budget == 0 { return Err(Error::Limit("materialization budget must be positive").into()); }
        if self.protocol.has_pending() {
            return Err(Error::Protocol("complete the outstanding batch before driving state").into());
        }
        let saved = self.checkpoint.clone();
        self.last = Counts::default();
        let ghost prepared = *self;
        let result = self.drive_step(document,state,budget);
        let ghost ended = *self;
        if result.is_err() {
            self.checkpoint = saved;
            self.poisoned = true;
        }
        assert(prepared.prepared_from(&before, document));
        assert(body_outcome(document, prepared, initial, budget, ended, *state, result));
        result
    }
}

} // verus!
