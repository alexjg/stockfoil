//! Canonical lazy enumeration of unions of chain-clock differences.
//! Shared by materialization changes and ancestry queries.
use super::pending::PendingChain;
use super::schedule_queues::Queues;
use super::tree::Clock;
use super::{clock::NodeIdx, history::History};
use std::collections::BTreeMap;

#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Traversal {
    pending: BTreeMap<NodeIdx, PendingChain>,
    queues: Queues,
    remaining: usize,
}

impl Traversal {
    pub(crate) closed spec fn valid(&self, h: &History) -> bool {
        super::traversal_step::valid(&h.topology, self.pending@, &self.queues, self.remaining)
    }
    pub(crate) closed spec fn selected(&self, h: &History) -> Set<NodeIdx> { super::pending::selected_set(&h.topology, self.pending@) }
    pub(crate) closed spec fn count(&self) -> usize { self.remaining }
    pub(crate) closed spec fn eligible(&self, h: &History, n: NodeIdx) -> bool { super::pending::eligible(&h.topology, self.pending@, n) }
    pub(crate) proof fn facts(&self, h: &History)
        requires h.topology.valid(), self.valid(h),
        ensures self.count() == self.selected(h).len(),
            forall|n: NodeIdx| self.selected(h).contains(n) ==> n@ < h.topology.nodes_view().len(),
            forall|n: NodeIdx| self.eligible(h, n) == (self.selected(h).contains(n) && forall|a: NodeIdx| self.selected(h).contains(a) && a != n ==> !crate::graph::reaches(h.topology.graph(), a@ as int, n@ as int)),
    {
        super::pending::selection_facts(&h.topology, self.pending@);
        assert forall|n: NodeIdx| self.selected(h).contains(n) implies n@ < h.topology.nodes_view().len() by {
            let f = choose|f: NodeIdx| self.pending@.contains_key(f) && self.pending@[f].selected(&h.topology, n);
            self.pending@[f].facts(&h.topology);
        }
    }
    /// Lift a retained cursor using the History's maintained lineage receipt.
    pub(crate) proof fn extend(&self, before: &History, after: &History)
        requires before.topology.valid(), after.topology.valid(), self.valid(before),
            after.topology.read_state().extends(before.topology.read_state()),
        ensures self.valid(after), self.selected(after) == self.selected(before),
            super::commits::project(&after.topology, self.selected(after)) == super::commits::project(&before.topology, self.selected(before)),
            forall|n: NodeIdx| self.eligible(after, n) == self.eligible(before, n),
    {
        super::traversal_extension::scheduler(&before.topology, &after.topology, self.pending@, &self.queues, self.remaining);
        super::traversal_extension::selection(&before.topology, &after.topology, self.pending@);
        assert forall|n: NodeIdx| self.eligible(after, n) == self.eligible(before, n) by {
            super::traversal_extension::eligibility(&before.topology, &after.topology, self.pending@, n);
        }
        let nodes = self.selected(before);
        assert forall|h: super::commit_hash::CommitHash| super::commits::project(&after.topology, nodes).contains(h)
            == super::commits::project(&before.topology, nodes).contains(h) by {
            if super::commits::project(&after.topology, nodes).contains(h) {
                let n = choose|n: NodeIdx| nodes.contains(n) && after.topology.nodes_view()[n@ as int].hash == h;
                super::traversal_extension::selected_hash(&before.topology, &after.topology, self.pending@, n);
                assert(before.topology.nodes_view()[n@ as int].hash == h);
                assert(super::commits::project(&before.topology, nodes).contains(h));
            }
            if super::commits::project(&before.topology, nodes).contains(h) {
                let n = choose|n: NodeIdx| nodes.contains(n) && before.topology.nodes_view()[n@ as int].hash == h;
                super::traversal_extension::selected_hash(&before.topology, &after.topology, self.pending@, n);
                assert(after.topology.nodes_view()[n@ as int].hash == h);
                assert(super::commits::project(&after.topology, nodes).contains(h));
            }
        }
        assert(super::commits::project(&after.topology, nodes) =~= super::commits::project(&before.topology, nodes));
    }

    /// Select the union of `after - before` for each pair. Initialization
    /// retains intervals, never a list of all selected commits.
    pub(crate) fn new(history: &History, differences: &[(&Clock, &Clock)]) -> (out: Self)
        requires super::traversal_init::inputs(history.topology.tables_view(), differences@),
        ensures out.valid(history), forall|n: NodeIdx| out.selected(history).contains(n) == super::traversal_init::requested(&history.topology, differences@, n),
    {
        let (pending, queues, remaining) =
            super::traversal_init::new(&history.topology, differences);
        proof { history.topology.established(); super::pending::selection_facts(&history.topology, pending@); }
        Self {
            pending,
            queues,
            remaining,
        }
    }


}

impl Traversal {
    /// Records remaining in this traversal, without expanding its intervals.
    pub(crate) fn remaining(&self) -> (out: usize)
        ensures out == self.count(),
    {
        self.remaining
    }

    pub(crate) fn next(&mut self, history: &History) -> (out: Option<NodeIdx>)
        requires old(self).valid(history),
        ensures final(self).valid(history), (out is None) == (old(self).count() == 0),
            match out {
                None => final(self).selected(history) == old(self).selected(history) && final(self).count() == old(self).count(),
                Some(n) => n@ < history.topology.nodes_view().len() && old(self).eligible(history, n)
                    && final(self).selected(history) == old(self).selected(history).remove(n)
                    && final(self).count() + 1 == old(self).count()
                    && forall|a: NodeIdx| old(self).eligible(history, a) ==> a@ < history.topology.nodes_view().len()
                        && super::stdlib::ready_le((history.topology.nodes_view()[n@ as int].hash, n), (history.topology.nodes_view()[a@ as int].hash, a)),
            },
    {
        super::traversal_step::next(
            &history.topology,
            &mut self.pending,
            &mut self.queues,
            &mut self.remaining,
        )
    }
}

} // verus!

#[cfg(test)]
impl Traversal {
    #[cfg(test)]
    pub(crate) fn retained_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.pending.len(),
            self.pending
                .values()
                .map(|c| c.intervals.interval_count())
                .sum(),
            self.queues.retained_counts().0,
            self.queues.retained_counts().1,
        )
    }
}
