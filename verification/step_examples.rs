//! Checked callers use the actual public step and actual callback method contracts.
use vstd::prelude::*;
use std::sync::Arc;
use crate::{api_error::Error, batch_protocol::Event, checkpoint::Checkpoint,
    document::Document, drive_error::DriveError, history::History,
    materialization_driver::Materialization, materialization_feedback::Applied,
    materialization_step as step, materializer::{Materializer,Step}};

verus! {

pub struct Recording {
    pub applied: bool,
    pub saved: Option<Checkpoint>,
    pub fail_apply: bool,
    pub fail_checkpoint: bool,
}
impl Materializer for Recording {
    type Error = u8;
    fn apply(&mut self, _: &History, _:Vec<Event<Arc<[u8]>>>) -> (out:Result<Applied,u8>)
        ensures final(self).applied, final(self).saved == old(self).saved,
            final(self).fail_apply == old(self).fail_apply, final(self).fail_checkpoint == old(self).fail_checkpoint,
            if old(self).fail_apply { out == Err(11) }
            else { out is Ok && out->Ok_0.rejected@.len() == 0 && out->Ok_0.wanted@.len() == 0 },
    {
        self.applied = true;
        if self.fail_apply { Err(11) } else { Ok(Applied::default()) }
    }
    fn checkpoint(&mut self, value:&Checkpoint) -> (out:Result<(),u8>)
        ensures final(self).applied == old(self).applied,
            final(self).saved is Some && final(self).saved.unwrap().same(value),
            final(self).fail_apply == old(self).fail_apply, final(self).fail_checkpoint == old(self).fail_checkpoint,
            out == if old(self).fail_checkpoint { Err(22) } else { Ok(()) },
    {
        self.saved = Some(value.clone());
        if self.fail_checkpoint { Err(22) } else { Ok(()) }
    }
}

pub(crate) fn public_step<C,M:Materializer,O,Q>(driver:&mut Materialization, document:&Document<C,O,Q>, state:&mut M, budget:usize) {
    let ghost before = *driver; let ghost initial = *state;
    let result = driver.step(document,state,budget);
    proof { driver.step_facts(&before,document,initial,*state,budget,result); }
    match result {
        Ok(status) => {
            assert(!driver.poisoned && !driver.dirty && driver.protocol.pending_value() is None);
            let current = driver.is_current(document.ledger());
            assert(current == (status is Current));
        },
        Err(_) => {
            assert(driver.checkpoint.same(&before.checkpoint));
            assert(before.preflight(document,budget) ==> driver.poisoned);
            assert(!before.preflight(document,budget) ==> *state == initial);
        },
    }
}

pub(crate) fn zero_budget<C,O,Q>(driver:&mut Materialization, document:&Document<C,O,Q>, state:&mut Recording)
    requires old(driver).guard_valid(&document.ledger_view().history_view()),
{
    let ghost before = *driver; let ghost initial = *state;
    let result = driver.step(document,state,0);
    proof { driver.step_facts(&before,document,initial,*state,0,result); }
    assert(result == Err(DriveError::Stockfoil(Error::Limit("materialization budget must be positive"))));
    assert(*state == initial && driver.same_except_owner(&before));
}

/// The checked implementation, not an assumed application law, identifies callback errors.
pub(crate) proof fn application_errors_are_exact<C,O,Q>(document:&Document<C,O,Q>, before:Materialization,
    initial:Recording, after:Materialization, state:Recording, budget:usize, result:Result<Step,DriveError<u8>>)
    requires step::body_outcome(document,before,initial,budget,after,state,result),
    ensures (result matches Err(DriveError::Apply(e)) ==> e == 11 && state.applied),
        (result matches Err(DriveError::Checkpoint(e)) ==> e == 22 && state.saved is Some),
{
    let (planned, plan) = choose|planned:Materialization, plan:Result<Option<crate::batch_protocol::Batch>,Error>|
        planned.plan_outcome(&before,&document.ledger_view(),budget,plan)
        && planned.plan_errors(&before,&document.ledger_view(),budget,plan)
        && match plan {
            Err(e) => after == planned && state == initial && result == Err(DriveError::Stockfoil(e)),
            Ok(batch) => exists|completed:Materialization, applied:Recording, out:Result<(),DriveError<u8>>|
                step::batches_outcome(document,planned,initial,batch,completed,applied,out)
                && match out { Err(e) => after == completed && state == applied && result == Err(e),
                    Ok(()) => step::finish_outcome(document,completed,applied,batch is Some,after,state,result) },
        };
    if let Ok(batch) = plan {
        let (completed, applied, out) = choose|completed:Materialization, applied:Recording, out:Result<(),DriveError<u8>>|
            step::batches_outcome(document,planned,initial,batch,completed,applied,out)
                && match out { Err(e) => after == completed && state == applied && result == Err(e),
                    Ok(()) => step::finish_outcome(document,completed,applied,batch is Some,after,state,result) };
        if let Err(error) = out {
            let (count, tail, prior, next) = choose|count:nat,tail:Materialization,prior:Recording,next:Option<crate::batch_protocol::Batch>|
                step::rounds(document,planned,initial,batch,tail,prior,next,count)
                && next is Some && step::tail_error(document,tail,prior,next.unwrap(),completed,applied,error,step::activation_set(batch));
        }
    }
}

pub(crate) fn checked_application<C,O,Q>(driver:&mut Materialization, document:&Document<C,O,Q>, state:&mut Recording,budget:usize) {
    let ghost before = *driver; let ghost initial = *state;
    let result = driver.step(document,state,budget);
    proof {
        driver.step_facts(&before,document,initial,*state,budget,result);
        if before.preflight(document,budget) {
            let (prepared,ended) = choose|prepared:Materialization,ended:Materialization|
                prepared.prepared_from(&before,document)
                && step::body_outcome(document,prepared,initial,budget,ended,*state,result)
                && match result { Ok(_) => *driver == ended, Err(_) => driver.rolled_back(&ended,&before.checkpoint) };
            application_errors_are_exact(document,prepared,initial,ended,*state,budget,result);
        }
    }
    assert(result matches Err(DriveError::Apply(e)) ==> e == 11 && state.applied);
    assert(result matches Err(DriveError::Checkpoint(e)) ==> e == 22 && state.saved is Some);
}


pub(crate) proof fn no_rounds_without_batch<C,M:Materializer,O,Q>(document:&Document<C,O,Q>,
    first:Materialization, initial:M, last:Materialization, state:M, next:Option<crate::batch_protocol::Batch>, count:nat)
    requires step::rounds(document,first,initial,None,last,state,next,count),
    ensures count == 0, last == first, state == initial, next is None,
    decreases count,
{
    if count > 0 {
        let (previous,prior,current,events,report) = choose|previous:Materialization,prior:M,current:crate::batch_protocol::Batch,events:Vec<Event<Arc<[u8]>>>,report:Applied|
            step::rounds(document,first,initial,None,previous,prior,Some(current),(count-1) as nat)
            && step::resolved(document,&current,events) && step::allowed_events(events,step::activation_set(None))
            && step::apply_effect(prior,state,&document.ledger_view().history_view(),events,Ok(report))
            && last.completion_outcome(&previous,&document.ledger_view(),&current,&report,Ok(next))
            && last.completion_errors(&previous,&document.ledger_view(),&current,&report,Ok(next));
        no_rounds_without_batch(document,first,initial,previous,prior,Some(current),(count-1) as nat);
    }
}

pub(crate) proof fn no_callbacks_without_batch<C,M:Materializer,O,Q>(document:&Document<C,O,Q>,
    first:Materialization,initial:M,last:Materialization,state:M,out:Result<(),DriveError<M::Error>>)
    requires step::batches_outcome(document,first,initial,None,last,state,out),
    ensures out is Ok, last == first, state == initial,
{
    let (count,tail,prior,next) = choose|count:nat,tail:Materialization,prior:M,next:Option<crate::batch_protocol::Batch>|
        step::rounds(document,first,initial,None,tail,prior,next,count)
        && match out { Ok(()) => next is None && last == tail && state == prior,
            Err(e) => next is Some && step::tail_error(document,tail,prior,next.unwrap(),last,state,e,step::activation_set(None)) };
    no_rounds_without_batch(document,first,initial,tail,prior,next,count);
}

/// Exercise the real checkpoint call on a current driver, including a hook that mutates then fails.
pub(crate) fn current_checkpoint<C,O,Q>(driver:&mut Materialization,document:&Document<C,O,Q>,fail:bool)
    requires old(driver).current(&document.ledger_view()), old(driver).guard_valid(&document.ledger_view().history_view()),
        old(driver).source is None || old(driver).source.unwrap().finished(),
{
    proof { driver.current_facts(&document.ledger_view()); }
    driver.dirty = true;
    proof { driver.current_facts(&document.ledger_view()); assert(driver.current(&document.ledger_view())); }
    let mut state = Recording { applied:false,saved:None,fail_apply:false,fail_checkpoint:fail };
    let ghost before = *driver; let ghost initial = state;
    let result = driver.step(document,&mut state,1);
    proof {
        before.current_facts(&document.ledger_view());
        driver.step_facts(&before,document,initial,state,1,result);
        let (prepared,ended) = choose|prepared:Materialization,ended:Materialization|
            prepared.prepared_from(&before,document) && step::body_outcome(document,prepared,initial,1,ended,state,result)
            && match result { Ok(_) => *driver == ended, Err(_) => driver.rolled_back(&ended,&before.checkpoint) };
        prepared.current_facts(&document.ledger_view());
        assert(prepared.current(&document.ledger_view()));
        let (planned,plan) = choose|planned:Materialization,plan:Result<Option<crate::batch_protocol::Batch>,Error>|
            planned.plan_outcome(&prepared,&document.ledger_view(),1,plan) && planned.plan_errors(&prepared,&document.ledger_view(),1,plan)
            && match plan { Err(e) => ended == planned && state == initial && result == Err(DriveError::Stockfoil(e)),
                Ok(batch) => exists|completed:Materialization,applied:Recording,out:Result<(),DriveError<u8>>|
                    step::batches_outcome(document,planned,initial,batch,completed,applied,out)
                    && match out { Err(e) => ended == completed && state == applied && result == Err(e),
                        Ok(()) => step::finish_outcome(document,completed,applied,batch is Some,ended,state,result) } };
        planned.plan_facts(&prepared,&document.ledger_view(),1,plan);
        assert(plan == Ok(None));
        let (completed,applied,out) = choose|completed:Materialization,applied:Recording,out:Result<(),DriveError<u8>>|
            step::batches_outcome(document,planned,initial,None,completed,applied,out)
            && match out { Err(e) => ended == completed && state == applied && result == Err(e),
                Ok(()) => step::finish_outcome(document,completed,applied,false,ended,state,result) };
        no_callbacks_without_batch(document,planned,initial,completed,applied,out);
        assert(step::finish_outcome(document,planned,initial,false,ended,state,result));
        planned.current_facts(&document.ledger_view()); ended.current_facts(&document.ledger_view());
        assert(ended.current(&document.ledger_view()));
    }
    assert(!state.applied && state.saved is Some && state.saved.unwrap().same(&before.checkpoint));
    if fail {
        assert(result == Err(DriveError::Checkpoint(22)) && driver.poisoned);
        assert(driver.checkpoint.same(&before.checkpoint) && driver.dirty);
    } else {
        assert(result == Ok(Step::Current) && !driver.dirty);
        assert(state.saved.unwrap().same(&driver.checkpoint));
    }
}


/// The bound used for every apply call is the first plan's captured authorization, minus rejections.
pub(crate) proof fn callback_activation_bound<C,O,Q>(document:&Document<C,O,Q>, before:Materialization,
    planned:Materialization,budget:usize,batch:crate::batch_protocol::Batch)
    requires planned.plan_outcome(&before,&document.ledger_view(),budget,Ok(Some(batch))),
        planned.plan_errors(&before,&document.ledger_view(),budget,Ok(Some(batch))),
    ensures forall|hash:crate::commit_hash::CommitHash| step::activation_set(Some(batch)).contains(hash) ==>
        planned.source.unwrap().after_members().contains(hash)
        && !crate::materialization_events::rejected(&before.checkpoint,&document.ledger_view().history_view(),hash),
{
    planned.plan_facts(&before,&document.ledger_view(),budget,Ok(Some(batch)));
    let pred = |e:crate::batch_protocol::EventView<u64>| crate::materialization_events::activation(e) is Some;
    let filtered = batch.events_view().filter(pred);
    let mapped = filtered.map(|i:int,e:crate::batch_protocol::EventView<u64>| crate::materialization_events::activation(e).unwrap());
    assert forall|hash:crate::commit_hash::CommitHash| step::activation_set(Some(batch)).contains(hash) implies
        planned.source.unwrap().after_members().contains(hash)
        && !crate::materialization_events::rejected(&before.checkpoint,&document.ledger_view().history_view(),hash) by {
        let i = choose|i:int| 0 <= i < mapped.len() && mapped[i] == hash;
        batch.events_view().lemma_filter_pred(pred,i);
        assert(filtered.contains(filtered[i]));
        batch.events_view().lemma_filter_contains_rev(pred,filtered[i]);
        let j = choose|j:int| 0 <= j < batch.events_view().len() && batch.events_view()[j] == filtered[i];
        assert(crate::materialization_events::activation(batch.events_view()[j]) == Some(hash));
    }
}

} // verus!
