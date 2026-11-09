//! Public feedback completion, correction rounds and checkpoint publication.
use super::{
    api_error::Error,
    batch_protocol::Batch,
    ledger::Ledger,
    materialization_corrections as corrections,
    materialization_driver::Materialization,
    materialization_feedback::{self as feedback, Applied},
};
#[cfg(verus_keep_ghost)]
use super::{
    batch_protocol::{EventView, event_views},
    checkpoint::Checkpoint,
    commit_hash::CommitHash,
    history::History,
    history_data::PayloadId,
    materialization_feedback::Pending,
    materialization_source::Source,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Keep counter failures explicit instead of wrapping or panicking.
fn add_count(before: usize, added: usize, name: &'static str) -> (out: Result<usize, Error>)
    ensures match out {
        Ok(total) => total == before + added,
        Err(e) => e is Exhausted && before + added > usize::MAX,
    },
{
    match before.checked_add(added) { Some(total) => Ok(total), None => Err(Error::Exhausted(name)) }
}

pub(crate) open spec fn report_valid(driver: &Materialization, history: &History, batch: &Batch, applied: &Applied) -> bool {
    driver.protocol.accepts(batch)
        && feedback::valid_rejections(driver.protocol.pending_value().unwrap().delivered@, applied)
        && feedback::valid_wanted(history, applied)
}

pub(crate) open spec fn reported(before: &Pending, published: &Checkpoint, applied: &Applied, checkpoint: &Checkpoint) -> bool {
    checkpoint.follow_view().same(&published.follow_view())
        && checkpoint.rejected_view() == feedback::merged(before.next.rejected_view(), applied.rejected@)
        && checkpoint.cursor_view() == before.next.cursor_view()
        && checkpoint.progress_view() == before.next.progress_view()
}

pub(crate) open spec fn added_rejections(before: &Pending, applied: &Applied) -> nat {
    feedback::rejection_hashes(applied.rejected@).to_set().difference(before.next.rejected_view().dom()).len()
}

/// Exact local round, including the checkpoint snapshot used to select corrections.
pub(crate) open spec fn round(before: &Pending, published: &Checkpoint, source: Option<Source>, history: &History,
    applied: &Applied, supports: bool, after: &Pending, emitted: Seq<EventView<PayloadId>>, snapshot: &Checkpoint) -> bool {
    let affected = corrections::affected(history, snapshot, before.corrected@, before.active@);
    let reset = !supports && affected.len() > 0;
    &&& reported(before, published, applied, snapshot)
    &&& after.next.follow_view() == snapshot.follow_view()
    &&& after.next.rejected_view() == snapshot.rejected_view()
    &&& after.origin == before.origin && after.original_active == before.original_active
    &&& after.delivered@ == before.delivered@
    &&& after.counts.delivered == before.counts.delivered && after.counts.activated == before.counts.activated
    &&& after.counts.rejected == before.counts.rejected + added_rejections(before, applied)
    &&& after.counts.retracted == before.counts.retracted + if reset { 0nat } else { affected.len() }
    &&& after.counts.rebuilt == (before.counts.rebuilt || reset)
    &&& after.restart == (before.restart || reset)
    &&& if reset {
        &&& emitted == seq![EventView::Reset] && after.corrected@ == before.corrected@ && after.active@.len() == 0
        &&& source is Some && source.unwrap().ready()
        &&& after.next.cursor_view().delivery_view().len() == 0 && after.next.cursor_view().target_view().heads_view().len() == 0
        &&& after.next.progress_view() is Some && after.next.progress_view().unwrap().applied == 0
        &&& after.next.progress_view().unwrap().target.same(&source.unwrap().target())
        &&& after.next.progress_view().unwrap().delivery_heads@ == source.unwrap().delivery()
    } else {
        let requested = corrections::payload_hashes(emitted);
        &&& after.active@ == before.active@
        &&& after.corrected@ == before.corrected@.union(affected.to_set())
        &&& after.next.cursor_view() == snapshot.cursor_view() && after.next.progress_view() == snapshot.progress_view()
        &&& requested.to_set() == applied.wanted@.to_set() && requested.no_duplicates()
        &&& vstd::std_specs::btree::increasing_seq(requested)
        &&& emitted == corrections::retractions(affected) + corrections::payload_events(history, requested)
    }
}

pub(crate) proof fn round_no_activation(before: &Pending, published: &Checkpoint, source: Option<Source>, history: &History,
    applied: &Applied, supports: bool, after: &Pending, emitted: Seq<EventView<PayloadId>>, snapshot: &Checkpoint)
    requires round(before, published, source, history, applied, supports, after, emitted, snapshot),
    ensures forall|i: int| 0 <= i < emitted.len() ==> super::materialization_events::activation(emitted[i]) is None,
{
    let affected = corrections::affected(history, snapshot, before.corrected@, before.active@);
    assert forall|i: int| 0 <= i < emitted.len() implies super::materialization_events::activation(emitted[i]) is None by {
        if supports || affected.len() == 0 {
            if i < affected.len() { assert(emitted[i] == corrections::retractions(affected)[i]); }
            else { assert(emitted[i] == corrections::payload_events(history, corrections::payload_hashes(emitted))[i - affected.len()]); }
        }
    }
}

pub(crate) open spec fn body_error(before: &Materialization, history: &History, applied: &Applied, error: Error, snapshot: &Checkpoint) -> bool {
    let pending = before.protocol.pending_value().unwrap();
    let affected = corrections::affected(history, snapshot, pending.corrected@, pending.active@);
    let reset = !before.supports_visibility_changes && affected.len() > 0;
    reported(&pending, &before.checkpoint, applied, snapshot)
        && if pending.counts.rejected + added_rejections(&pending, applied) > usize::MAX { error is Exhausted }
        else if reset && (before.source is None || !before.source.unwrap().ready()) { error is Poisoned }
        else if pending.counts.retracted + (if reset { 0nat } else { affected.len() }) > usize::MAX { error is Exhausted }
        else { error is Exhausted && before.protocol.sequence() == u64::MAX
            && (reset || affected.len() > 0 || applied.wanted@.len() > 0) }
}

impl Materialization {
    pub closed spec fn completion_errors<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, batch: &Batch,
        applied: &Applied, out: Result<Option<Batch>, Error>) -> bool {
        self.completion_error_details(before, ledger, batch, applied, out)
    }
    pub(crate) open spec fn completion_error_details<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, batch: &Batch,
        applied: &Applied, out: Result<Option<Batch>, Error>) -> bool {
        let history = ledger.history_view();
        if !before.guard_valid(&history) {
            out is Err && before.guard_error(&history, out->Err_0) && *self == *before
        } else {
            &&& self.owner == Some(history.history_id())
            &&& if !report_valid(before, &history, batch, applied) {
                out is Err && self.same_except_owner(before) && match out->Err_0 {
                    Error::StaleBatch => !before.protocol.accepts(batch),
                    Error::Protocol(_) => before.protocol.accepts(batch)
                        && !feedback::valid_rejections(before.protocol.pending_value().unwrap().delivered@, applied),
                    Error::UnknownCommit(hash) => before.protocol.accepts(batch)
                        && feedback::valid_rejections(before.protocol.pending_value().unwrap().delivered@, applied)
                        && applied.wanted@.contains(hash) && !history.known(hash),
                    _ => false,
                }
            } else {
                match out {
                    Ok(_) => true,
                    Err(error) => self.protocol.pending_value() is None && !self.protocol.accepts(batch)
                        && self.protocol.sequence() == before.protocol.sequence()
                        && exists|snapshot: Checkpoint| body_error(before, &history, applied, error, &snapshot),
                }
            }
        }
    }

    pub closed spec fn completion_outcome<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, batch: &Batch,
        applied: &Applied, out: Result<Option<Batch>, Error>) -> bool {
        self.completion_details(before, ledger, batch, applied, out)
    }
    pub(crate) proof fn completion_facts<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, batch: &Batch,
        applied: &Applied, out: Result<Option<Batch>, Error>)
        requires self.completion_outcome(before, ledger, batch, applied, out), self.completion_errors(before, ledger, batch, applied, out),
        ensures self.completion_details(before, ledger, batch, applied, out), self.completion_error_details(before, ledger, batch, applied, out),
    {}

    pub(crate) open spec fn completion_details<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, batch: &Batch,
        applied: &Applied, out: Result<Option<Batch>, Error>) -> bool {
        let history = ledger.history_view();
        &&& self.dirty == before.dirty && self.poisoned == before.poisoned
        &&& self.supports_visibility_changes == before.supports_visibility_changes
        &&& self.protocol.planner_id() == before.protocol.planner_id()
        &&& match out {
            Err(_) => self.checkpoint == before.checkpoint && self.last == before.last && self.source == before.source,
            Ok(next) => before.guard_valid(&history) && report_valid(before, &history, batch, applied)
                && self.owner == Some(history.history_id()) && !self.protocol.accepts(batch)
                && (match next { Some(followup) => forall|i: int| 0 <= i < followup.events_view().len()
                    ==> super::materialization_events::activation(followup.events_view()[i]) is None, None => true })
                && exists|pending: Pending, emitted: Seq<EventView<PayloadId>>, snapshot: Checkpoint|
                    round(&before.protocol.pending_value().unwrap(), &before.checkpoint, before.source, &history,
                        applied, before.supports_visibility_changes, &pending, emitted, &snapshot)
                    && match next {
                        Some(followup) => emitted.len() > 0 && followup.events_view() == emitted
                            && self.protocol.accepts(&followup) && self.protocol.pending_value() == Some(pending)
                            && self.protocol.pending_events() == emitted && self.protocol.sequence() == before.protocol.sequence() + 1
                            && self.checkpoint == before.checkpoint && self.last == before.last && self.source == before.source,
                        None => emitted.len() == 0 && self.protocol.pending_value() is None
                            && self.protocol.sequence() == before.protocol.sequence()
                            && self.checkpoint == pending.next && self.last == pending.counts
                            && self.source == if pending.restart { None } else { before.source },
                    },
        }
    }

    /// Record a batch's effects, returning correction/payload follow-ups until
    /// the checkpoint is safe to save. The ledger supplies requested payload IDs
    /// and cascading rejection ancestry; no borrow survives this call.
    ///
    /// Invalid tokens or reports preserve pending work; a retained clone of the
    /// valid token can be used to retry a corrected report.
    /// Counter exhaustion is an error. Errors after report acceptance consume its
    /// token without publishing progress; restore state and checkpoint together
    /// before continuing after such an error.
    pub fn complete<Options, Queue>(&mut self, ledger: &Ledger<Options, Queue>, batch: Batch, applied: Applied)
        -> (out: Result<Option<Batch>, Error>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).counters_consistent() && (out matches Ok(None)) ==> final(self).completed_counters_match(old(self)),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).checkpoint_consistent() && (out matches Ok(None)) ==> final(self).completed_checkpoint_matches(old(self)),
            old(self).source_consistent() ==> final(self).source_consistent(),
            old(self).source_consistent() && (out matches Err(Error::Poisoned)) ==> old(self).poisoned_view(),
            final(self).completion_outcome(old(self), ledger, &batch, &applied, out),
            final(self).completion_errors(old(self), ledger, &batch, &applied, out),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self;
        let history = ledger.history();
        self.check_history(history, ledger.permissions())?;
        let (mut pending, report) = feedback::accept_report(&mut self.protocol, history, &batch, applied)?;
        let ghost original = pending;
        proof { self.checkpoint.representation(); pending.next.representation(); }
        pending.next.follow = self.checkpoint.follow.clone();
        proof { pending.next.representation(); }
        let added = report.record_rejections(&mut pending.next);
        let ghost snapshot = pending.next;
        assert(reported(&original, &before.checkpoint, &applied, &snapshot));
        pending.counts.rejected = match add_count(pending.counts.rejected, added, "materialization rejections") {
            Ok(count) => count,
            Err(error) => { assert(body_error(&before, history, &applied, error, &snapshot)); return Err(error); },
        };
        let correction = corrections::plan(history, &pending.next, &pending.active, &mut pending.corrected,
            &report.wanted, self.supports_visibility_changes);
        if correction.reset {
            let source = match &self.source { Some(source) => source, None => { assert(body_error(&before, history, &applied, Error::Poisoned, &snapshot)); return Err(Error::Poisoned); } };
            match pending.restart_from(source) {
                Ok(()) => {},
                Err(error) => { assert(body_error(&before, history, &applied, error, &snapshot)); return Err(error); },
            }
        }
        pending.counts.retracted = match add_count(pending.counts.retracted, correction.retracted, "materialization retractions") {
            Ok(count) => count,
            Err(error) => { assert(body_error(&before, history, &applied, error, &snapshot)); return Err(error); },
        };
        let emitted = correction.events;
        let ghost events = event_views(emitted@);
        assert(round(&original, &before.checkpoint, before.source, history, &applied, before.supports_visibility_changes, &pending, events, &snapshot));
        proof { round_no_activation(&original, &before.checkpoint, before.source, history, &applied,
            before.supports_visibility_changes, &pending, events, &snapshot); }
        proof {
            if before.checkpoint_consistent() {
                super::materialization_invariants::round_preserves_alignment(&original,&before.checkpoint,before.source.unwrap(),history,
                    &applied,before.supports_visibility_changes,&pending,events,&snapshot);
            }
        }
        proof {
            if before.counters_consistent() {
                super::materialization_invariants::round_preserves_counts(&original,&before.checkpoint,before.source.unwrap(),history,
                    &applied,before.supports_visibility_changes,&pending,events,&snapshot);
            }
        }
        proof { if before.counters_consistent() {
            super::materialization_invariants::correction_budget(&pending,&before.source.unwrap());
        } }
        let ghost completed = pending;
        if !emitted.is_empty() {
            proof {
                let affected = corrections::affected(history, &snapshot, original.corrected@, original.active@);
                if !correction.reset && affected.len() == 0 && applied.wanted@.len() == 0 {
                    let requests = corrections::payload_hashes(events);
                    if requests.len() > 0 { assert(requests.to_set().contains(requests[0])); }
                    assert(requests.len() == 0);
                    assert(events.len() == 0);
                }
            }
            let followup = match self.protocol.install(pending, emitted) {
                Ok(batch) => batch,
                Err(error) => {
                    assert(body_error(&before, history, &applied, error, &snapshot));
                    return Err(error);
                },
            };
            assert(!self.protocol.accepts(&batch));
            return Ok(Some(followup));
        }
        if pending.restart { self.source = None; }
        self.checkpoint = pending.next;
        self.last = pending.counts;
        Ok(None)
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, CommitHash, Event, Follow, FormatId, Rejection};

    fn fixture(supports: bool) -> (Ledger, Materialization, Batch, CommitHash, CommitHash) {
        let author = Author::from([1; 32]);
        let root = CommitHash::from([2; 32]);
        let child = CommitHash::from([3; 32]);
        let mut ledger = Ledger::new(FormatId::new("completion-test").unwrap(), author);
        ledger
            .history
            .topology
            .insert(root, author, &[], 10, 0)
            .unwrap();
        ledger
            .history
            .topology
            .insert(child, author, &[root], 11, 0)
            .unwrap();
        let mut driver = Materialization::new(Follow::Working, supports);
        let batch = driver.plan(&ledger, 10).unwrap().unwrap();
        (ledger, driver, batch, root, child)
    }

    // These fixtures deliberately inject extreme internal counter states.
    // They exercise the defensive returned-error branches instead of allocating usize::MAX records.
    fn replace_counts(
        driver: &mut Materialization,
        batch: Batch,
        rejected: usize,
        retracted: usize,
    ) -> Batch {
        let mut pending = driver.protocol.take(&batch).unwrap();
        pending.counts.rejected = rejected;
        pending.counts.retracted = retracted;
        driver
            .protocol
            .install(pending, batch.events().to_vec())
            .unwrap()
    }

    fn cascade(root: CommitHash) -> Applied {
        Applied {
            rejected: vec![Rejection {
                hash: root,
                reason: "bad root".into(),
                cascade: true,
            }],
            wanted: vec![],
        }
    }

    #[test]
    fn rejection_overflow_consumes_token_without_publishing() {
        let (ledger, mut driver, batch, root, _) = fixture(true);
        let batch = replace_counts(&mut driver, batch, usize::MAX, 0);
        let retry = batch.clone();
        let saved = driver.checkpoint().clone();
        let source_checkpoint = driver.source.as_ref().unwrap().checkpoint(&saved).unwrap();
        let result = driver.complete(&ledger, batch, Applied::default().reject(root, "bad"));
        assert_eq!(
            result.unwrap_err(),
            Error::Exhausted("materialization rejections")
        );
        assert_eq!(driver.checkpoint(), &saved);
        assert_eq!(
            driver.source.as_ref().unwrap().checkpoint(&saved).unwrap(),
            source_checkpoint
        );
        assert_eq!(driver.last.rejected, 0);
        assert!(!driver.protocol.has_pending());
        assert_eq!(
            driver
                .complete(&ledger, retry, Applied::default())
                .unwrap_err(),
            Error::StaleBatch
        );
    }

    #[test]
    fn retraction_overflow_does_not_publish_recorded_rejections() {
        let (ledger, mut driver, batch, root, _) = fixture(true);
        let batch = replace_counts(&mut driver, batch, 0, usize::MAX);
        let saved = driver.checkpoint().clone();
        assert_eq!(
            driver.complete(&ledger, batch, cascade(root)).unwrap_err(),
            Error::Exhausted("materialization retractions")
        );
        assert_eq!(driver.checkpoint(), &saved);
        assert_eq!(driver.last.retracted, 0);
        assert!(!driver.protocol.has_pending());
    }

    #[test]
    fn reset_without_source_is_a_returned_error() {
        let (ledger, mut driver, batch, root, _) = fixture(false);
        let saved = driver.checkpoint().clone();
        driver.source = None;
        assert_eq!(
            driver.complete(&ledger, batch, cascade(root)).unwrap_err(),
            Error::Poisoned
        );
        assert_eq!(driver.checkpoint(), &saved);
        assert!(!driver.protocol.has_pending());
        assert!(!driver.last.rebuilt);
    }

    #[test]
    fn zero_increments_allow_maximum_existing_counts() {
        let (ledger, mut driver, batch, _, child) = fixture(true);
        let batch = replace_counts(&mut driver, batch, usize::MAX, usize::MAX);
        assert!(
            driver
                .complete(&ledger, batch, Applied::default())
                .unwrap()
                .is_none()
        );
        assert_eq!(driver.heads(), &[child]);
        assert_eq!(driver.last.rejected, usize::MAX);
        assert_eq!(driver.last.retracted, usize::MAX);
        assert!(driver.is_current(&ledger));
    }

    #[test]
    fn repeated_reports_count_new_rejections_and_corrections_once() {
        let (ledger, mut driver, first, root, child) = fixture(true);
        let saved = driver.checkpoint().clone();
        let second = driver
            .complete(
                &ledger,
                first,
                Applied::default().reject(root, "bad").want(child),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            second.events(),
            &[Event::Payload {
                hash: child,
                payload: 11
            }]
        );
        assert_eq!(driver.protocol.check(&second).unwrap().counts.rejected, 1);
        let third = driver
            .complete(&ledger, second, cascade(root).want(child))
            .unwrap()
            .unwrap();
        assert_eq!(
            third.events(),
            &[
                Event::Retract { hash: child },
                Event::Payload {
                    hash: child,
                    payload: 11
                }
            ]
        );
        assert_eq!(driver.checkpoint(), &saved);
        let fourth = driver
            .complete(&ledger, third, cascade(root).want(child))
            .unwrap()
            .unwrap();
        assert_eq!(
            fourth.events(),
            &[Event::Payload {
                hash: child,
                payload: 11
            }]
        );
        assert_eq!(driver.checkpoint(), &saved);
        assert!(
            driver
                .complete(&ledger, fourth, cascade(root))
                .unwrap()
                .is_none()
        );
        assert_eq!(driver.last.delivered, 2);
        assert_eq!(driver.last.rejected, 1);
        assert_eq!(driver.last.retracted, 1);
        assert!(driver.is_current(&ledger));
    }
}
