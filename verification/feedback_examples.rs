//! Real report admission, source-to-token composition and durable rejection updates.
use vstd::prelude::*;
use crate::{api_error::Error, batch_protocol::{Batch, Protocol}, checkpoint::Checkpoint,
    history::History, materialization_feedback::{self as feedback, Applied, Pending, Validated, Rejection},
    materialization_source::Source, materialization_events, commit_hash::CommitHash};

verus! {

pub(crate) fn arbitrary_report(protocol: &mut Protocol<Pending>, history: &History, batch: &Batch, applied: Applied) {
    let ghost before = *protocol;
    let ghost should_accept = before.accepts(batch)
        && feedback::valid_rejections(before.pending_value().unwrap().delivered@, &applied)
        && feedback::valid_wanted(history, &applied);
    let result = feedback::accept_report(protocol, history, batch, applied);
    assert((result is Ok) == should_accept);
    match result {
        Err(_) => { assert(*protocol == before); },
        Ok((mut pending, report)) => {
            assert(Some(pending) == before.pending_value());
            assert(report.matches(&applied));
            let ghost base = pending.next;
            let added = report.record_rejections(&mut pending.next);
            assert(pending.next.rejected_view() == feedback::merged(base.rejected_view(), applied.rejected@));
            assert(added <= applied.rejected.len());
            assert(pending.next.follow_view() == base.follow_view() && pending.next.cursor_view() == base.cursor_view()
                && pending.next.progress_view() == base.progress_view());
            let repeated = protocol.check(batch);
            assert(repeated is Err);
        },
    }
}

/// Bad application feedback must leave the exact pending value available for a corrected retry.
pub(crate) fn malformed_report_then_retry(history: &History, pending: Pending, applied: Applied) {
    let mut protocol = Protocol::new();
    let batch = protocol.install(pending, Vec::new()).unwrap();
    let ghost waiting = protocol;
    if let Err(_) = feedback::accept_report(&mut protocol, history, &batch, applied) {
        assert(protocol == waiting);
        let (retried, report) = feedback::accept_report(&mut protocol, history, &batch, Applied::default()).unwrap();
        assert(retried == pending && report.rejected.len() == 0 && report.wanted@.is_empty());
        let again = protocol.check(&batch);
        assert(again is Err);
    }
}

/// The real emission/summary/constructor chain supplies the report's permitted delivery set.
pub(crate) fn source_to_report(history: &History, source: &mut Source, checkpoint: &Checkpoint,
    budget: usize, applied: Applied) -> Result<(), Error>
{
    let emitted = source.advance(history, budget)?;
    let (events, summary) = materialization_events::translate(history, checkpoint, &emitted.records, emitted.reset);
    let ghost expected_delivered = materialization_events::deliveries(crate::batch_protocol::event_views(events@));
    let next = source.checkpoint(checkpoint).unwrap();
    let pending = Pending::new(next, summary);
    assert(pending.delivered@ == expected_delivered);
    let mut protocol = Protocol::new();
    let batch = protocol.install(pending, events).unwrap();
    let ghost waiting = protocol;
    match feedback::accept_report(&mut protocol, history, &batch, applied) {
        Err(error) => { assert(protocol == waiting); Err(error) },
        Ok((mut accepted, report)) => {
            assert(feedback::rejection_hashes(report.rejected@).to_set().subset_of(expected_delivered));
            assert(feedback::rejection_hashes(report.rejected@).no_duplicates());
            assert(report.matches(&applied));
            let added = report.record_rejections(&mut accepted.next);
            assert(added == feedback::rejection_hashes(applied.rejected@).to_set().difference(next.rejected_view().dom()).len());
            Ok(())
        },
    }
}

pub(crate) fn nonempty_dedup_and_rejection_merge() {
    let hash = CommitHash([3; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let _node = crate::topology_examples::add(&mut topology, hash, crate::author::Author([1; 32]), &vec![]);
    history.topology = topology;
    proof { history.representation(); }
    let delivered = std::collections::HashSet::new();
    let requested = Applied::default().want(hash).want(hash);
    assert(requested.wanted@ == seq![hash, hash]);
    assert(requested.wanted@.to_set() =~= Set::empty().insert(hash));
    let report = feedback::validate(&history, &delivered, requested).unwrap();
    assert(report.wanted@ == Set::empty().insert(hash) && report.rejected.len() == 0);
    let mut checkpoint = Checkpoint::default();
    let report = Validated { rejected: vec![
        Rejection { hash, reason: String::new(), cascade: false },
        Rejection { hash, reason: String::new(), cascade: true },
        Rejection { hash, reason: String::new(), cascade: false },
    ], wanted: std::collections::BTreeSet::new() };
    assert(feedback::rejection_hashes(report.rejected@) =~= seq![hash, hash, hash]);
    assert(feedback::rejection_hashes(report.rejected@).to_set() =~= Set::empty().insert(hash));
    assert(feedback::rejection_hashes(report.rejected@).to_set().difference(checkpoint.rejected_view().dom()) =~= Set::empty().insert(hash));
    assert(report.rejected[1].hash == hash && report.rejected[1].cascade);
    assert(feedback::cascade_for(report.rejected@, hash));
    let added = report.record_rejections(&mut checkpoint);
    assert(added == 1 && checkpoint.rejected_view()[hash]);
    assert(feedback::rejection_hashes(report.rejected@).to_set().difference(checkpoint.rejected_view().dom()) =~= Set::<CommitHash>::empty());
    let again = report.record_rejections(&mut checkpoint);
    assert(again == 0 && checkpoint.rejected_view()[hash]);
}

} // verus!
