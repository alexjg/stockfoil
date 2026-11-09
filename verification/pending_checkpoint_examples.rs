//! Public callers connect staged progress and cumulative rejection counts to publication.
use vstd::prelude::*;
use crate::{batch_protocol::Batch, commit_hash::CommitHash,
    ledger::Ledger, materialization_driver::Materialization,
    materialization_feedback::Applied, materialization_invariants::{self as invariants, pending_aligned, staged}};
verus! {

pub(crate) proof fn pending_progress_is_bounded(driver:&Materialization)
    requires driver.checkpoint_consistent(), driver.protocol.pending_value() is Some,
    ensures driver.source is Some,
        (driver.protocol.pending_value().unwrap().restart ==> driver.protocol.pending_value().unwrap().next.progress_view() is Some
            && driver.protocol.pending_value().unwrap().next.progress_view().unwrap().applied == 0),
        (driver.protocol.pending_value().unwrap().next.progress_view() is Some && !driver.protocol.pending_value().unwrap().restart ==>
            driver.protocol.pending_value().unwrap().next.progress_view().unwrap().applied == driver.source.unwrap().processed()),
        (driver.protocol.pending_value().unwrap().next.progress_view() is Some ==>
            driver.protocol.pending_value().unwrap().next.progress_view().unwrap().target.same(&driver.source.unwrap().target())
            && driver.protocol.pending_value().unwrap().next.progress_view().unwrap().delivery_heads@ == driver.source.unwrap().delivery()),
        driver.protocol.pending_value().unwrap().counts.rejected == driver.protocol.pending_value().unwrap().next.rejected_view().dom().difference(driver.checkpoint.rejected_view().dom()).len(),
        driver.source.unwrap().processed() <= driver.source.unwrap().total(),
        (driver.protocol.pending_value().unwrap().next.progress_view() is Some ==>
            driver.protocol.pending_value().unwrap().next.progress_view().unwrap().applied <= driver.source.unwrap().total()),
{
    Materialization::checkpoint_consistency_definition();
    Materialization::source_consistency_definition();
}

pub(crate) fn completed_round<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,batch:Batch,report:Applied)
    requires old(driver).checkpoint_consistent(),
    ensures final(driver).checkpoint_consistent(),
{
    let ghost before = *driver;
    let result = driver.complete(ledger,batch,report);
    proof {
        Materialization::checkpoint_consistency_definition();
        Materialization::source_consistency_definition();
        driver.completion_facts(&before,ledger,&batch,&report,result);
    }
    proof { match result {
        Ok(None) => {
            assert(driver.protocol.pending_value() is None);
            assert(driver.completed_checkpoint_matches(&before));
            assert(staged(&before.source.unwrap(),&driver.checkpoint,driver.source is None));
            assert(driver.last.rejected == driver.checkpoint.rejected_view().dom().difference(before.checkpoint.rejected_view().dom()).len());
            if driver.checkpoint.progress_view() is Some {
                assert(driver.checkpoint.progress_view().unwrap().applied <= before.source.unwrap().total());
            }
        },
        Ok(Some(_)) => {
            assert(driver.checkpoint == before.checkpoint);
            assert(pending_aligned(&driver.protocol.pending_value().unwrap(),&driver.source.unwrap(),&driver.checkpoint));
            pending_progress_is_bounded(driver);
            if driver.protocol.pending_value().unwrap().restart {
                assert(driver.protocol.pending_value().unwrap().next.progress_view().unwrap().applied == 0);
            }
        },
        Err(_) => { assert(driver.checkpoint == before.checkpoint); },
    } }
}

pub(crate) fn planned_round<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,budget:usize)
    requires old(driver).checkpoint_consistent(),
    ensures final(driver).checkpoint_consistent(),
{
    let ghost before = *driver;
    let result = driver.plan(ledger,budget);
    proof {
        Materialization::checkpoint_consistency_definition();
        driver.plan_facts(&before,ledger,budget,result);
    }
    if result matches Ok(Some(_)) {
        assert(driver.protocol.pending_value().unwrap().counts.rejected == 0);
        assert(!driver.protocol.pending_value().unwrap().restart);
        assert(staged(&driver.source.unwrap(),&driver.protocol.pending_value().unwrap().next,false));
        proof { pending_progress_is_bounded(driver); }
    }
}

/// A repeated report may strengthen cascade flags, but cannot inflate the count
/// of distinct rejection hashes recorded since the last published checkpoint.
pub(crate) proof fn repeated_rejections(base:Set<CommitHash>,prior:Set<CommitHash>,reported:Set<CommitHash>)
    requires base.subset_of(prior), reported.subset_of(prior),
    ensures prior.union(reported).difference(base).len() == prior.difference(base).len(),
{
    invariants::rejection_count_union(base,prior,reported);
    assert(reported.difference(prior) =~= Set::<CommitHash>::empty());
}
}
