//! Actual immutable public batch tokens and exclusive pending-work handshake.
use super::{
    api_error::Error, author::Author, commit_hash::CommitHash, history_data::PayloadId,
    identity::Identity,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    prelude::*,
    resource::map::{GhostMapAuth, GhostPersistentPointsTo},
};

verus! {

/// An incremental state-maintenance event emitted by a [`Materialization`](crate::Materialization).
///
/// `P` is the payload representation: [`Bytes`](crate::Bytes) when driving an in-memory
/// [`Materializer`](crate::Materializer), or [`PayloadId`] when planning [`Batch`]es against a
/// [`Ledger`](crate::Ledger).
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub enum Event<P> {
    /// Clear all materialized state in preparation for a clean replay of
    /// history.
    ///
    /// Emitted when `supports_visibility_changes` is `false` and a permission
    /// change, branch switch, or cascading payload rejection retracts an
    /// already-active commit.
    Reset,
    /// First delivery of a commit to this materialization.
    ///
    /// Every commit in the graph is delivered once, even when `active` is
    /// `false` (for example, when authored by a revoked writer or outside a
    /// pinned branch), because later active commits may reference its
    /// structural metadata. Record any causal metadata your CRDT needs, and
    /// apply user-visible effects only when `active` is `true`.
    Deliver {
        hash: CommitHash,
        author: Author,
        parents: Vec<CommitHash>,
        payload: P,
        active: bool,
    },
    /// A previously delivered commit has entered the active view (for example,
    /// after a permission grant or branch change). Only emitted when
    /// `supports_visibility_changes` is `true`.
    Activate { hash: CommitHash },
    /// A previously active commit has left the active view (for example, after
    /// a write-frontier revocation, branch change, or cascading payload
    /// rejection). Only emitted when `supports_visibility_changes` is `true`.
    Retract { hash: CommitHash },
    /// Delivers the payload of an older commit that the application requested
    /// via [`Applied::wanted`](crate::Applied::wanted) in the preceding batch round.
    Payload { hash: CommitHash, payload: P },
}

#[cfg(verus_keep_ghost)]
pub enum EventView<P> {
    Reset,
    Deliver { hash: CommitHash, author: Author, parents: Seq<CommitHash>, payload: P, active: bool },
    Activate { hash: CommitHash },
    Retract { hash: CommitHash },
    Payload { hash: CommitHash, payload: P },
}
impl<P> Event<P> {
    pub open spec fn view(&self) -> EventView<P> {
        match self {
            Self::Reset => EventView::Reset,
            Self::Deliver { hash, author, parents, payload, active } => EventView::Deliver {
                hash: *hash, author: *author, parents: parents@, payload: *payload, active: *active,
            },
            Self::Activate { hash } => EventView::Activate { hash: *hash },
            Self::Retract { hash } => EventView::Retract { hash: *hash },
            Self::Payload { hash, payload } => EventView::Payload { hash: *hash, payload: *payload },
        }
    }
}
pub open spec fn event_views<P>(events: Seq<Event<P>>) -> Seq<EventView<P>> {
    events.map(|i: int, e: Event<P>| e.view())
}

/// An immutable batch of events and its completion token.
///
/// Inspect [`events`](Self::events) or [`resolve`](Self::resolve) payload IDs
/// without consuming the token, then pass it to [`Materialization::complete`](crate::Materialization::complete).
/// Clones identify the same work; completing one invalidates all of them.
///
/// ```compile_fail
/// # use stockfoil::Batch;
/// fn alter_events(batch: &mut Batch) {
///     batch.events.clear(); // events cannot be changed while retaining the token
/// }
/// ```
///
/// ```compile_fail
/// # use stockfoil::Batch;
/// fn alter_rebuild(batch: &mut Batch) {
///     batch.rebuild = false;
/// }
/// ```
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct Batch {
    events: Vec<Event<PayloadId>>,
    rebuild: bool,
    planner: Identity,
    ticket: u64,
    #[cfg(verus_keep_ghost)] receipt: Tracked<GhostPersistentPointsTo<u64, Seq<EventView<PayloadId>>>>,
}

pub open spec fn has_reset(events: Seq<EventView<PayloadId>>) -> bool {
    exists|i: int| 0 <= i < events.len() && events[i] is Reset
}

proof fn reset_push(events: Seq<EventView<PayloadId>>, event: EventView<PayloadId>)
    ensures has_reset(events.push(event)) == (has_reset(events) || event is Reset),
{
    if has_reset(events) {
        let i = choose|i: int| 0 <= i < events.len() && events[i] is Reset;
        assert(events.push(event)[i] is Reset);
    } else if event is Reset {
        assert(events.push(event)[events.len() as int] is Reset);
    } else {
        assert forall|i: int| 0 <= i < events.push(event).len() implies !(events.push(event)[i] is Reset) by {};
    }
}

impl Batch {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        &&& self.planner.lineage() == self.receipt@.id()
        &&& 0 < self.ticket
        &&& self.ticket == self.receipt@.key()
        &&& event_views(self.events@) == self.receipt@.value()
        &&& self.rebuild == has_reset(event_views(self.events@))
    }
    pub closed spec fn events_view(&self) -> Seq<EventView<PayloadId>> { event_views(self.events@) }
    pub closed spec fn planner_id(&self) -> u64 { self.planner.raw() }
    pub closed spec fn ticket_id(&self) -> u64 { self.ticket }

    /// Planned events, in application order. The completion token always
    /// refers to exactly these events.
    pub fn events(&self) -> (out: &[Event<PayloadId>]) ensures event_views(out@) == self.events_view(), { &self.events }
    /// Whether this batch contains a rebuild event.
    pub fn is_rebuild(&self) -> (out: bool) ensures out == has_reset(self.events_view()),
    { proof { use_type_invariant(self); } self.rebuild }
    pub fn len(&self) -> (out: usize) ensures out == self.events_view().len(), { self.events.len() }
    pub fn is_empty(&self) -> (out: bool) ensures out == (self.events_view().len() == 0), { self.events.is_empty() }
}

