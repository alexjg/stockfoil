//! Acknowledge a planned batch without new feedback, including existing rejections.
use vstd::prelude::*;
use crate::{batch_protocol::{Batch,EventView},checkpoint::Checkpoint,commit_hash::CommitHash,
    history::History,ledger::Ledger,materialization_driver::Materialization,
    materialization_events as events,materialization_feedback::{self as feedback,Applied},
    materialization_complete as complete,materialization_corrections as corrections};

verus! {

pub(crate) proof fn active_safe(history: &History,checkpoint: &Checkpoint,emitted: Seq<EventView<u64>>)
    requires forall|i: int| 0 <= i < emitted.len() && events::activation(emitted[i]) is Some
        ==> !events::rejected(checkpoint,history,events::activation(emitted[i]).unwrap()),
    ensures forall|i: int| 0 <= i < events::active_deliveries(emitted).len()
        ==> !events::rejected(checkpoint,history,events::active_deliveries(emitted)[i]),
    decreases emitted.len(),
{
    if emitted.len() > 0 {
        active_safe(history,checkpoint,emitted.drop_last());
        assert forall|i: int| 0 <= i < events::active_deliveries(emitted).len()
            implies !events::rejected(checkpoint,history,events::active_deliveries(emitted)[i]) by {
            let previous = events::active_deliveries(emitted.drop_last());
            if i < previous.len() { assert(events::active_deliveries(emitted)[i] == previous[i]); }
            else {
                assert(events::activation(emitted[emitted.len() as int-1]) is Some);
                assert(events::active_deliveries(emitted)[i] == events::activation(emitted.last()).unwrap());
            }
        }
    }
}

proof fn no_affected(history: &History,checkpoint: &Checkpoint,active: Seq<CommitHash>,corrected: Set<CommitHash>)
    requires forall|i: int| 0 <= i < active.len() ==> !events::rejected(checkpoint,history,active[i]),
    ensures corrections::affected(history,checkpoint,corrected,active).len() == 0,
{
    corrections::affected_facts(history,checkpoint,corrected,active);
    let selected = corrections::affected(history,checkpoint,corrected,active);
    assert(selected.to_set() =~= Set::<CommitHash>::empty());
    if selected.len() > 0 { assert(selected.to_set().contains(selected[0])); }
}

/// An unchanged report cannot create new corrections for already-filtered active records.
pub(crate) fn acknowledge<O,Q>(driver: &mut Materialization,ledger: &Ledger<O,Q>,batch: Batch)
    requires old(driver).guard_valid(&ledger.history_view()),old(driver).protocol.accepts(&batch),
        old(driver).protocol.pending_value().unwrap().next.follow_view().same(&old(driver).checkpoint.follow_view()),
        !old(driver).protocol.pending_value().unwrap().restart,
        forall|i: int| 0 <= i < old(driver).protocol.pending_value().unwrap().active.len()
            ==> !events::rejected(&old(driver).protocol.pending_value().unwrap().next,&ledger.history_view(),old(driver).protocol.pending_value().unwrap().active[i]),
    ensures final(driver).checkpoint.same(&old(driver).protocol.pending_value().unwrap().next),
        crate::replay_plan::same_counts(final(driver).last,old(driver).protocol.pending_value().unwrap().counts),
        final(driver).source == old(driver).source,final(driver).protocol.pending_value() is None,
        final(driver).guard_valid(&ledger.history_view()),!final(driver).protocol.accepts(&batch),
{
    let ghost before = *driver; let ghost pending = before.protocol.pending_value().unwrap();
    let report = Applied::default();
    let result = driver.complete(ledger,batch,report);
    proof {
        driver.completion_facts(&before,ledger,&batch,&report,result);
        assert(complete::report_valid(&before,&ledger.history_view(),&batch,&report));
        assert(feedback::rejection_hashes(report.rejected@).to_set() =~= Set::empty());
        assert(feedback::merged(pending.next.rejected_view(),report.rejected@) == pending.next.rejected_view());
        assert(feedback::rejection_hashes(report.rejected@).to_set().difference(pending.next.rejected_view().dom()) =~= Set::<CommitHash>::empty());
        assert(complete::added_rejections(&pending,&report) == 0);
        if result is Err {
            let snapshot = choose|s: Checkpoint| complete::body_error(&before,&ledger.history_view(),&report,result->Err_0,&s);
            no_affected(&ledger.history_view(),&snapshot,pending.active@,pending.corrected@);
            assert(false);
        }
        let (after,emitted,snapshot) = choose|p: feedback::Pending,e: Seq<EventView<u64>>,s: Checkpoint|
            complete::round(&pending,&before.checkpoint,before.source,&ledger.history_view(),&report,before.supports_visibility_changes,&p,e,&s)
            && match result->Ok_0 {
                Some(next) => e.len() > 0 && driver.protocol.pending_value() == Some(p),
                None => e.len() == 0 && driver.checkpoint == p.next && driver.last == p.counts && driver.source == if p.restart { None } else { before.source },
            };
        no_affected(&ledger.history_view(),&snapshot,pending.active@,pending.corrected@);
        let requests = corrections::payload_hashes(emitted);
        if requests.len() > 0 { assert(requests.to_set().contains(requests[0])); }
        assert(requests.len() == 0 && emitted.len() == 0);
        assert(result is Ok && result->Ok_0 is None);
    }
}

} // verus!
