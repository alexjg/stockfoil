//! Public completion preserves initial counts and counts only distinct corrections.
use vstd::prelude::*;
use crate::{batch_protocol::{Batch,EventView}, checkpoint::Checkpoint, commit_hash::CommitHash,
    ledger::Ledger, materialization_driver::Materialization,
    materialization_feedback::{Applied,Pending}, materialization_invariants as invariants,
    materialization_complete as complete};
verus! {

pub(crate) proof fn pending_correction_bound(driver:&Materialization)
    requires driver.counters_consistent(), driver.protocol.pending_value() is Some,
    ensures driver.protocol.pending_value().unwrap().counts.delivered == driver.protocol.pending_value().unwrap().delivered@.len(),
        driver.protocol.pending_value().unwrap().counts.activated == driver.protocol.pending_value().unwrap().origin@.activated,
        driver.protocol.pending_value().unwrap().counts.retracted == driver.protocol.pending_value().unwrap().origin@.retracted
            + driver.protocol.pending_value().unwrap().corrected@.len(),
        driver.protocol.pending_value().unwrap().corrected@.subset_of(driver.protocol.pending_value().unwrap().original_active@.to_set()),
        driver.protocol.pending_value().unwrap().corrected@.len() <= driver.protocol.pending_value().unwrap().counts.delivered,
{
    Materialization::counter_consistency_definition();
    invariants::correction_budget(&driver.protocol.pending_value().unwrap(),&driver.source.unwrap());
}

pub(crate) fn public_completion_counts<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,batch:Batch,report:Applied)
    requires old(driver).counters_consistent(),
    ensures final(driver).counters_consistent(),
{
    let ghost before = *driver;
    let result = driver.complete(ledger,batch,report);
    proof {
        Materialization::counter_consistency_definition();
        Materialization::checkpoint_consistency_definition();
        Materialization::source_consistency_definition();
        driver.completion_facts(&before,ledger,&batch,&report,result);
        if result is Ok {
            let (pending,emitted,snapshot) = choose|pending:Pending,emitted:Seq<EventView<u64>>,snapshot:Checkpoint|
                complete::round(&before.protocol.pending_value().unwrap(),&before.checkpoint,before.source,&ledger.history_view(),
                    &report,before.supports_visibility_changes,&pending,emitted,&snapshot)
                && match result->Ok_0 {
                    Some(_) => driver.protocol.pending_value() == Some(pending),
                    None => driver.last == pending.counts,
                };
            let original = before.protocol.pending_value().unwrap();
            invariants::round_preserves_counts(&original,&before.checkpoint,before.source.unwrap(),&ledger.history_view(),
                &report,before.supports_visibility_changes,&pending,emitted,&snapshot);
            assert(pending.counts.delivered == original.origin@.delivered);
            assert(pending.counts.activated == original.origin@.activated);
            assert(pending.counts.retracted == original.counts.retracted + pending.corrected@.difference(original.corrected@).len());
            assert(original.corrected@.subset_of(pending.corrected@));
            if pending.corrected@ == original.corrected@ {
                assert(pending.counts.retracted == original.counts.retracted);
            }
            if result->Ok_0 is None {
                assert(driver.completed_counters_match(&before));
                assert(driver.last.retracted <= original.origin@.retracted + original.original_active@.len());
            }
        }
    }
}

/// Payload requests may repeat; a remembered correction contributes zero new retractions.
pub(crate) proof fn remembered_corrections_do_not_recount(before:&Pending,after:&Pending,source:crate::materialization_source::Source,
    published:&Checkpoint,history:&crate::history::History,report:&Applied,supports:bool,events:Seq<EventView<u64>>,snapshot:&Checkpoint)
    requires invariants::counted(before,&source),
        complete::round(before,published,Some(source),history,report,supports,after,events,snapshot),
        after.corrected@.subset_of(before.corrected@),
    ensures after.counts.retracted == before.counts.retracted,
{
    invariants::round_preserves_counts(before,published,source,history,report,supports,after,events,snapshot);
    assert(after.corrected@.difference(before.corrected@) =~= Set::<CommitHash>::empty());
}
}
