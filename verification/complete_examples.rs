//! Public completion callers, including preservation, token lifetime and publication.
use vstd::prelude::*;
use crate::{api_error::Error, batch_protocol::Batch, checkpoint::Checkpoint, ledger::Ledger,
    materialization_driver::Materialization, materialization_feedback::{self as feedback, Applied},
    materialization_complete as complete, materialization_corrections as corrections};

verus! {

pub(crate) fn arbitrary_completion<O, Q>(driver: &mut Materialization, ledger: &Ledger<O, Q>, batch: Batch, applied: Applied) {
    let ghost before = *driver;
    let result = driver.complete(ledger, batch, applied);
    proof { driver.completion_facts(&before, ledger, &batch, &applied, result); }
    assert(driver.dirty == before.dirty && driver.poisoned == before.poisoned);
    match result {
        Err(_) => { assert(driver.checkpoint == before.checkpoint && driver.last == before.last && driver.source == before.source); },
        Ok(None) => { assert(driver.protocol.pending_value() is None && !driver.protocol.accepts(&batch)); },
        Ok(Some(next)) => {
            assert(driver.protocol.accepts(&next) && !driver.protocol.accepts(&batch));
            assert(driver.checkpoint == before.checkpoint && driver.last == before.last);
            assert forall|i: int| 0 <= i < next.events_view().len() implies
                crate::materialization_events::activation(next.events_view()[i]) is None by {}
        },
    }
}

pub(crate) proof fn no_rejections_no_corrections(history: &crate::history::History, checkpoint: &Checkpoint,
    active: Seq<crate::commit_hash::CommitHash>, corrected: Set<crate::commit_hash::CommitHash>)
    requires checkpoint.rejected_view().is_empty(),
    ensures corrections::affected(history, checkpoint, corrected, active).len() == 0,
{
    corrections::affected_facts(history, checkpoint, corrected, active);
    let selected = corrections::affected(history, checkpoint, corrected, active);
    assert(selected.to_set() =~= Set::<crate::commit_hash::CommitHash>::empty());
    if selected.len() > 0 { assert(selected.to_set().contains(selected[0])); }
}

/// A valid acknowledgement with no existing rejection needs neither a source nor a fresh ticket.
pub(crate) fn acknowledge_unrejected<O, Q>(driver: &mut Materialization, ledger: &Ledger<O, Q>, batch: Batch)
    requires old(driver).guard_valid(&ledger.history_view()), old(driver).protocol.accepts(&batch),
        old(driver).protocol.pending_value().unwrap().next.rejected_view().is_empty(),
    ensures final(driver).protocol.pending_value() is None, !final(driver).protocol.accepts(&batch),
        final(driver).owner == Some(ledger.history_view().history_id()), !final(driver).poisoned,
        final(driver).checkpoint.cursor_view() == old(driver).protocol.pending_value().unwrap().next.cursor_view(),
        final(driver).checkpoint.progress_view() == old(driver).protocol.pending_value().unwrap().next.progress_view(),
        final(driver).checkpoint.rejected_view().is_empty(), final(driver).checkpoint.follow_view().same(&old(driver).checkpoint.follow_view()),
        final(driver).last.delivered == old(driver).protocol.pending_value().unwrap().counts.delivered,
        final(driver).last.activated == old(driver).protocol.pending_value().unwrap().counts.activated,
        final(driver).last.retracted == old(driver).protocol.pending_value().unwrap().counts.retracted,
        final(driver).last.rejected == old(driver).protocol.pending_value().unwrap().counts.rejected,
        final(driver).last.rebuilt == old(driver).protocol.pending_value().unwrap().counts.rebuilt,
        (old(driver).source is None || old(driver).source.unwrap().finished()) ==> final(driver).source is None || final(driver).source.unwrap().finished(),
{
    let ghost before = *driver;
    let ghost pending = before.protocol.pending_value().unwrap();
    let applied = Applied::default();
    let result = driver.complete(ledger, batch, applied);
    proof {
        driver.completion_facts(&before, ledger, &batch, &applied, result);
        assert(complete::report_valid(&before, &ledger.history_view(), &batch, &applied));
        assert(feedback::rejection_hashes(applied.rejected@).to_set() =~= Set::empty());
        assert(feedback::merged(pending.next.rejected_view(), applied.rejected@) =~= Map::empty());
        assert(feedback::rejection_hashes(applied.rejected@).to_set().difference(pending.next.rejected_view().dom()) =~= Set::empty());
        assert(complete::added_rejections(&pending, &applied) == 0);
        if result is Err {
            let snapshot = choose|snapshot: Checkpoint| complete::body_error(&before, &ledger.history_view(), &applied, result->Err_0, &snapshot);
            no_rejections_no_corrections(&ledger.history_view(), &snapshot, pending.active@, pending.corrected@);
            assert(false);
        }
        let (after, emitted, snapshot) = choose|after: feedback::Pending, emitted: Seq<crate::batch_protocol::EventView<u64>>, snapshot: Checkpoint|
            complete::round(&pending, &before.checkpoint, before.source, &ledger.history_view(), &applied,
                before.supports_visibility_changes, &after, emitted, &snapshot)
                && match result->Ok_0 {
                    Some(next) => emitted.len() > 0 && driver.protocol.pending_value() == Some(after),
                    None => emitted.len() == 0 && driver.checkpoint == after.next && driver.last == after.counts,
                };
        no_rejections_no_corrections(&ledger.history_view(), &snapshot, pending.active@, pending.corrected@);
        let requests = corrections::payload_hashes(emitted);
        if requests.len() > 0 { assert(requests.to_set().contains(requests[0])); }
        assert(requests.len() == 0 && emitted.len() == 0);
        assert(result is Ok && result->Ok_0 is None);
    }
}

pub(crate) fn invalid_feedback_retains_token<O, Q>(driver: &mut Materialization, ledger: &Ledger<O, Q>, batch: Batch, applied: Applied)
    requires old(driver).guard_valid(&ledger.history_view()), old(driver).protocol.accepts(&batch),
        !feedback::valid_rejections(old(driver).protocol.pending_value().unwrap().delivered@, &applied),
{
    let ghost before = *driver;
    let retry = batch.clone();
    let result = driver.complete(ledger, batch, applied);
    proof { driver.completion_facts(&before, ledger, &batch, &applied, result); }
    assert(result is Err && result->Err_0 is Protocol);
    assert(driver.same_except_owner(&before));
    assert(driver.protocol.accepts(&retry));
}

pub(crate) fn poisoned_precedes_feedback<O, Q>(driver: &mut Materialization, ledger: &Ledger<O, Q>, batch: Batch, applied: Applied)
    requires old(driver).poisoned,
{
    let ghost before = *driver;
    let result = driver.complete(ledger, batch, applied);
    proof { driver.completion_facts(&before, ledger, &batch, &applied, result); }
    assert(result is Err && result->Err_0 is Poisoned && *driver == before);
}

pub(crate) fn repeated_feedback_does_not_recount<O, Q>(driver: &mut Materialization, ledger: &Ledger<O, Q>,
    batch: Batch, first: Applied, again: Applied)
    requires again.rejected@ == first.rejected@,
{
    let ghost before = *driver;
    let first_result = driver.complete(ledger, batch, first);
    proof { driver.completion_facts(&before, ledger, &batch, &first, first_result); }
    if let Ok(Some(next)) = first_result {
        let ghost waiting = *driver;
        proof {
            let (pending, emitted, snapshot) = choose|pending: feedback::Pending, emitted: Seq<crate::batch_protocol::EventView<u64>>, snapshot: Checkpoint|
                complete::round(&before.protocol.pending_value().unwrap(), &before.checkpoint, before.source,
                    &ledger.history_view(), &first, before.supports_visibility_changes, &pending, emitted, &snapshot)
                    && waiting.protocol.pending_value() == Some(pending);
            assert(feedback::rejection_hashes(again.rejected@).to_set().subset_of(pending.next.rejected_view().dom()));
            assert(feedback::rejection_hashes(again.rejected@).to_set().difference(pending.next.rejected_view().dom()) =~= Set::empty());
            assert(complete::added_rejections(&pending, &again) == 0);
        }
        let second_result = driver.complete(ledger, next, again);
        proof {
            driver.completion_facts(&waiting, ledger, &next, &again, second_result);
            if second_result is Ok {
                let (pending, emitted, snapshot) = choose|pending: feedback::Pending, emitted: Seq<crate::batch_protocol::EventView<u64>>, snapshot: Checkpoint|
                    complete::round(&waiting.protocol.pending_value().unwrap(), &waiting.checkpoint, waiting.source,
                        &ledger.history_view(), &again, waiting.supports_visibility_changes, &pending, emitted, &snapshot)
                        && match second_result->Ok_0 {
                            Some(_) => driver.protocol.pending_value() == Some(pending),
                            None => driver.last == pending.counts,
                        };
                assert(pending.counts.rejected == waiting.protocol.pending_value().unwrap().counts.rejected);
                assert(match second_result->Ok_0 {
                    Some(_) => driver.protocol.pending_value().unwrap().counts.rejected == waiting.protocol.pending_value().unwrap().counts.rejected,
                    None => driver.last.rejected == waiting.protocol.pending_value().unwrap().counts.rejected,
                });
            }
        }
    }
}

} // verus!
