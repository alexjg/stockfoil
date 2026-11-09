//! Checked callers of the production update and exact progress aggregation.
use vstd::prelude::*;
use crate::{api_error::Error, document::Document, drive_error::DriveError,
    materialization_driver::Materialization, materialization_events::Counts,
    materialization_update::updates, materializer::{Materializer,Step}, progress::{Progress,Totals,sum}};

verus! {

/// Count the actual successful More steps, using the checked ticket progression.
pub(crate) proof fn prefix_progress<C,M:Materializer,O,Q>(document:&Document<C,O,Q>,
    first:Materialization,initial:M,last:Materialization,state:M,counts:Seq<Counts>,trace:Seq<(Materialization,M)>,n:nat)
    requires updates(document,first,initial,last,state,counts,trace), n <= counts.len(),
    ensures first.protocol.sequence() + n <= trace[n as int].0.protocol.sequence(),
    decreases n,
{
    if n > 0 {
        let previous = trace[n as int-1];
        let next = trace[n as int];
        assert(counts[n as int-1] == trace[n as int].0.last);
        next.0.step_facts(&previous.0,document,previous.1,next.1,1024,Ok(Step::More));
        prefix_progress(document,first,initial,last,state,counts,trace,(n-1) as nat);
    }
}

/// Success reports exact totals; errors retain the final step's recovery boundary.
/// There is deliberately no claim of rollback to the start of the entire update.
pub(crate) fn public_update<C,M:Materializer,O,Q>(driver:&mut Materialization,document:&Document<C,O,Q>,state:&mut M) {
    let ghost before = *driver; let ghost initial = *state;
    let result = driver.update(document,state);
    proof {
        driver.update_facts(&before,document,initial,*state,result);
        let (previous,prior,counts,trace,status) = choose|previous:Materialization,prior:M,counts:Seq<Counts>,trace:Seq<(Materialization,M)>,status:Result<Step,DriveError<M::Error>>|
            updates(document,before,initial,previous,prior,counts,trace) && sum(counts).fits()
            && driver.step_outcome(&previous,document,prior,*state,1024,status)
            && driver.step_status(&previous,document,1024,status)
            && match status {
                Err(e) => result == Err(e),
                Ok(step) => if !sum(counts).plus(driver.last).fits() {
                    result == Err(DriveError::Stockfoil(Error::Exhausted("materialization progress")))
                } else {
                    step is Current && (result matches Ok(progress)
                        && progress.totals() == sum(counts).plus(driver.last)
                        && progress.heads_before@ == before.checkpoint_view().cursor_view().target_view().heads_view()
                        && progress.heads_after@ == driver.checkpoint_view().cursor_view().target_view().heads_view())
                },
            };
        prefix_progress(document,before,initial,previous,prior,counts,trace,counts.len());
        driver.step_facts(&previous,document,prior,*state,1024,status);
        match status {
            Err(e) => {
                assert(result == Err(e));
                assert(driver.checkpoint.same(&previous.checkpoint));
                assert(previous.preflight(document,1024) ==> driver.poisoned);
                assert(!previous.preflight(document,1024) ==> *state == prior);
            },
            Ok(step) => {
                assert(!driver.poisoned && !driver.dirty && driver.protocol.pending_value() is None);
                if result is Err {
                    assert(result == Err(DriveError::Stockfoil(Error::Exhausted("materialization progress"))));
                    assert(!sum(counts).plus(driver.last).fits());
                } else {
                    assert(step is Current && driver.current(&document.ledger_view()));
                    assert(result->Ok_0.totals() == sum(counts).plus(driver.last));
                    assert(result->Ok_0.heads_before@ == before.checkpoint_view().cursor_view().target_view().heads_view());
                    assert(result->Ok_0.heads_after@ == driver.checkpoint_view().cursor_view().target_view().heads_view());
                }
            },
        }
        assert(true);
    }
}

/// Concrete arithmetic witnesses exercise each counter and sticky rebuilds.
pub(crate) fn progress_witness() {
    let mut report = Progress::default();
    let first = Counts { delivered:2,activated:3,retracted:5,rejected:7,rebuilt:true };
    let second = Counts { delivered:11,activated:13,retracted:17,rejected:19,rebuilt:false };
    let a = report.accumulate(&first);
    let b = report.accumulate(&second);
    assert(a is Ok && b is Ok);
    assert(report.delivered == 13 && report.activated == 16 && report.retracted == 22 && report.rejected == 26 && report.rebuilt);
    let changed = report.changed();
    assert(changed);
    let copied = report.clone();
    assert(copied.same(&report));
}

pub(crate) fn overflow_witness() {
    let mut report = Progress::default();
    report.rejected = usize::MAX;
    let ghost before = report;
    let counts = Counts { delivered:1,activated:2,retracted:3,rejected:1,rebuilt:true };
    let out = report.accumulate(&counts);
    assert(out == Err(Error::Exhausted("materialization progress")));
    assert(report == before);
    let zero = report.accumulate(&Counts::default());
    assert(zero is Ok && report.same(&before));
}

pub(crate) fn no_activity_witness() {
    let report = Progress::default();
    assert(report.totals() == Totals::zero());
    let changed = report.changed();
    assert(!changed);
}

} // verus!
