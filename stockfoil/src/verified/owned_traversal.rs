//! Owned materialization cursor with a maintained origin and growth-safe stepping.
use super::{
    api_error::Error, clock::NodeIdx, history::History, identity::Identity, traversal::Traversal,
    tree::Clock,
};
#[cfg(verus_keep_ghost)]
use super::{commit_hash::CommitHash, commits};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct OwnedTraversal {
    identity: Identity,
    // A failed/unwinding transition leaves a valid exhausted cursor.
    traversal: Option<Traversal>,
    #[cfg(verus_keep_ghost)] origin: Ghost<History>,
    #[cfg(verus_keep_ghost)] receipt: Tracked<crate::lineage::Receipt>,
}

impl OwnedTraversal {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        &&& self.origin@.topology.valid()
        &&& self.identity.lineage() == self.receipt@.id()
        &&& self.receipt@.id() == self.origin@.topology.lineage_id()
        &&& self.receipt@.state() == self.origin@.topology.read_state()
        &&& self.traversal is Some ==> self.traversal.unwrap().valid(&self.origin@)
    }

    pub(crate) closed spec fn history_id(&self) -> u64 { self.identity.raw() }
    pub(crate) closed spec fn lineage_id(&self) -> vstd::resource::Loc { self.identity.lineage() }
    pub(crate) closed spec fn count(&self) -> usize { match self.traversal { None => 0, Some(t) => t.count() } }
    closed spec fn selected(&self) -> Set<NodeIdx> { match self.traversal { None => Set::empty(), Some(t) => t.selected(&self.origin@) } }
    pub(crate) closed spec fn remaining_hashes(&self) -> Set<CommitHash> { commits::project(&self.origin@.topology, self.selected()) }

    pub(crate) open spec fn eligible(&self, history: &History, hash: CommitHash) -> bool {
        self.remaining_hashes().contains(hash) && forall|a: CommitHash| self.remaining_hashes().contains(a) && a != hash
            ==> !history.strict_ancestor(a, hash)
    }

    pub(crate) proof fn facts(tracked &self)
        ensures self.count() == self.remaining_hashes().len(),
    {
        use_type_invariant(self);
        if let Some(cursor) = &self.traversal { cursor.facts(&self.origin@); }
        commits::projection(&self.origin@.topology, self.selected());
    }

    pub(crate) fn new(history: &History, differences: &[(&Clock, &Clock)]) -> (out: Self)
        requires super::traversal_init::inputs(history.topology.tables_view(), differences@),
        ensures out.history_id() == history.history_id(),
            forall|h: CommitHash| out.remaining_hashes().contains(h) == (history.topology.hashes().contains_key(h)
                && super::traversal_init::requested(&history.topology, differences@, history.topology.hashes()[h])),
    {
        let traversal = Traversal::new(history, differences);
        let identity = history.topology.identity().clone();
        proof {
            history.topology.established(); history.representation(); traversal.facts(history);
            commits::projection(&history.topology, traversal.selected(history));
        }
        Self { identity, traversal: Some(traversal),
            #[cfg(verus_keep_ghost)] origin: Ghost(*history),
            #[cfg(verus_keep_ghost)] receipt: Tracked(history.topology.snapshot()) }
    }

    pub(crate) fn check_history(&self, history: &History) -> (out: Result<(), Error>)
        ensures match out {
            Ok(()) => self.history_id() == history.history_id() && self.lineage_id() == history.topology.lineage_id(),
            Err(e) => e is WrongHistory && self.history_id() != history.history_id(),
        },
    {
        proof { history.topology.established(); history.representation(); }
        if self.identity.same(history.topology.identity()) { Ok(()) } else { Err(Error::WrongHistory) }
    }

    pub(crate) fn remaining(&self) -> (out: usize)
        ensures out == self.count() == self.remaining_hashes().len(),
    {
        proof { self.facts(); }
        match &self.traversal { None => 0, Some(cursor) => cursor.remaining() }
    }

    /// No caller validity assumption: the numeric guard and persistent receipt
    /// justify resuming against any later state of the same History.
    pub(crate) fn next(&mut self, history: &History) -> (out: Result<Option<NodeIdx>, Error>)
        ensures final(self).history_id() == old(self).history_id(),
            match out {
                Err(e) => e is WrongHistory && old(self).history_id() != history.history_id() && *final(self) == *old(self),
                Ok(None) => old(self).history_id() == history.history_id()
                    && old(self).remaining_hashes().is_empty() && final(self).remaining_hashes().is_empty()
                    && final(self).count() == 0,
                Ok(Some(n)) => old(self).history_id() == history.history_id()
                    && n@ < history.topology.nodes_view().len()
                    && old(self).remaining_hashes().contains(history.topology.nodes_view()[n@ as int].hash)
                    && final(self).remaining_hashes() == old(self).remaining_hashes().remove(history.topology.nodes_view()[n@ as int].hash)
                    && final(self).count() + 1 == old(self).count()
                    && old(self).eligible(history, history.topology.nodes_view()[n@ as int].hash)
                    && (forall|h: CommitHash| old(self).eligible(history, h) ==>
                        history.topology.nodes_view()[n@ as int].hash == h
                        || super::stdlib::hash_less(history.topology.nodes_view()[n@ as int].hash, h)),
            },
    {
        match self.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
        proof {
            use_type_invariant(&*self); history.topology.established(); self.facts();
            history.topology.compatible(self.receipt.borrow());
            if let Some(cursor) = &self.traversal { cursor.extend(&self.origin@, history); cursor.facts(history); }
        }
        let ghost previous = *self;
        let ghost nodes = self.selected();
        let identity = self.identity.clone();
        let mut traversal = match self.traversal.take() { None => return Ok(None), Some(cursor) => cursor };
        let next = traversal.next(history);
        proof {
            if let Some(n) = next {
                commits::projection_remove(&history.topology, nodes, n);
                commits::eligibility(history, nodes, n);
                commits::projection(&history.topology, nodes);
                history.topology.hash_facts();
                let hash = history.topology.nodes_view()[n@ as int].hash;
                assert(previous.eligible(history, hash));
                assert forall|h: CommitHash| previous.eligible(history, h) implies hash == h || super::stdlib::hash_less(hash, h) by {
                    let a = history.topology.hashes()[h];
                    assert(nodes.contains(a));
                    assert(history.topology.nodes_view()[a@ as int].hash == h);
                    commits::eligibility(history, nodes, a);
                    assert(previous.traversal.unwrap().eligible(history, a));
                }
            }
        }
        *self = Self { identity, traversal: Some(traversal),
            #[cfg(verus_keep_ghost)] origin: Ghost(*history),
            #[cfg(verus_keep_ghost)] receipt: Tracked(history.topology.snapshot()) };
        Ok(next)
    }
}

} // verus!

#[cfg(test)]
impl OwnedTraversal {
    pub(crate) fn retained_counts(&self) -> (usize, usize, usize, usize) {
        self.traversal
            .as_ref()
            .map_or((0, 0, 0, 0), Traversal::retained_counts)
    }
}
