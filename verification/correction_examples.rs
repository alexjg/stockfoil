//! Real source/report/correction/restart/token composition, without application-state assumptions.
use vstd::prelude::*;
use crate::{api_error::Error, batch_protocol::{Protocol, event_views}, checkpoint::Checkpoint,
    history::History, commit_hash::CommitHash, materialization_corrections as corrections,
    materialization_events as events, materialization_feedback::{self as feedback, Applied, Pending},
    materialization_source::Source};

verus! {

pub(crate) fn source_report_correction(history: &History, source: &mut Source, checkpoint: &Checkpoint,
    budget: usize, applied: Applied, supports: bool) -> Result<(), Error>
{
    let emitted = source.advance(history, budget)?;
    let (planned, summary) = events::translate(history, checkpoint, &emitted.records, emitted.reset);
    proof { corrections::translated_active_facts(history, checkpoint, emitted.records@, emitted.reset); }
    assert(summary.active@.no_duplicates());
    let next = source.checkpoint(checkpoint).unwrap();
    let pending = Pending::new(next, summary);
    let mut protocol = Protocol::new();
    let first = protocol.install(pending, planned).unwrap();
    let (mut pending, report) = feedback::accept_report(&mut protocol, history, &first, applied)?;
    let _added = report.record_rejections(&mut pending.next);
    let ghost prior = pending;
    let correction = corrections::plan(history, &pending.next, &pending.active, &mut pending.corrected, &report.wanted, supports);
    if correction.reset {
        pending.restart_from(source).unwrap();
        assert(pending.active.len() == 0 && pending.restart && pending.next.progress_view().unwrap().applied == 0);
        assert(correction.retracted == 0 && event_views(correction.events@) == seq![crate::batch_protocol::EventView::Reset]);
    } else {
        proof { corrections::affected_facts(history, &prior.next, prior.corrected@, prior.active@); }
        let ghost retract = corrections::affected(history, &prior.next, prior.corrected@, prior.active@);
        assert(retract.no_duplicates());
        assert(retract.to_set().subset_of(prior.active@.to_set()));
        assert(event_views(correction.events@) == corrections::retractions(retract) + corrections::payload_events(history, correction.requested()));
        assert(correction.requested().no_duplicates() && correction.requested().to_set() == report.wanted@);
    }
    if !correction.events.is_empty() {
        let ghost issued = event_views(correction.events@);
        let second = protocol.install(pending, correction.events).unwrap();
        assert(second.events_view() == issued);
        let stale = protocol.check(&first);
        assert(stale is Err);
        let held = protocol.check(&second).unwrap();
        assert(*held == pending);
    } else {
        assert(!correction.reset && correction.retracted == 0);
        assert(protocol.pending_value() is None);
        // Final checkpoint/application-state publication is still outside this consumer.
    }
    Ok(())
}

/// Repeated payload requests may generate payloads again; the same rejection cannot retract twice.
pub(crate) fn no_repeated_retraction(history: &History, checkpoint: &Checkpoint, active: &[CommitHash],
    corrected: &mut std::collections::HashSet<CommitHash>, wanted: &std::collections::BTreeSet<CommitHash>)
    requires forall|h: CommitHash| wanted@.contains(h) ==> history.known(h),
{
    let ghost before = corrected@;
    let first = corrections::plan(history, checkpoint, active, corrected, wanted, true);
    assert(!first.reset);
    proof { corrections::remembered_is_quiet(history, checkpoint, before, active@); }
    let again = corrections::plan(history, checkpoint, active, corrected, wanted, true);
    assert(!again.reset && again.retracted == 0);
    assert(event_views(again.events@) == corrections::payload_events(history, again.requested()));
}

pub(crate) fn restart_preserves_report_and_capture(pending: &mut Pending, source: &Source) {
    let ghost before = *pending;
    match pending.restart_from(source) {
        Err(_) => { assert(*pending == before); },
        Ok(()) => {
            assert(pending.next.rejected_view() == before.next.rejected_view());
            assert(pending.delivered@ == before.delivered@);
            assert(pending.active.len() == 0 && pending.restart && pending.counts.rebuilt);
            assert(pending.next.progress_view().unwrap().delivery_heads@ == source.delivery());
            assert(pending.next.progress_view().unwrap().target.same(&source.target()));
        },
    }
}

pub(crate) fn nonempty_cascade_and_payload() {
    let root_hash = CommitHash([0; 32]);
    let child_hash = CommitHash([1; 32]);
    assert(root_hash.0[0] != child_hash.0[0]);
    assert(root_hash != child_hash);
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, root_hash, crate::author::Author([0; 32]), &vec![]);
    let parents = vec![root_hash];
    let ghost resolved = parents@.map(|i: int, h: CommitHash| topology.hashes()[h]);
    assert(resolved =~= seq![root]);
    proof { crate::topology_examples::singleton_parent(root); }
    let child = crate::topology_examples::add(&mut topology, child_hash, crate::author::Author([1; 32]), &parents);
    assert(topology.graph()[1] =~= ISet::empty().insert(0int));
    let mut history = History::new();
    history.topology = topology;
    proof {
        history.representation(); history.topology.permission_facts();
        crate::graph::unfold_reaches(history.graph_view(), 0, 0);
        crate::graph::unfold_reaches(history.graph_view(), 0, 1);
    }
    assert(history.strict_ancestor(root_hash, child_hash));
    let mut checkpoint = Checkpoint::default();
    checkpoint.reject(root_hash, true);
    let active = vec![root_hash, child_hash];
    let mut corrected = std::collections::HashSet::new();
    let mut wanted = std::collections::BTreeSet::new();
    proof { crate::stdlib::commit_order_model(); }
    wanted.insert(child_hash);
    assert(corrections::needs_correction(&history, &checkpoint, corrected@, child_hash));
    assert(!corrections::needs_correction(&history, &checkpoint, corrected@, root_hash));
    proof { reveal_with_fuel(corrections::affected, 3); }
    assert(corrections::affected(&history, &checkpoint, corrected@, active@) =~= seq![child_hash]);
    let ghost original_corrected = corrected@;
    let first = corrections::plan(&history, &checkpoint, &active, &mut corrected, &wanted, true);
    assert(!first.reset && first.retracted == 1);
    assert(wanted@ =~= Set::empty().insert(child_hash));
    proof { first.requested().unique_seq_to_set(); first.requested().to_set_ensures(); }
    assert(first.requested().len() == 1);
    assert(first.requested().to_set().contains(first.requested()[0]));
    assert(first.requested()[0] == child_hash);
    assert(first.requested() =~= seq![child_hash]);
    assert(event_views(first.events@) == corrections::retractions(corrections::affected(&history, &checkpoint, original_corrected, active@)) + corrections::payload_events(&history, first.requested()));
    assert(event_views(first.events@) =~= seq![crate::batch_protocol::EventView::Retract { hash: child_hash }, crate::batch_protocol::EventView::Payload { hash: child_hash, payload: 42 }]);
    assert(first.events.len() == 2);
    assert(first.events[0].view() == crate::batch_protocol::EventView::Retract { hash: child_hash });
    assert(first.events[1].view() == crate::batch_protocol::EventView::Payload { hash: child_hash, payload: 42 });
    let again = corrections::plan(&history, &checkpoint, &active, &mut corrected, &wanted, true);
    assert(!again.reset && again.retracted == 0);
    let mut uncorrected = std::collections::HashSet::new();
    let reset = corrections::plan(&history, &checkpoint, &active, &mut uncorrected, &wanted, false);
    assert(reset.reset && reset.retracted == 0 && uncorrected@.is_empty());
    assert(event_views(reset.events@) == seq![crate::batch_protocol::EventView::Reset]);
}

} // verus!
