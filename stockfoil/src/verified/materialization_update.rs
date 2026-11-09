//! Public update composes completed steps and reports exact cumulative activity.
#[cfg(verus_keep_ghost)]
use super::{api_error::Error, materialization_events::Counts, progress::sum};
use super::{
    commit_hash::CommitHash,
    document::Document,
    drive_error::DriveError,
    materialization_driver::Materialization,
    materializer::{Materializer, Step},
    progress::Progress,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) fn copy_heads(heads:&[CommitHash]) -> (out:Vec<CommitHash>)
    ensures out@ == heads@,
{
    let mut out = Vec::new();
    let mut i = 0;
    while i < heads.len()
        invariant i <= heads.len(), out@ == heads@.subrange(0,i as int),
        decreases heads.len() - i,
    {
        out.push(heads[i]);
        i += 1;
    }
    out
}


/// An explicit finite trace of successful More steps, with exact per-step counts.
#[verifier::prophetic]
pub(crate) open spec fn updates<C,M:Materializer,O,Q>(document:&Document<C,O,Q>,
    first:Materialization, initial:M, last:Materialization, state:M, counts:Seq<Counts>,trace:Seq<(Materialization,M)>) -> bool
{
    trace.len() == counts.len() + 1 && trace[0] == (first,initial) && trace.last() == (last,state)
    && sum(counts).fits()
    && forall|i:int| 0 <= i < counts.len() ==>
        counts[i] == trace[i+1].0.last
        && trace[i+1].0.step_outcome(&trace[i].0,document,trace[i].1,trace[i+1].1,1024,Ok(Step::More))
        && trace[i+1].0.step_status::<C,M::Error,O,Q>(&trace[i].0,document,1024,Ok(Step::More))
}

impl Materialization {
    #[verifier::prophetic]
    pub closed spec fn update_outcome<C,M:Materializer,O,Q>(&self,before:&Self,document:&Document<C,O,Q>,
        initial:M,state:M,out:Result<Progress,DriveError<M::Error>>) -> bool {
        self.update_details(before,document,initial,state,out)
    }
    #[verifier::prophetic]
    pub(crate) open spec fn update_details<C,M:Materializer,O,Q>(&self,before:&Self,document:&Document<C,O,Q>,
        initial:M,state:M,out:Result<Progress,DriveError<M::Error>>) -> bool {
        exists|previous:Materialization,prior:M,counts:Seq<Counts>,trace:Seq<(Materialization,M)>,status:Result<Step,DriveError<M::Error>>|
            updates(document,*before,initial,previous,prior,counts,trace) && sum(counts).fits()
            && self.step_outcome(&previous,document,prior,state,1024,status)
            && self.step_status(&previous,document,1024,status)
            && match status {
                Err(e) => out == Err(e),
                Ok(step) => if !sum(counts).plus(self.last).fits() {
                    out == Err(DriveError::Stockfoil(Error::Exhausted("materialization progress")))
                } else {
                    step is Current && (out matches Ok(progress)
                        && progress.totals() == sum(counts).plus(self.last)
                        && progress.heads_before@ == before.checkpoint_view().cursor_view().target_view().heads_view()
                        && progress.heads_after@ == self.checkpoint_view().cursor_view().target_view().heads_view())
                },
            }
    }
    pub(crate) proof fn update_facts<C,M:Materializer,O,Q>(&self,before:&Self,document:&Document<C,O,Q>,
        initial:M,state:M,out:Result<Progress,DriveError<M::Error>>)
        requires self.update_outcome(before,document,initial,state,out),
        ensures self.update_details(before,document,initial,state,out),
    {}

    /// Apply and checkpoint steps until current, reporting their cumulative activity.
    /// Earlier successful checkpoints remain published if a later step fails.
    /// A report-counter overflow returns `Error::Exhausted` after the successful
    /// step; its checkpoint remains valid and its driver is not newly poisoned.
    pub fn update<C,M:Materializer>(&mut self,document:&Document<C,impl Sized,impl Sized>,state:&mut M)
        -> (out:Result<Progress,DriveError<M::Error>>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).update_outcome(old(self),document,*old(state),*final(state),out),
            out is Ok ==> final(self).current(&document.ledger_view()) && !final(self).dirty_view()
                && out->Ok_0.heads_before@ == old(self).checkpoint_view().cursor_view().target_view().heads_view()
                && out->Ok_0.heads_after@ == final(self).checkpoint_view().cursor_view().target_view().heads_view(),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost first = *self; let ghost initial = *state;
        let mut progress = Progress::default();
        progress.heads_before = copy_heads(self.heads());
        let ghost mut counts:Seq<Counts> = Seq::empty();
        let ghost mut trace = seq![(first,initial)];
        loop
            invariant first == *old(self), initial == *old(state),
                first.source_consistent() ==> self.source_consistent(),
                first.checkpoint_consistent() ==> self.checkpoint_consistent(),
                first.counters_consistent() ==> self.counters_consistent(),
                updates(document,first,initial,*self,*state,counts,trace),
                progress.totals() == sum(counts), sum(counts).fits(),
                progress.heads_before@ == first.checkpoint_view().cursor_view().target_view().heads_view(),
                progress.heads_after@.len() == 0,
            decreases u64::MAX - self.protocol.sequence(),
        {
            let ghost previous = *self; let ghost prior = *state;
            let result = self.step(document,state,1024);
            proof { self.step_facts(&previous,document,prior,*state,1024,result); }
            let status = match result { Err(e) => return Err(e), Ok(status) => status };
            let aggregated = progress.accumulate(&self.last);
            match aggregated { Err(e) => return Err(DriveError::Stockfoil(e)), Ok(()) => {} }
            if let Step::Current = status {
                progress.heads_after = copy_heads(self.heads());
                proof { self.representation(); }
                return Ok(progress);
            }
            proof {
                let extended = trace.push((*self,*state));
                let next_counts = counts.push(self.last);
                assert(next_counts.drop_last() =~= counts);
                assert(sum(next_counts) == sum(counts).plus(self.last));
                assert forall|i:int| 0 <= i < next_counts.len() implies
                    next_counts[i] == extended[i+1].0.last
                    && extended[i+1].0.step_outcome(&extended[i].0,document,extended[i].1,extended[i+1].1,1024,Ok(Step::More))
                    && extended[i+1].0.step_status::<C,M::Error,_,_>(&extended[i].0,document,1024,Ok(Step::More)) by {
                    if i < counts.len() { assert(trace[i+1] == extended[i+1]); }
                    else { assert(trace[i] == (previous,prior)); }
                }
                assert(updates(document,first,initial,*self,*state,next_counts,extended));
                counts = next_counts;
                trace = extended;
            }
        }
    }
}

} // verus!
