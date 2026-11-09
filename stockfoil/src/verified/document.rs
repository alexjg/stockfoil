//! Actual in-memory storage, with checked reads independent of the codec.
use super::{
    api_error::Error,
    batch_protocol::{Batch, Event, clone_event},
    commit_hash::CommitHash,
    history::History,
    history_data::{ItemId, PayloadId},
    ledger::Ledger,
};
#[cfg(verus_keep_ghost)]
use super::{
    batch_protocol::{EventView, event_views},
    payload_resolution::{mapped, payload},
};
use std::{collections::HashMap, sync::Arc};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) struct Staged {
    pub(crate) members: Vec<Arc<[u8]>>,
    pub(crate) references: usize,
}

/// A ledger with canonical payloads in memory and a pure bundle codec.
///
/// Nothing does I/O or awaits: this is the small-document path. For storage
/// elsewhere use [`Ledger`] directly, assigning payload IDs as bytes stream
/// through caller-controlled buffers. Graph queries and advanced operations
/// remain available through [`ledger`](Self::ledger).
pub struct Document<C,
    #[cfg(not(verus_keep_ghost))] Options = crate::ReceiveOptions,
    #[cfg(verus_keep_ghost)] Options,
    #[cfg(not(verus_keep_ghost))] Queue = crate::Queue<crate::ItemId>,
    #[cfg(verus_keep_ghost)] Queue,
> {
    pub(crate) ledger: Ledger<Options, Queue>,
    pub(crate) codec: C,
    pub(crate) payloads: Vec<Arc<[u8]>>,
    pub(crate) index: HashMap<CommitHash, PayloadId>,
    pub(crate) staged: HashMap<ItemId, Staged>,
}

impl<C, Options, Queue> Document<C, Options, Queue> {
    pub(crate) proof fn representation(&self)
        ensures self.ledger_view() == self.ledger, self.payloads_view() == self.payloads@,
    {}
    pub closed spec fn ledger_view(&self) -> Ledger<Options, Queue> { self.ledger }
    pub closed spec fn payloads_view(&self) -> Seq<Arc<[u8]>> { self.payloads@ }
    pub open spec fn resolvable(&self, events: Seq<EventView<PayloadId>>) -> bool {
        forall|i: int| 0 <= i < events.len() ==> match payload(#[trigger] events[i]) {
            Some(id) => id < self.payloads_view().len(), None => true,
        }
    }
    pub open spec fn resolves(&self, before: EventView<PayloadId>, after: EventView<Arc<[u8]>>) -> bool {
        mapped(before, after, |id: PayloadId, bytes: Arc<[u8]>|
            id < self.payloads_view().len() && bytes == self.payloads_view()[id as int])
    }
    pub fn ledger(&self) -> (out: &Ledger<Options, Queue>)
        ensures *out == self.ledger_view(),
    { &self.ledger }
    pub fn history(&self) -> (out: &History)
        ensures *out == self.ledger_view().history_view(),
    { self.ledger.history() }

    /// Resolve the exact stored slot, rejecting IDs that cannot index it.
    pub(crate) fn payload_by_id(&self, id: PayloadId) -> (out: Result<Arc<[u8]>, Error>)
        ensures match out {
            Ok(bytes) => id < self.payloads_view().len() && bytes == self.payloads_view()[id as int],
            Err(e) => id >= self.payloads_view().len() && e is Storage,
        },
    {
        if id as u128 >= self.payloads.len() as u128 {
            return Err(Error::Storage("unknown in-memory payload ID".into()));
        }
        Ok(self.payloads[id as usize].clone())
    }

    /// Resolve the whole batch before handing any events to the application.
    pub(crate) fn resolve_batch(&self, batch: &Batch) -> (out: Result<Vec<Event<Arc<[u8]>>>, Error>)
        ensures match out {
            Ok(events) => self.resolvable(batch.events_view())
                && events.len() == batch.events_view().len()
                && forall|i: int| 0 <= i < events.len() ==> self.resolves(batch.events_view()[i], #[trigger] events[i].view()),
            Err(e) => !self.resolvable(batch.events_view()) && e is Storage,
        },
    {
        let events = batch.events();
        let mut resolved: Vec<Event<Arc<[u8]>>> = Vec::new();
        let mut i = 0;
        while i < events.len()
            invariant i <= events.len(), resolved.len() == i,
                event_views(events@) == batch.events_view(),
                forall|j: int| 0 <= j < i ==> match payload(#[trigger] batch.events_view()[j]) {
                    Some(id) => id < self.payloads_view().len(), None => true,
                },
                forall|j: int| 0 <= j < i ==> self.resolves(batch.events_view()[j], #[trigger] resolved[j].view()),
            decreases events.len() - i,
        {
            let event = clone_event(&events[i]);
            assert(event.view() == batch.events_view()[i as int]);
            let next = event.try_map_payload(|id: PayloadId| -> (out: Result<Arc<[u8]>, Error>)
                ensures match out {
                    Ok(bytes) => id < self.payloads_view().len() && bytes == self.payloads_view()[id as int],
                    Err(e) => id >= self.payloads_view().len() && e is Storage,
                },
            { self.payload_by_id(id) });
            match next {
                Err(e) => {
                    assert(payload(batch.events_view()[i as int]) is Some);
                    assert(payload(batch.events_view()[i as int]).unwrap() >= self.payloads_view().len());
                    assert(!self.resolvable(batch.events_view()));
                    return Err(e);
                },
                Ok(event) => { resolved.push(event); },
            }
            i += 1;
        }
        Ok(resolved)
    }
}

} // verus!
