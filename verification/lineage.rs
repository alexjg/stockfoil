//! Owned append-only lineage and duplicable receipts for actual captured states.
//! All storage here is tracked proof state, erased from the production binary.
use vstd::prelude::*;
use vstd::resource::{Loc, map::{GhostMapAuth, GhostPersistentPointsTo}};
use crate::capture_refinement::ReadState;

verus! {

pub(crate) tracked struct Receipt {
    point: GhostPersistentPointsTo<nat, ReadState>,
}

impl Receipt {
    pub(crate) closed spec fn id(&self) -> Loc { self.point.id() }
    pub(crate) closed spec fn state(&self) -> ReadState { self.point.value() }

    pub(crate) proof fn duplicate(tracked &self) -> (tracked out: Self)
        ensures out.id() == self.id(), out.state() == self.state(),
    { Self { point: self.point.duplicate() } }
}

pub(crate) tracked struct Lineage {
    states: GhostMapAuth<nat, ReadState>,
    latest: Receipt,
}

impl Lineage {
    pub(crate) closed spec fn id(&self) -> Loc { self.states.id() }
    pub(crate) closed spec fn valid(&self, current: ReadState) -> bool {
        &&& self.latest.id() == self.id()
        &&& self.latest.state() == current
        &&& forall|n: nat| self.states@.contains_key(n) ==> n <= current.labels.len()
            && current.extends(self.states@[n])
    }

    pub(crate) proof fn new(state: ReadState) -> (tracked out: Self)
        ensures out.valid(state),
    {
        state.reflexive();
        let tracked (mut states, unused) = GhostMapAuth::new(Map::<nat, ReadState>::empty());
        let tracked point = states.insert(state.labels.len(), state).persist();
        Self { states, latest: Receipt { point } }
    }

    pub(crate) proof fn advance(tracked &mut self, before: ReadState, after: ReadState)
        requires old(self).valid(before), after.extends(before),
            after.labels.len() > before.labels.len(),
        ensures final(self).valid(after), final(self).id() == old(self).id(),
    {
        assert forall|n: nat| self.states@.contains_key(n) implies
            n <= after.labels.len() && after.extends(self.states@[n]) by {
            after.transitive(before, self.states@[n]);
        }
        after.reflexive();
        let tracked point = self.states.insert(after.labels.len(), after).persist();
        self.latest = Receipt { point };
    }

    pub(crate) proof fn snapshot(tracked &self, state: ReadState) -> (tracked out: Receipt)
        requires self.valid(state),
        ensures out.id() == self.id(), out.state() == state,
    { self.latest.duplicate() }

    pub(crate) proof fn compatible(tracked &self, tracked receipt: &Receipt, current: ReadState)
        requires self.valid(current), receipt.id() == self.id(),
        ensures current.extends(receipt.state()),
    {
        receipt.point.agree(&self.states);
        assert(self.states@.contains_key(receipt.point.key()));
    }
}

} // verus!