pub(crate) fn clone_event(event: &Event<PayloadId>) -> (out: Event<PayloadId>) ensures out.view() == event.view(),
{
    match event {
        Event::Reset => Event::Reset,
        Event::Deliver { hash, author, parents, payload, active } => Event::Deliver {
            hash: *hash, author: *author, parents: parents.clone(), payload: *payload, active: *active,
        },
        Event::Activate { hash } => Event::Activate { hash: *hash },
        Event::Retract { hash } => Event::Retract { hash: *hash },
        Event::Payload { hash, payload } => Event::Payload { hash: *hash, payload: *payload },
    }
}

impl Clone for Batch {
    fn clone(&self) -> (out: Self)
        ensures out.events_view() == self.events_view(), out.planner_id() == self.planner_id(), out.ticket_id() == self.ticket_id(),
    {
        proof { use_type_invariant(self); }
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.events.len()
            invariant i <= self.events.len(), event_views(events@) == event_views(self.events@).take(i as int),
            decreases self.events.len() - i,
        {
            let ghost previous = events@;
            let event = clone_event(&self.events[i]);
            events.push(event);
            assert(event_views(events@) =~= event_views(previous).push(event.view()));
            assert(event_views(self.events@)[i as int] == event.view());
            assert(event_views(self.events@).take(i as int + 1) =~= event_views(self.events@).take(i as int).push(event.view()));
            i += 1;
            assert(event_views(events@) =~= event_views(self.events@).take(i as int));
        }
        assert(event_views(events@) =~= event_views(self.events@));
        Self { events, rebuild: self.rebuild, planner: self.planner.clone(), ticket: self.ticket,
            #[cfg(verus_keep_ghost)] receipt: Tracked(self.receipt.borrow().duplicate()) }
    }
}

/// Owns exactly one outstanding completion value. Issuance history is erased.
pub(crate) struct Protocol<P> {
    identity: Identity,
    ticket: u64,
    pending: Option<P>,
    #[cfg(verus_keep_ghost)] issued: Tracked<GhostMapAuth<u64, Seq<EventView<PayloadId>>>>,
}

impl<P> Protocol<P> {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        &&& self.identity.lineage() == self.issued@.id()
        &&& forall|t: u64| self.issued@@.contains_key(t) ==> t <= self.ticket
        &&& self.pending is Some ==> self.issued@@.contains_key(self.ticket)
    }
    pub closed spec fn planner_id(&self) -> u64 { self.identity.raw() }
    pub(crate) closed spec fn sequence(&self) -> u64 { self.ticket }
    pub(crate) closed spec fn pending_value(&self) -> Option<P> { self.pending }
    pub(crate) closed spec fn pending_events(&self) -> Seq<EventView<PayloadId>> { self.issued@@[self.ticket] }
    pub(crate) open spec fn accepts(&self, batch: &Batch) -> bool {
        self.pending_value() is Some && self.planner_id() == batch.planner_id() && self.sequence() == batch.ticket_id()
    }

