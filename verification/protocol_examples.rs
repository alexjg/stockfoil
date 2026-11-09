//! Production immutable tokens and handshake, with arbitrary typed inputs.
use vstd::prelude::*;
use crate::{batch_protocol::{self, Batch, Event, Protocol}, history_data::PayloadId};

verus! {

pub(crate) fn arbitrary_consumption<P>(protocol: &mut Protocol<P>, batch: &Batch)
    ensures protocol_result(old(protocol), final(protocol), batch),
{
    let ghost original = *protocol;
    let result = protocol.take(batch);
    match result {
        Err(e) => { assert(e is StaleBatch && *protocol == original); },
        Ok(_) => {
            let pending = protocol.has_pending();
            assert(!pending);
            let again = protocol.check(batch);
            assert(again is Err);
        },
    }
}
pub(crate) open spec fn protocol_result<P>(before: &Protocol<P>, after: &Protocol<P>, batch: &Batch) -> bool {
    if before.accepts(batch) { after.pending_value() is None } else { *before == *after }
}

pub(crate) fn cloned_token_and_follow_up<P>(pending: P, follow_up: P,
    events: Vec<Event<PayloadId>>, next_events: Vec<Event<PayloadId>>)
{
    let mut protocol = Protocol::new();
    let batch = protocol.install(pending, events).unwrap();
    let clone = batch.clone();
    assert(clone.events_view() == batch.events_view());
    let reference = protocol.check(&clone).unwrap();
    assert(*reference == pending);
    let ghost waiting = protocol;
    let overwrite = protocol.install(follow_up, next_events);
    assert(overwrite is Err && protocol == waiting);
    let completed = protocol.take(&clone).unwrap();
    assert(completed == pending);
    let stale = protocol.check(&batch);
    assert(stale is Err);
}

pub(crate) fn follow_up_invalidates_old(events: Vec<Event<PayloadId>>, next_events: Vec<Event<PayloadId>>) {
    let mut protocol = Protocol::new();
    let first = protocol.install(17u64, events).unwrap();
    let clone = first.clone();
    let taken = protocol.take(&first).unwrap();
    assert(taken == 17);
    let second = protocol.install(29u64, next_events).unwrap();
    let ghost current = protocol;
    let stale = protocol.take(&clone);
    assert(stale is Err && protocol == current);
    let reference = protocol.check(&second).unwrap();
    assert(*reference == 29u64);
    let done = protocol.take(&second).unwrap();
    let pending = protocol.has_pending();
    assert(done == 29 && !pending);
}

pub(crate) fn other_planner_rejects(batch: &Batch, events: Vec<Event<PayloadId>>) {
    let mut other = Protocol::new();
    proof { other.fresh_distinct(batch); }
    let own = other.install(7u64, events).unwrap();
    let ghost waiting = other;
    let wrong = other.take(batch);
    assert(wrong is Err && other == waiting);
    let completed = other.take(&own).unwrap();
    assert(completed == 7);
}

pub(crate) fn public_batch_observation(batch: &Batch) {
    let events = batch.events();
    let length = batch.len();
    let empty = batch.is_empty();
    let rebuild = batch.is_rebuild();
    assert(batch_protocol::event_views(events@) == batch.events_view());
    assert(length == events.len() && empty == (length == 0));
    assert(rebuild == batch_protocol::has_reset(batch.events_view()));
    let clone = batch.clone();
    assert(clone.events_view() == batch.events_view());
    let cloned_rebuild = clone.is_rebuild();
    assert(cloned_rebuild == rebuild);
}

pub(crate) fn nonempty_batch_roundtrip() {
    let hash = crate::commit_hash::CommitHash([7; 32]);
    let author = crate::author::Author([9; 32]);
    let mut events = Vec::new();
    events.push(Event::Reset);
    events.push(Event::Deliver { hash, author, parents: Vec::new(), payload: 42, active: false });
    events.push(Event::Activate { hash });
    events.push(Event::Retract { hash });
    events.push(Event::Payload { hash, payload: 77 });
    let mut protocol = Protocol::new();
    let batch = protocol.install(83u64, events).unwrap();
    let clone = batch.clone();
    let count = clone.len();
    let rebuild = clone.is_rebuild();
    assert(clone.events_view()[0] is Reset);
    assert(count == 5 && rebuild);
    assert(clone.events_view()[1] == batch_protocol::EventView::Deliver {
        hash, author, parents: Seq::empty(), payload: 42, active: false,
    });
    let consumed = protocol.take(&clone).unwrap();
    assert(consumed == 83);
    let again = protocol.take(&batch);
    assert(again is Err);
}

} // verus!
