//! Checked consumers of the production source, translator, checkpoint and batch token.
use vstd::prelude::*;
use crate::{api_error::Error, batch_protocol::{Batch, Protocol}, checkpoint::Checkpoint,
    history::History, materialization_source::Source, materialization_events::{self as events, activation}};

verus! {

/// No assumed records or authorization answers: use the actual captured source.
pub(crate) fn source_to_batch(source: &mut Source, history: &History, published: &Checkpoint,
    protocol: &mut Protocol<Checkpoint>, budget: usize) -> (out: Result<Batch, Error>)
    ensures match out {
        Err(_) => true,
        Ok(batch) => forall|i: int| #![trigger batch.events_view()[i]] 0 <= i < batch.events_view().len() ==> {
            let event = batch.events_view()[i];
            activation(event) is Some ==> old(source).after_members().contains(activation(event).unwrap())
                && !events::rejected(published, history, activation(event).unwrap())
        },
    },
{
    let ghost saved = *source;
    let batch = source.advance(history, budget)?;
    let (planned, summary) = events::translate(history, published, &batch.records, batch.reset);
    proof { events::translation_authorized(history, published, batch.records@, saved.after_members()); }
    assert(summary.describes(crate::batch_protocol::event_views(planned@)));
    let next = source.checkpoint(published).unwrap();
    let out = protocol.install(next, planned)?;
    assert forall|i: int| #![trigger out.events_view()[i]] 0 <= i < out.events_view().len() implies {
        let event = out.events_view()[i];
        activation(event) is Some ==> saved.after_members().contains(activation(event).unwrap())
            && !events::rejected(published, history, activation(event).unwrap())
    } by {
        let offset = if batch.reset { 1int } else { 0int };
        if i >= offset {
            assert(out.events_view()[i] == events::translated(history, published, batch.records@)[i - offset]);
        }
    }
    Ok(out)
}

pub(crate) fn rejection_semantics(history: &History, checkpoint: &mut Checkpoint,
    root: crate::commit_hash::CommitHash, descendant: crate::commit_hash::CommitHash) {
    checkpoint.reject(root, true);
    let rejected = events::is_rejected(checkpoint, history, &root);
    assert(rejected);
    if history.is_ancestor(&root, &descendant) {
        let rejected = events::is_rejected(checkpoint, history, &descendant);
        assert(rejected);
    }
    checkpoint.reject(root, false);
    if history.is_ancestor(&root, &descendant) {
        let rejected = events::is_rejected(checkpoint, history, &descendant);
        assert(rejected);
    }
}

/// Concrete translator witness covering every visibility label, independent of the sequence spec.
pub(crate) fn nonempty_visibility_and_rejection() {
    use crate::{change_plan::Change, events::Vis, commit_hash::CommitHash};
    let hash = CommitHash([7; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let _node = crate::topology_examples::add(&mut topology, hash, crate::author::Author([3; 32]), &vec![]);
    history.topology = topology;
    proof { history.representation(); }
    let records = vec![
        Change { hash, payload: 17, newly_delivered: true, visibility: Vis::Before },
        Change { hash, payload: 18, newly_delivered: true, visibility: Vis::After },
        Change { hash, payload: 19, newly_delivered: true, visibility: Vis::Both },
        Change { hash, payload: 20, newly_delivered: true, visibility: Vis::Neither },
        Change { hash, payload: 21, newly_delivered: false, visibility: Vis::Before },
        Change { hash, payload: 22, newly_delivered: false, visibility: Vis::After },
        Change { hash, payload: 23, newly_delivered: false, visibility: Vis::Both },
        Change { hash, payload: 24, newly_delivered: false, visibility: Vis::Neither },
    ];
    let mut checkpoint = Checkpoint::default();
    let (planned, summary) = events::translate(&history, &checkpoint, &records, false);
    proof { reveal_with_fuel(events::translated, 9); reveal_with_fuel(events::count, 8);
        reveal_with_fuel(events::active_deliveries, 8); reveal_with_fuel(events::deliveries, 8); }
    assert(planned.len() == 6);
    assert(planned[0].view() is Deliver && !planned[0].view()->Deliver_active && planned[0].view()->Deliver_payload == 17);
    assert(planned[1].view() is Deliver && planned[1].view()->Deliver_active && planned[1].view()->Deliver_payload == 18);
    assert(planned[2].view() is Deliver && planned[2].view()->Deliver_active && planned[2].view()->Deliver_payload == 19);
    assert(planned[3].view() is Deliver && !planned[3].view()->Deliver_active && planned[3].view()->Deliver_payload == 20);
    assert(planned[4].view() is Retract && planned[5].view() is Activate);
    assert(summary.counts.delivered == 4 && summary.counts.activated == 1 && summary.counts.retracted == 1 && !summary.counts.rebuilt);
    assert(summary.active@ == seq![hash, hash] && summary.delivered@ == Set::empty().insert(hash));
    checkpoint.reject(hash, false);
    let (rejected, rejected_summary) = events::translate(&history, &checkpoint, &records, true);
    assert(rejected.len() == 5 && rejected[0].view() is Reset);
    assert(!rejected[1].view()->Deliver_active && !rejected[2].view()->Deliver_active
        && !rejected[3].view()->Deliver_active && !rejected[4].view()->Deliver_active);
    assert(rejected_summary.active.len() == 0 && rejected_summary.counts.delivered == 4
        && rejected_summary.counts.activated == 0 && rejected_summary.counts.retracted == 0 && rejected_summary.counts.rebuilt);
}

} // verus!
