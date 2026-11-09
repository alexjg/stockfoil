//! Actual public API interleavings preserve source readiness and ownership.
//! The caller supplies arbitrary reports, tokens, checkpoints and documents.
use vstd::prelude::*;
use crate::{
    api_error::Error, batch_protocol::Batch, checkpoint::{Checkpoint, Follow},
    document::Document, draft::Draft, materialization_driver::Materialization,
    materialization_feedback::Applied, step_examples::Recording,
};
verus! {

pub(crate) enum Action {
    Follow(Follow),
    Plan(usize),
    Complete(Batch, Applied),
    Report(Applied),
    Local(Draft),
    Step(usize),
    Update,
    Edit,
    Resume(Checkpoint, bool),
}

// Only the protocol is under examination: no application-state law is assumed.
fn edit_bytes(_: &mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures out matches Ok(Some(bytes)) && bytes@ == seq![7u8],
{ Ok(Some(vec![7u8])) }

/// Constructor establishment plus induction over arbitrary public mutations.
/// Errors are retained in the trace; the next action may retry or resume.
pub(crate) fn arbitrary_interleavings<C,O,Q>(document:&mut Document<C,O,Q>,state:&mut Recording,
    actions:Vec<Action>,supports:bool) -> (driver:Materialization)
    ensures driver.source_consistent(), driver.checkpoint_consistent(), driver.counters_consistent(),
{
    let mut driver = Materialization::new(Follow::Working,supports);
    let mut pending:Option<Batch> = None;
    for action in iter: actions
        invariant driver.source_consistent(), driver.checkpoint_consistent(), driver.counters_consistent(),
    {
        match action {
            Action::Follow(follow) => driver.set_follow(follow),
            Action::Plan(budget) => {
                let ghost before = driver;
                let result = driver.plan(document.ledger(),budget);
                assert((result matches Err(Error::Poisoned)) ==> before.poisoned_view());
                assert((result matches Err(Error::WrongHistory)) ==> before.owner_view() is Some
                    && before.owner_view().unwrap() != document.ledger_view().history_view().history_id());
                if let Ok(batch) = result { pending = batch; }
            },
            Action::Complete(batch,report) => {
                let ghost before = driver;
                let result = driver.complete(document.ledger(),batch,report);
                assert((result matches Err(Error::Poisoned)) ==> before.poisoned_view());
            },
            Action::Report(report) => {
                if let Some(batch) = pending.take() {
                    let ghost before = driver;
                    let result = driver.complete(document.ledger(),batch,report);
                    assert((result matches Err(Error::Poisoned)) ==> before.poisoned_view());
                    if let Ok(followup) = result { pending = followup; }
                }
            },
            Action::Local(draft) => { let _ = driver.mark_local(document.ledger(),&draft); },
            Action::Step(budget) => { let _ = driver.step(document,state,budget); },
            Action::Update => { let _ = driver.update(document,state); },
            Action::Edit => { let _ = driver.change(document,state,edit_bytes); },
            Action::Resume(checkpoint,supports) => driver = Materialization::resume(checkpoint,supports),
        }
        assert(driver.source_consistent());
        assert(driver.checkpoint_consistent());
        assert(driver.counters_consistent());
    }
    driver
}

/// A token emitted by plan carries a ready source through any accepted follow-ups.
/// No bound on callbacks or counters is assumed; exhaustion remains an error.
pub(crate) fn completion_chain<O,Q>(driver:&mut Materialization,ledger:&crate::ledger::Ledger<O,Q>,
    batch:Batch,reports:Vec<Applied>)
    requires old(driver).source_consistent(), old(driver).checkpoint_consistent(), old(driver).counters_consistent(),
    ensures final(driver).source_consistent(), final(driver).checkpoint_consistent(), final(driver).counters_consistent(),
{
    let mut next = Some(batch);
    for report in iter: reports
        invariant driver.source_consistent(), driver.checkpoint_consistent(), driver.counters_consistent(),
    {
        let batch = match next { Some(batch) => batch, None => return };
        let ghost before = *driver;
        let result = driver.complete(ledger,batch,report);
        assert((result matches Err(Error::Poisoned)) ==> before.poisoned_view());
        match result { Ok(followup) => next = followup, Err(_) => return }
    }
}

/// Public construction/resume establishes the invariant for arbitrary checkpoint
/// contents; checkpoint reference validity is still checked by the history guard.
pub(crate) fn resumed_source(checkpoint:Checkpoint,supports:bool) {
    let driver = Materialization::resume(checkpoint,supports);
    assert(driver.source_consistent());
    assert(driver.checkpoint_consistent());
    assert(driver.counters_consistent());
    proof { Materialization::source_consistency_definition(); driver.initial_facts(); }
    assert(driver.source is None && driver.protocol.pending_value() is None);
}
}