    pub(crate) fn new() -> (out: Self)
        ensures out.pending_value() is None, out.sequence() == 0,
    {
        let tracked (issued, unused) = GhostMapAuth::<u64, Seq<EventView<PayloadId>>>::new(Map::empty());
        let identity = Identity::new(#[cfg(verus_keep_ghost)] { Ghost(issued.id()) });
        Self { identity, ticket: 0, pending: None, #[cfg(verus_keep_ghost)] issued: Tracked(issued) }
    }

    /// A new planner cannot adopt an already issued token, even with identical events.
    pub(crate) proof fn fresh_distinct(tracked &self, tracked batch: &Batch)
        requires self.sequence() == 0,
        ensures self.planner_id() != batch.planner_id(),
    {
        use_type_invariant(self); use_type_invariant(batch);
        self.identity.agreement(&batch.planner);
        if self.planner_id() == batch.planner_id() {
            batch.receipt.borrow().agree(self.issued.borrow());
        }
    }

    pub(crate) fn has_pending(&self) -> (out: bool) ensures out == (self.pending_value() is Some), { self.pending.is_some() }

    /// Refuse overwrite and exhaustion before altering pending work.
    pub(crate) fn install(&mut self, pending: P, events: Vec<Event<PayloadId>>) -> (out: Result<Batch, Error>)
        ensures final(self).planner_id() == old(self).planner_id(),
            match out {
                Err(e) => *final(self) == *old(self) &&
                    (if old(self).pending_value() is Some { e is Protocol } else { e is Exhausted && old(self).sequence() == u64::MAX }),
                Ok(batch) => old(self).pending_value() is None && old(self).sequence() < u64::MAX
                    && final(self).sequence() == old(self).sequence() + 1
                    && final(self).pending_value() == Some(pending)
                    && final(self).pending_events() == event_views(events@)
                    && batch.events_view() == event_views(events@) && final(self).accepts(&batch),
            },
    {
        if self.pending.is_some() { return Err(Error::Protocol("complete the outstanding batch before planning")); }
        if self.ticket == u64::MAX { return Err(Error::Exhausted("materialization tickets")); }
        proof { use_type_invariant(&*self); }
        let ticket = self.ticket + 1;
        let mut rebuild = false;
        let mut i = 0;
        while i < events.len()
            invariant i <= events.len(), rebuild == has_reset(event_views(events@).take(i as int)),
            decreases events.len() - i,
        {
            proof { reset_push(event_views(events@).take(i as int), events[i as int].view()); }
            if matches!(&events[i], Event::Reset) { rebuild = true; }
            i += 1;
            assert(event_views(events@).take(i as int) =~= event_views(events@).take(i as int - 1).push(events[i as int - 1].view()));
        }
        assert(event_views(events@).take(i as int) =~= event_views(events@));
        self.ticket = ticket;
        let tracked receipt = self.issued.borrow_mut().insert(ticket, event_views(events@)).persist();
        let batch = Batch { events, rebuild, planner: self.identity.clone(), ticket,
            #[cfg(verus_keep_ghost)] receipt: Tracked(receipt) };
        self.pending = Some(pending);
        Ok(batch)
    }

    pub(crate) fn check(&self, batch: &Batch) -> (out: Result<&P, Error>)
        ensures match out {
            Err(e) => e is StaleBatch && !self.accepts(batch),
            Ok(pending) => self.accepts(batch) && self.pending_value() == Some(*pending)
                && batch.events_view() == self.pending_events(),
        },
    {
        proof { use_type_invariant(self); use_type_invariant(batch); }
        let pending = match &self.pending { Some(p) => p, None => return Err(Error::StaleBatch) };
        if !self.identity.same(&batch.planner) || batch.ticket != self.ticket { return Err(Error::StaleBatch); }
        proof { batch.receipt.borrow().agree(self.issued.borrow()); }
        Ok(pending)
    }

    /// A successful take invalidates every copy of this ticket.
    pub(crate) fn take(&mut self, batch: &Batch) -> (out: Result<P, Error>)
        ensures final(self).planner_id() == old(self).planner_id(), final(self).sequence() == old(self).sequence(),
            match out {
                Err(e) => e is StaleBatch && !old(self).accepts(batch) && *final(self) == *old(self),
                Ok(pending) => old(self).accepts(batch) && old(self).pending_value() == Some(pending)
                    && batch.events_view() == old(self).pending_events() && final(self).pending_value() is None,
            },
    {
        match self.check(batch) { Err(e) => return Err(e), Ok(_) => {} }
        proof { use_type_invariant(&*self); }
        Ok(self.pending.take().unwrap())
    }
}

/// Concrete maximum-ticket witness; no assumption of a well-formed external caller.
#[cfg(verus_keep_ghost)]
pub(crate) fn exhausted_ticket_witness() {
    let mut protocol = Protocol::new();
    proof { use_type_invariant(&protocol); }
    protocol.ticket = u64::MAX - 1;
    let last = protocol.install(11u64, Vec::new()).unwrap();
    assert(last.ticket_id() == u64::MAX);
    let taken = protocol.take(&last).unwrap();
    assert(taken == 11);
    let ghost exhausted = protocol;
    let result = protocol.install(12u64, Vec::new());
    assert(result matches Err(Error::Exhausted(_)) && protocol == exhausted);
    let stale = protocol.take(&last);
    assert(stale is Err && protocol == exhausted);
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_ticket_completes_without_allowing_wraparound_or_overwrite() {
        let mut protocol = Protocol::new();
        protocol.ticket = u64::MAX - 1;
        let last = protocol.install(11u64, vec![Event::Reset]).unwrap();
        let copy = last.clone();
        assert_eq!(last.events(), copy.events());
        assert!(copy.is_rebuild());
        assert!(matches!(
            protocol.install(12, vec![]),
            Err(Error::Protocol(_))
        ));
        assert_eq!(*protocol.check(&copy).unwrap(), 11);
        assert_eq!(protocol.take(&last).unwrap(), 11);
        for _ in 0..2 {
            assert!(matches!(
                protocol.install(13, vec![]),
                Err(Error::Exhausted(_))
            ));
            assert!(matches!(protocol.take(&copy), Err(Error::StaleBatch)));
            assert!(!protocol.has_pending());
            assert_eq!(protocol.ticket, u64::MAX);
        }
    }
}
