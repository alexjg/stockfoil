//! Payload conversion preserves the authorization and structural event envelope.
use super::{
    batch_protocol::{Batch, Event, clone_event},
    history_data::PayloadId,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use {
    super::batch_protocol::{EventView, event_views},
    vstd::prelude::*,
};

verus! {

pub open spec fn payload<P>(event: EventView<P>) -> Option<P> {
    match event {
        EventView::Deliver { payload, .. } | EventView::Payload { payload, .. } => Some(payload),
        _ => None,
    }
}

/// The payload relation is arbitrary: metadata and visibility are independent of it.
pub open spec fn mapped<P, Q>(before: EventView<P>, after: EventView<Q>, relates: spec_fn(P, Q) -> bool) -> bool {
    match (before, after) {
        (EventView::Reset, EventView::Reset) => true,
        (EventView::Deliver { hash: h, author: a, parents: ps, payload: p, active: v },
         EventView::Deliver { hash: k, author: b, parents: qs, payload: q, active: w }) =>
            h == k && a == b && ps == qs && v == w && relates(p, q),
        (EventView::Activate { hash: h }, EventView::Activate { hash: k }) => h == k,
        (EventView::Retract { hash: h }, EventView::Retract { hash: k }) => h == k,
        (EventView::Payload { hash: h, payload: p }, EventView::Payload { hash: k, payload: q }) => h == k && relates(p, q),
        _ => false,
    }
}

impl<P> Event<P> {
    pub fn map_payload<Q>(self, f: impl FnOnce(P) -> Q) -> (out: Event<Q>)
        requires payload(self.view()) matches Some(p) ==> f.requires((p,)),
        ensures mapped(self.view(), out.view(), |p: P, q: Q| f.ensures((p,), q)),
    {
        match self {
            Self::Reset => Event::Reset,
            Self::Deliver { hash, author, parents, payload, active } => Event::Deliver {
                hash, author, parents, payload: f(payload), active,
            },
            Self::Activate { hash } => Event::Activate { hash },
            Self::Retract { hash } => Event::Retract { hash },
            Self::Payload { hash, payload } => Event::Payload { hash, payload: f(payload) },
        }
    }
    pub fn try_map_payload<Q, E>(self, f: impl FnOnce(P) -> Result<Q, E>) -> (out: Result<Event<Q>, E>)
        requires payload(self.view()) matches Some(p) ==> f.requires((p,)),
        ensures match out {
            Ok(event) => mapped(self.view(), event.view(), |p: P, q: Q| f.ensures((p,), Ok(q))),
            Err(e) => payload(self.view()) matches Some(p) && f.ensures((p,), Err(e)),
        },
    {
        Ok(match self {
            Self::Reset => Event::Reset,
            Self::Deliver { hash, author, parents, payload, active } => Event::Deliver {
                hash, author, parents, payload: f(payload)?, active,
            },
            Self::Activate { hash } => Event::Activate { hash },
            Self::Retract { hash } => Event::Retract { hash },
            Self::Payload { hash, payload } => Event::Payload { hash, payload: f(payload)? },
        })
    }
}

impl Batch {
    /// Resolve payloads in event order without consuming the completion token.
    pub fn resolve<P>(&self, mut f: impl FnMut(PayloadId) -> P) -> (out: Vec<Event<P>>)
        requires forall|i: int| 0 <= i < self.events_view().len()
            && payload(self.events_view()[i]) is Some ==> f.requires((payload(self.events_view()[i]).unwrap(),)),
        ensures out.len() == self.events_view().len(),
            forall|i: int| 0 <= i < out.len() ==> mapped(self.events_view()[i], #[trigger] out[i].view(), |p: PayloadId, q: P| f.ensures((p,), q)),
    {
        let ghost original_f = f;
        let events = self.events();
        let mut out: Vec<Event<P>> = Vec::new();
        let mut i = 0;
        while i < events.len()
            invariant i <= events.len(), out.len() == i,
                event_views(events@) == self.events_view(),
                forall|j: int| 0 <= j < self.events_view().len()
                    && payload(self.events_view()[j]) is Some ==> f.requires((payload(self.events_view()[j]).unwrap(),)),
                forall|p: PayloadId, q: P| f.ensures((p,), q) == original_f.ensures((p,), q),
                forall|j: int| 0 <= j < i ==> mapped(self.events_view()[j], #[trigger] out[j].view(), |p: PayloadId, q: P| f.ensures((p,), q)),
            decreases events.len() - i,
        {
            let event = clone_event(&events[i]);
            assert(event.view() == self.events_view()[i as int]);
            assert(payload(event.view()) matches Some(p) ==> f.requires((p,)));
            let mapped_event = match event {
                Event::Reset => Event::Reset,
                Event::Deliver { hash, author, parents, payload, active } => Event::Deliver {
                    hash, author, parents, payload: f(payload), active,
                },
                Event::Activate { hash } => Event::Activate { hash },
                Event::Retract { hash } => Event::Retract { hash },
                Event::Payload { hash, payload } => Event::Payload { hash, payload: f(payload) },
            };
            out.push(mapped_event);
            i += 1;
        }
        out
    }
}

} // verus!
