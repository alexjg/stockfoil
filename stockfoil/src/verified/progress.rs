//! Actual public progress values and checked aggregation across completed steps.
use super::{api_error::Error, commit_hash::CommitHash, materialization_events::Counts};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Activity reported by one update, including repeated deliveries during rebuilds.
/// Counts summarize events, not distinct commits or differences between the heads.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, PartialEq, Eq))]
pub struct Progress {
    pub heads_before: Vec<CommitHash>,
    pub heads_after: Vec<CommitHash>,
    pub delivered: usize,
    pub activated: usize,
    pub retracted: usize,
    pub rejected: usize,
    pub rebuilt: bool,
}

#[cfg(verus_keep_ghost)]
pub struct Totals {
    pub delivered: nat,
    pub activated: nat,
    pub retracted: nat,
    pub rejected: nat,
    pub rebuilt: bool,
}
#[cfg(verus_keep_ghost)]
impl Totals {
    pub open spec fn zero() -> Self {
        Self { delivered:0, activated:0, retracted:0, rejected:0, rebuilt:false }
    }
    pub open spec fn plus(self, counts:Counts) -> Self {
        Self { delivered:(self.delivered + counts.delivered) as nat, activated:(self.activated + counts.activated) as nat,
            retracted:(self.retracted + counts.retracted) as nat, rejected:(self.rejected + counts.rejected) as nat,
            rebuilt:self.rebuilt || counts.rebuilt }
    }
    pub open spec fn fits(self) -> bool {
        self.delivered <= usize::MAX && self.activated <= usize::MAX && self.retracted <= usize::MAX && self.rejected <= usize::MAX
    }
}

pub open spec fn sum(counts:Seq<Counts>) -> Totals
    decreases counts.len(),
{
    if counts.len() == 0 { Totals::zero() } else { sum(counts.drop_last()).plus(counts.last()) }
}

impl Progress {
    pub open spec fn totals(&self) -> Totals {
        Totals { delivered:self.delivered as nat, activated:self.activated as nat, retracted:self.retracted as nat,
            rejected:self.rejected as nat, rebuilt:self.rebuilt }
    }
    pub open spec fn same(&self, other:&Self) -> bool {
        self.heads_before@ == other.heads_before@ && self.heads_after@ == other.heads_after@ && self.totals() == other.totals()
    }
    /// Whether the report contains any event activity or a rebuild.
    /// A checkpoint-only update can still return false.
    pub fn changed(&self) -> (out:bool)
        ensures out == (self.totals() != Totals::zero()),
    {
        self.delivered > 0 || self.activated > 0 || self.retracted > 0 || self.rejected > 0 || self.rebuilt
    }

    /// Compute all sums before changing the report; overflow preserves it exactly.
    pub(crate) fn accumulate(&mut self, counts:&Counts) -> (out:Result<(),Error>)
        ensures final(self).heads_before@ == old(self).heads_before@, final(self).heads_after@ == old(self).heads_after@,
            match out {
                Ok(()) => final(self).totals() == old(self).totals().plus(*counts),
                Err(e) => *final(self) == *old(self) && !old(self).totals().plus(*counts).fits()
                    && e == Error::Exhausted("materialization progress"),
            },
    {
        let delivered = match self.delivered.checked_add(counts.delivered) {
            Some(n) => n, None => return Err(Error::Exhausted("materialization progress")),
        };
        let activated = match self.activated.checked_add(counts.activated) {
            Some(n) => n, None => return Err(Error::Exhausted("materialization progress")),
        };
        let retracted = match self.retracted.checked_add(counts.retracted) {
            Some(n) => n, None => return Err(Error::Exhausted("materialization progress")),
        };
        let rejected = match self.rejected.checked_add(counts.rejected) {
            Some(n) => n, None => return Err(Error::Exhausted("materialization progress")),
        };
        self.delivered = delivered; self.activated = activated;
        self.retracted = retracted; self.rejected = rejected;
        self.rebuilt = self.rebuilt || counts.rebuilt;
        Ok(())
    }
}

impl Default for Progress {
    fn default() -> (out:Self)
        ensures out.heads_before@.len() == 0, out.heads_after@.len() == 0, out.totals() == Totals::zero(),
    {
        Self { heads_before:Vec::new(), heads_after:Vec::new(), delivered:0, activated:0, retracted:0, rejected:0, rebuilt:false }
    }
}
impl Clone for Progress {
    fn clone(&self) -> (out:Self) ensures out.same(self),
    {
        Self { heads_before:self.heads_before.clone(), heads_after:self.heads_after.clone(), delivered:self.delivered,
            activated:self.activated, retracted:self.retracted, rejected:self.rejected, rebuilt:self.rebuilt }
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_in_each_counter_preserves_the_entire_report() {
        for index in 0..4 {
            let mut report = Progress {
                heads_before: vec![CommitHash([1; 32])],
                heads_after: vec![CommitHash([2; 32])],
                delivered: 3,
                activated: 5,
                retracted: 7,
                rejected: 11,
                rebuilt: false,
            };
            let counters = [
                &mut report.delivered,
                &mut report.activated,
                &mut report.retracted,
                &mut report.rejected,
            ];
            *counters.into_iter().nth(index).unwrap() = usize::MAX;
            let before = report.clone();
            let counts = Counts {
                delivered: 1,
                activated: 1,
                retracted: 1,
                rejected: 1,
                rebuilt: true,
            };
            assert!(matches!(
                report.accumulate(&counts),
                Err(Error::Exhausted("materialization progress"))
            ));
            assert_eq!(report, before);
        }
    }

    #[test]
    fn totals_include_every_counter_and_keep_rebuilds() {
        let mut report = Progress::default();
        // Heads alone do not report event activity.
        report.heads_after.push(CommitHash([1; 32]));
        assert!(!report.changed());
        report
            .accumulate(&Counts {
                delivered: 2,
                activated: 3,
                retracted: 5,
                rejected: 7,
                rebuilt: true,
            })
            .unwrap();
        report
            .accumulate(&Counts {
                delivered: 11,
                activated: 13,
                retracted: 17,
                rejected: 19,
                rebuilt: false,
            })
            .unwrap();
        assert_eq!(
            (
                report.delivered,
                report.activated,
                report.retracted,
                report.rejected
            ),
            (13, 16, 22, 26)
        );
        assert!(report.rebuilt && report.changed());
        report.delivered = usize::MAX;
        let before = report.clone();
        report.accumulate(&Counts::default()).unwrap();
        assert_eq!(report, before);
    }
}
