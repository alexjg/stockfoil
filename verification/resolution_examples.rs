//! Checked callers compose public planning with the actual byte-resolution path.
use vstd::prelude::*;
use std::{sync::Arc, collections::HashMap};
use crate::{batch_protocol::{Batch, Event, EventView, Protocol}, document::Document,
    ledger::Ledger, materialization_driver::Materialization, materialization_events::activation,
    commit_hash::CommitHash, author::Author, payload_resolution::mapped};

verus! {

/// Resolution cannot activate anything that was inactive in the planned batch.
pub(crate) fn plan_resolve<C, O, Q>(document: &Document<C, O, Q>, driver: &mut Materialization, budget: usize) {
    let ghost before = *driver;
    let ledger = document.ledger();
    let result = driver.plan(ledger, budget);
    proof { driver.plan_facts(&before, ledger, budget, result); }
    if let Ok(Some(batch)) = result {
        let bytes = document.resolve_batch(&batch);
        assert(driver.protocol.accepts(&batch));
        if let Ok(events) = bytes {
            assert(events.len() == batch.events_view().len());
            assert forall|i: int| 0 <= i < events.len() implies
                activation(#[trigger] events[i].view()) == activation(batch.events_view()[i]) by {}
            assert forall|i: int| 0 <= i < events.len() && activation(events[i].view()) is Some implies
                driver.source.unwrap().after_members().contains(activation(#[trigger] events[i].view()).unwrap())
                && !crate::materialization_events::rejected(&before.checkpoint, &document.ledger_view().history_view(), activation(events[i].view()).unwrap()) by {
                assert(activation(events[i].view()) == activation(batch.events_view()[i]));
            }
        }
    }
}

/// A real nonempty payload table and a real issued token, including a hidden delivery.
pub(crate) fn mixed_bytes<O, Q>(ledger: Ledger<O, Q>, bytes: Arc<[u8]>) {
    let document = Document { ledger, codec: (), payloads: vec![bytes.clone()], index: HashMap::new(), staged: HashMap::new() };
    let hash = CommitHash([1; 32]); let parent = CommitHash([2; 32]); let author = Author([3; 32]);
    let input = vec![Event::Reset,
        Event::Deliver { hash, author, parents: vec![parent], payload: 0, active: false },
        Event::Activate { hash }, Event::Retract { hash }, Event::Payload { hash, payload: 0 }];
    let mut protocol = Protocol::new();
    let issued = protocol.install((), input);
    assert(issued is Ok);
    let batch = issued.unwrap();
    proof { document.representation(); }
    let resolved = document.resolve_batch(&batch);
    assert(resolved is Ok);
    let events = resolved.unwrap();
    assert(events.len() == 5);
    assert(events[0].view() is Reset);
    assert(document.resolves(batch.events_view()[1], events[1].view()));
    assert(batch.events_view()[1]->Deliver_parents =~= seq![parent]);
    assert(batch.events_view()[1] == EventView::Deliver { hash, author, parents: seq![parent], payload: 0, active: false });
    assert(events[1].view() == EventView::Deliver { hash, author, parents: seq![parent], payload: bytes, active: false });
    assert(events[2].view() == EventView::Activate { hash });
    assert(events[3].view() == EventView::Retract { hash });
    assert(events[4].view() == EventView::Payload { hash, payload: bytes });
    assert(protocol.accepts(&batch));
    let consumed = protocol.take(&batch);
    assert(consumed is Ok);
}

pub(crate) fn missing_bytes<O, Q>(ledger: Ledger<O, Q>) {
    let document = Document { ledger, codec: (), payloads: Vec::new(), index: HashMap::new(), staged: HashMap::new() };
    let mut protocol = Protocol::new();
    let issued = protocol.install((), vec![Event::Reset, Event::Payload { hash: CommitHash([4; 32]), payload: u64::MAX }]);
    assert(issued is Ok);
    let batch = issued.unwrap();
    proof { document.representation(); }
    let resolved = document.resolve_batch(&batch);
    assert(crate::payload_resolution::payload(batch.events_view()[1]) == Some(u64::MAX));
    assert(!document.resolvable(batch.events_view()));
    assert(resolved is Err && resolved->Err_0 is Storage);
    assert(protocol.accepts(&batch));
}

/// The public generic resolver preserves the token and the exact event sequence.
pub(crate) fn identity_resolution(batch: &Batch) {
    let events = batch.resolve(|id: u64| -> (out: u64) ensures out == id { id });
    assert forall|i: int| 0 <= i < events.len() implies #[trigger] events[i].view() == batch.events_view()[i] by {
        assert(mapped(batch.events_view()[i], events[i].view(), |p: u64, q: u64| p == q));
    }
}

/// Fallible mapping returns the original application error and never changes the envelope.
pub(crate) fn exact_error<E>(error: E) {
    let event = Event::Payload { hash: CommitHash([5; 32]), payload: 9u64 };
    let ghost original = error;
    let result = event.try_map_payload(|_id: u64| -> (out: Result<u8, E>) ensures out == Err(original) { Err(error) });
    assert(result == Err(original));
}

pub(crate) fn hidden_stays_hidden<P, Q>(hash: CommitHash, author: Author, parents: Vec<CommitHash>, payload: P, replacement: Q) {
    let event = Event::Deliver { hash, author, parents, payload, active: false };
    let mapped_event = event.map_payload(|_p: P| -> (out: Q) ensures out == replacement { replacement });
    assert(mapped_event.view() == EventView::Deliver { hash, author, parents: parents@, payload: replacement, active: false });
}

} // verus!
