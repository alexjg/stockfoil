//! Production frontier selection and dependency scheduling over stored topology.
use super::{
    chain_intervals::Intervals, clock::NodeIdx, schedule_queues::Queues, stdlib::Earlier,
    topology::Topology, tree::ChainId,
};
#[cfg(verus_keep_ghost)]
use crate::graph;
use std::collections::BTreeMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Captured positions on one chain; later appends cannot expand the selection.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct PendingChain {
    pub(crate) chain: ChainId,
    pub(crate) intervals: Intervals,
}

impl PendingChain {
    pub(crate) open spec fn valid<P>(&self, topology: &Topology<P>) -> bool {
        self.chain < topology.tables_view().len() && self.intervals.view().len() > 0
            && super::chain_intervals::canonical(self.intervals.view())
            && self.intervals.bounded(topology.tables_view()[self.chain as int].len())
    }

    pub(crate) open spec fn frontier<P>(&self, topology: &Topology<P>) -> NodeIdx {
        topology.tables_view()[self.chain as int][self.intervals.view()[0].start as int]
    }

    pub(crate) open spec fn selected<P>(&self, topology: &Topology<P>, node: NodeIdx) -> bool {
        exists|p: int| self.intervals.contains(p)
            && topology.tables_view()[self.chain as int][p] == node
    }

    pub(crate) proof fn facts<P>(&self, topology: &Topology<P>)
        requires topology.valid(), self.valid(topology),
        ensures self.selected(topology, self.frontier(topology)),
            self.frontier(topology)@ < topology.nodes_view().len(),
            topology.nodes_view()[self.frontier(topology)@ as int].chain == self.chain,
            forall|n: NodeIdx| self.selected(topology, n) ==>
                n@ < topology.nodes_view().len()
                && topology.nodes_view()[n@ as int].chain == self.chain
                && self.frontier(topology)@ <= n@
                && graph::reaches(topology.graph(), self.frontier(topology)@ as int, n@ as int),
    {
        topology.traversal_facts();
        let first = self.intervals.view()[0].start as int;
        assert(self.intervals.contains(first));
        assert forall|n: NodeIdx| self.selected(topology, n) implies
            n@ < topology.nodes_view().len()
            && topology.nodes_view()[n@ as int].chain == self.chain
            && self.frontier(topology)@ <= n@
            && graph::reaches(topology.graph(), self.frontier(topology)@ as int, n@ as int) by {
            let p = choose|p: int| self.intervals.contains(p)
                && topology.tables_view()[self.chain as int][p] == n;
            let r = choose|r: int| 0 <= r < self.intervals.view().len()
                && #[trigger] self.intervals.view()[r].start <= p < self.intervals.view()[r].end;
            if r > 0 { assert(self.intervals.view()[0].end < self.intervals.view()[r].start); }
            assert(first <= p);
            assert(topology.tables_view()[self.chain as int][p] == n);
            if first < p { assert(self.frontier(topology)@ <= n@); }
        }
    }

    proof fn labels<P>(&self, topology: &Topology<P>, n: NodeIdx)
        requires topology.valid(), self.valid(topology),
        ensures self.selected(topology, self.frontier(topology)),
            self.frontier(topology)@ < topology.nodes_view().len(),
            topology.nodes_view()[self.frontier(topology)@ as int].chain == self.chain,
            self.selected(topology, n) ==> n@ < topology.nodes_view().len()
                && topology.nodes_view()[n@ as int].chain == self.chain,
    { self.facts(topology); }

    pub(crate) fn first<P>(&self, topology: &Topology<P>) -> (out: NodeIdx)
        requires self.valid(topology),
        ensures out == self.frontier(topology),
    {
        topology.chain_nodes()[self.chain as usize][self.intervals.first()]
    }

    /// Consume exactly the current frontier, retaining all other selected positions.
    pub(crate) fn advance<P>(self, _topology: &Topology<P>) -> (out: Option<Self>)
        requires self.valid(_topology),
        ensures match out {
            None => (forall|n: NodeIdx| self.selected(_topology, n) ==> n == self.frontier(_topology))
                && super::chain_intervals::weight(self.intervals.view()) == 1,
            Some(next) => next.valid(_topology) && next.chain == self.chain
                && next.frontier(_topology)@ > self.frontier(_topology)@
                && (forall|n: NodeIdx| next.selected(_topology, n) ==
                    (self.selected(_topology, n) && n != self.frontier(_topology)))
                && super::chain_intervals::weight(next.intervals.view()) == super::chain_intervals::weight(self.intervals.view()) - 1,
        },
    {
        proof { _topology.established(); _topology.traversal_facts(); self.facts(_topology); }
        let ghost before = self;
        let next = Self { chain: self.chain, intervals: self.intervals.advance() };
        proof {
            next.intervals.facts();
            assert forall|n: NodeIdx| next.selected(_topology, n) ==
                (before.selected(_topology, n) && n != before.frontier(_topology)) by {
                let row = _topology.tables_view()[before.chain as int]@;
                let first = before.intervals.view()[0].start as int;
                if next.selected(_topology, n) {
                    let p = choose|p: int| next.intervals.contains(p) && row[p] == n;
                    assert(before.intervals.contains(p));
                    assert(before.selected(_topology, n));
                    if p < first { assert(row[p]@ < row[first]@); }
                    if first < p { assert(row[first]@ < row[p]@); }
                }
                if before.selected(_topology, n) && n != before.frontier(_topology) {
                    let p = choose|p: int| before.intervals.contains(p) && row[p] == n;
                    assert(p != first);
                    assert(next.intervals.contains(p));
                    assert(next.selected(_topology, n));
                }
            }
        }
        if next.intervals.is_empty() {
            assert(super::chain_intervals::weight(next.intervals.view()) == 0);
            None
        } else {
            proof {
                next.facts(_topology);
                let n = next.frontier(_topology);
                assert(before.selected(_topology, n) && n != before.frontier(_topology));
                if n@ == before.frontier(_topology)@ { NodeIdx::same_index(n, before.frontier(_topology)); }
            }
            Some(next)
        }
    }

    /// Wait on the last selected ancestor, avoiding repeated intermediate wakeups.
    pub(crate) fn last_before<P>(&self, topology: &Topology<P>, tip: NodeIdx) -> (out: NodeIdx)
        requires self.valid(topology), self.frontier(topology)@ <= tip@,
        ensures self.selected(topology, out), out@ <= tip@,
            forall|n: NodeIdx| self.selected(topology, n) && n@ <= tip@ ==> n@ <= out@,
    {
        proof { topology.established(); topology.traversal_facts(); }
        let nodes = &topology.chain_nodes()[self.chain as usize];
        let out = self.intervals.last_node_before(nodes, tip);
        assert forall|n: NodeIdx| self.selected(topology, n) && n@ <= tip@ implies n@ <= out@ by {
            let p = choose|p: int| self.intervals.contains(p) && nodes@[p] == n;
            assert(nodes[p]@ <= out@);
        }
        out
    }
}

pub(crate) open spec fn valid<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>) -> bool {
    &&& forall|f: NodeIdx| pending.contains_key(f) ==> pending[f].valid(topology)
        && pending[f].frontier(topology) == f
    &&& forall|f: NodeIdx, g: NodeIdx| pending.contains_key(f) && pending.contains_key(g)
        && pending[f].chain == pending[g].chain ==> f == g
}

pub(crate) open spec fn selected<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>, n: NodeIdx) -> bool {
    exists|f: NodeIdx| pending.contains_key(f) && pending[f].selected(topology, n)
}

/// A finite selection independent of the executable remaining counter.
pub(crate) open spec fn selected_set<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>) -> Set<NodeIdx> {
    topology.hashes().values().filter(|n: NodeIdx| selected(topology, pending, n))
}

pub(crate) proof fn selection_facts<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>)
    requires topology.valid(), valid(topology, pending),
    ensures forall|n: NodeIdx| selected_set(topology, pending).contains(n) == selected(topology, pending, n),
        forall|f: NodeIdx| pending.contains_key(f) ==> selected_set(topology, pending).contains(f),
        selected_set(topology, pending).is_empty() == pending.dom().is_empty(),
{
    topology.traversal_facts();
    assert forall|n: NodeIdx| selected_set(topology, pending).contains(n) == selected(topology, pending, n) by {
        if selected(topology, pending, n) {
            let f = choose|f: NodeIdx| pending.contains_key(f) && pending[f].selected(topology, n);
            pending[f].facts(topology);
            let h = topology.nodes_view()[n@ as int].hash;
            NodeIdx::same_index(topology.hashes()[h], n);
            assert(topology.hashes().values().contains(n));
        }
    }
    assert forall|f: NodeIdx| pending.contains_key(f) implies selected_set(topology, pending).contains(f) by {
        pending[f].facts(topology);
        assert(selected(topology, pending, f));
    }
    if pending.dom().is_empty() {
        assert(selected_set(topology, pending).is_empty()) by {
            if !selected_set(topology, pending).is_empty() {
                let n = selected_set(topology, pending).choose();
                let f = choose|f: NodeIdx| pending.contains_key(f) && pending[f].selected(topology, n);
            }
        }
    }
}

/// Advance the actual pending map, never overwriting another chain's frontier.
pub(crate) fn advance<P>(topology: &Topology<P>, pending: &mut BTreeMap<NodeIdx, PendingChain>, idx: NodeIdx) -> (out: Option<NodeIdx>)
    requires valid(topology, old(pending)@), old(pending)@.contains_key(idx),
    ensures valid(topology, final(pending)@),
        selected_set(topology, final(pending)@) == selected_set(topology, old(pending)@).remove(idx),
        selected_set(topology, final(pending)@).len() + 1 == selected_set(topology, old(pending)@).len(),
        forall|n: NodeIdx| selected(topology, final(pending)@, n) == (selected(topology, old(pending)@, n) && n != idx),
        forall|f: NodeIdx| old(pending)@.contains_key(f) && f != idx ==>
            final(pending)@.contains_key(f) && final(pending)@[f] == old(pending)@[f],
        match out {
            None => final(pending)@ == old(pending)@.remove(idx),
            Some(next) => next@ > idx@ && !old(pending)@.contains_key(next)
                && final(pending)@.dom() == old(pending)@.dom().remove(idx).insert(next)
                && final(pending)@[next].chain == old(pending)@[idx].chain,
        },
{
    proof { topology.established(); super::stdlib::node_order_model(); }
    let ghost before = pending@;
    let chain = pending.remove(&idx).expect("ready frontier");
    let mut out = None;
    if let Some(next_chain) = chain.advance(topology) {
        let next = next_chain.first(topology);
        proof {
            next_chain.facts(topology);
            if before.contains_key(next) {
                before[next].facts(topology);
                assert(before[next].chain == next_chain.chain);
                assert(next == idx);
            }
        }
        pending.insert(next, next_chain);
        out = Some(next);
    }
    proof { advance_update(topology, before, pending@, idx, out); }
    out
}

proof fn advance_update<P>(topology: &Topology<P>, before: Map<NodeIdx, PendingChain>, after: Map<NodeIdx, PendingChain>, idx: NodeIdx, out: Option<NodeIdx>)
    requires topology.valid(), valid(topology, before), before.contains_key(idx),
        match out {
            None => after == before.remove(idx)
                && forall|n: NodeIdx| before[idx].selected(topology, n) ==> n == idx,
            Some(next) => next@ > idx@ && !before.contains_key(next) && after.contains_key(next)
                && after == before.remove(idx).insert(next, after[next])
                && after[next].valid(topology) && after[next].frontier(topology) == next
                && after[next].chain == before[idx].chain
                && forall|n: NodeIdx| after[next].selected(topology, n) == (before[idx].selected(topology, n) && n != idx),
        },
    ensures valid(topology, after),
        selected_set(topology, after) == selected_set(topology, before).remove(idx),
        selected_set(topology, after).len() + 1 == selected_set(topology, before).len(),
        forall|n: NodeIdx| selected(topology, after, n) == (selected(topology, before, n) && n != idx),
{
    advance_valid(topology, before, after, idx, out);
    assert forall|n: NodeIdx| selected(topology, after, n) == (selected(topology, before, n) && n != idx) by {
        advance_member(topology, before, after, idx, out, n);
    }
    selection_facts(topology, before); selection_facts(topology, after);
    assert forall|n: NodeIdx| selected_set(topology, after).contains(n) == selected_set(topology, before).remove(idx).contains(n) by {
        assert(selected(topology, after, n) == (selected(topology, before, n) && n != idx));
    }
    assert(selected_set(topology, after) =~= selected_set(topology, before).remove(idx));
    assert(selected_set(topology, before).contains(idx));

}

proof fn advance_valid<P>(topology: &Topology<P>, before: Map<NodeIdx, PendingChain>, after: Map<NodeIdx, PendingChain>, idx: NodeIdx, out: Option<NodeIdx>)
    requires topology.valid(), valid(topology, before), before.contains_key(idx),
        match out {
            None => after == before.remove(idx)
                && forall|n: NodeIdx| before[idx].selected(topology, n) ==> n == idx,
            Some(next) => next@ > idx@ && !before.contains_key(next) && after.contains_key(next)
                && after == before.remove(idx).insert(next, after[next])
                && after[next].valid(topology) && after[next].frontier(topology) == next
                && after[next].chain == before[idx].chain
                && forall|n: NodeIdx| after[next].selected(topology, n) == (before[idx].selected(topology, n) && n != idx),
        },
    ensures valid(topology, after),
{
    assert forall|f: NodeIdx| after.contains_key(f) implies
        after[f].valid(topology) && after[f].frontier(topology) == f by {
        if before.contains_key(f) { assert(f != idx); }
    }
    assert forall|f: NodeIdx, g: NodeIdx| after.contains_key(f) && after.contains_key(g)
        && after[f].chain == after[g].chain implies f == g by {
        if out is Some {
            let next = out.unwrap();
            if f == next && g != next { assert(g == idx); }
            if g == next && f != next { assert(f == idx); }
        }
    }

}

proof fn advance_member<P>(topology: &Topology<P>, before: Map<NodeIdx, PendingChain>, after: Map<NodeIdx, PendingChain>, idx: NodeIdx, out: Option<NodeIdx>, n: NodeIdx)
    requires topology.valid(), valid(topology, before), before.contains_key(idx),
        match out {
            None => after == before.remove(idx)
                && forall|n: NodeIdx| before[idx].selected(topology, n) ==> n == idx,
            Some(next) => next@ > idx@ && !before.contains_key(next) && after.contains_key(next)
                && after == before.remove(idx).insert(next, after[next])
                && after[next].valid(topology) && after[next].frontier(topology) == next
                && after[next].chain == before[idx].chain
                && forall|n: NodeIdx| after[next].selected(topology, n) == (before[idx].selected(topology, n) && n != idx),
        },
    ensures selected(topology, after, n) == (selected(topology, before, n) && n != idx),
{
    if selected(topology, after, n) {
        let f = choose|f: NodeIdx| after.contains_key(f) && after[f].selected(topology, n);
        if before.contains_key(f) {
            after[f].labels(topology, n); before[idx].labels(topology, idx);
            if n == idx { assert(before[f].chain == before[idx].chain); assert(f == idx); }
            assert(selected(topology, before, n));
        } else {
            assert(out == Some(f));
            assert(before[idx].selected(topology, n));
            assert(n != idx);
            assert(selected(topology, before, n));
        }
    }
    if selected(topology, before, n) && n != idx {
        let f = choose|f: NodeIdx| before.contains_key(f) && before[f].selected(topology, n);
        if f != idx { assert(after.contains_key(f)); assert(after[f].selected(topology, n)); }
        else {
            assert(out is Some);
            assert(after.contains_key(out.unwrap()));
            assert(after[out.unwrap()].selected(topology, n));
            assert(selected(topology, after, n));
        }
    }

}

pub(crate) open spec fn eligible<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>, n: NodeIdx) -> bool {
    selected(topology, pending, n) && forall|a: NodeIdx| selected(topology, pending, a) && a != n
        ==> !graph::reaches(topology.graph(), a@ as int, n@ as int)
}

/// Testing only earlier chain frontiers is equivalent to testing every selected ancestor.
pub(crate) proof fn frontier_test<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>, idx: NodeIdx)
    requires topology.valid(), valid(topology, pending), pending.contains_key(idx),
        forall|f: NodeIdx| pending.contains_key(f) && f@ < idx@ ==>
            !graph::reaches(topology.graph(), f@ as int, idx@ as int),
    ensures eligible(topology, pending, idx),
{
    pending[idx].facts(topology);
    assert(selected(topology, pending, idx));
    assert forall|a: NodeIdx| selected(topology, pending, a) && a != idx implies
        !graph::reaches(topology.graph(), a@ as int, idx@ as int) by {
        if graph::reaches(topology.graph(), a@ as int, idx@ as int) {
            let f = choose|f: NodeIdx| pending.contains_key(f) && pending[f].selected(topology, a);
            pending[f].facts(topology);
            graph::reaches_order(topology.graph(), a@ as int, idx@ as int);
            if a@ == idx@ { NodeIdx::same_index(a, idx); }
            graph::transitive(topology.graph(), f@ as int, a@ as int, idx@ as int);
            assert(f@ < idx@);
        }
    }
}

pub(crate) fn dependency<P>(topology: &Topology<P>, pending: &BTreeMap<NodeIdx, PendingChain>, idx: NodeIdx) -> (out: Option<NodeIdx>)
    requires valid(topology, pending@), pending@.contains_key(idx),
    ensures match out {
        None => eligible(topology, pending@, idx),
        Some(dep) => selected(topology, pending@, dep) && dep@ < idx@
            && graph::reaches(topology.graph(), dep@ as int, idx@ as int)
            && exists|f: NodeIdx| pending@.contains_key(f) && f@ <= dep@,
    },
{
    proof { topology.established(); pending@[idx].facts(topology); }
    let (mut earlier, nonempty) = Earlier::new(pending, idx);
    let ghost all = earlier.remaining();
    if nonempty {
        let ancestry = topology.clock(idx);
        let ghost mut done = 0int;
        loop
            invariant topology.valid(), valid(topology, pending@), pending@.contains_key(idx),
                idx@ < topology.nodes_view().len(),
                ancestry.valid(),
                forall|a: usize| a < topology.nodes_view().len() ==> super::clock::below(ancestry.lookup(topology.nodes_view()[a as int].chain), a)
                    == graph::history(topology.graph(), ISet::empty().insert(idx@ as int)).contains(a as int),
                0 <= done <= all.len(), earlier.remaining() == all.skip(done),
                forall|i: int| 0 <= i < all.len() ==> all[i].0@ < idx@ && pending@.contains_key(all[i].0) && pending@[all[i].0] == all[i].1,
                forall|f: NodeIdx| pending@.contains_key(f) && f@ < idx@ ==> all.contains((f, pending@[f])),
                forall|i: int| 0 <= i < done ==> !graph::reaches(topology.graph(), all[i].0@ as int, idx@ as int),
            decreases earlier.remaining().len(),
        {
            let Some((frontier, chain)) = earlier.next() else {
                proof {
                    assert(done == all.len());
                    assert forall|f: NodeIdx| pending@.contains_key(f) && f@ < idx@ implies
                        !graph::reaches(topology.graph(), f@ as int, idx@ as int) by {
                        let i = choose|i: int| 0 <= i < all.len() && all[i] == (f, pending@[f]);
                    }
                }
                proof { frontier_test(topology, pending@, idx); }
                return None;
            };
            proof {
                chain.facts(topology);
                crate::ancestry_refinement::singleton_history(topology.graph(), frontier@ as int, idx@ as int);
            }
            if ancestry.contains(chain.chain, *frontier) {
                let tip = ancestry.position(chain.chain).unwrap();
                let dep = chain.last_before(topology, tip);
                proof {
                    assert(pending@[*frontier].selected(topology, dep));
                    assert(selected(topology, pending@, dep));
                    crate::ancestry_refinement::singleton_history(topology.graph(), dep@ as int, idx@ as int);
                    assert(graph::reaches(topology.graph(), dep@ as int, idx@ as int));
                    graph::reaches_order(topology.graph(), dep@ as int, idx@ as int);
                    pending@[idx].facts(topology);
                    assert(chain.chain != pending@[idx].chain);
                    assert(dep != idx);
                    if dep@ == idx@ { NodeIdx::same_index(dep, idx); }
                    assert(frontier@ <= dep@);
                }
                return Some(dep);
            }
            proof { done = done + 1; }
        }
    } else {
        assert forall|f: NodeIdx| pending@.contains_key(f) && f@ < idx@ implies false by {
            assert(all.contains((f, pending@[f])));
        }
    }
    proof { frontier_test(topology, pending@, idx); }
    None
}

pub(crate) open spec fn waiting(queues: &Queues, dep: NodeIdx, n: NodeIdx) -> bool {
    queues.blocked().contains_key(dep) && queues.blocked()[dep].contains(n)
}

pub(crate) open spec fn scheduled<P>(topology: &Topology<P>, pending: Map<NodeIdx, PendingChain>, queues: &Queues, idx: NodeIdx) -> bool {
    &&& queues.queued(idx)
    &&& queues.ready().contains((topology.nodes_view()[idx@ as int].hash, idx)) == eligible(topology, pending, idx)
    &&& forall|key: super::schedule_queues::ReadyKey| queues.ready().contains(key) && key.1 == idx ==>
        key.0 == topology.nodes_view()[idx@ as int].hash && eligible(topology, pending, idx)
    &&& forall|dep: NodeIdx| waiting(queues, dep, idx) ==>
        selected(topology, pending, dep) && graph::reaches(topology.graph(), dep@ as int, idx@ as int)
}

pub(crate) fn schedule<P>(topology: &Topology<P>, pending: &BTreeMap<NodeIdx, PendingChain>, queues: &mut Queues, idx: NodeIdx)
    requires valid(topology, pending@), pending@.contains_key(idx), old(queues).valid(), !old(queues).queued(idx),
    ensures final(queues).valid(),
        scheduled(topology, pending@, final(queues), idx),
        forall|key: super::schedule_queues::ReadyKey| key.1 != idx ==>
            final(queues).ready().contains(key) == old(queues).ready().contains(key),
        forall|dep: NodeIdx, n: NodeIdx| n != idx ==>
            waiting(final(queues), dep, n)
                == waiting(old(queues), dep, n),
        forall|n: NodeIdx| final(queues).queued(n) == (old(queues).queued(n) || n == idx),
        final(queues).ready().contains((topology.nodes_view()[idx@ as int].hash, idx)) == eligible(topology, pending@, idx),
        final(queues).ready() == if eligible(topology, pending@, idx) {
            old(queues).ready().insert((topology.nodes_view()[idx@ as int].hash, idx))
        } else { old(queues).ready() },
        eligible(topology, pending@, idx) ==> final(queues).blocked() == old(queues).blocked(),
        !eligible(topology, pending@, idx) ==> exists|dep: NodeIdx|
            selected(topology, pending@, dep) && dep@ < idx@
            && graph::reaches(topology.graph(), dep@ as int, idx@ as int)
            && (exists|f: NodeIdx| pending@.contains_key(f) && f@ <= dep@)
            && final(queues).blocked() == old(queues).blocked().insert(dep,
                (if old(queues).blocked().contains_key(dep) { old(queues).blocked()[dep] } else { Seq::empty() }).push(idx)),
{
    proof { topology.established(); pending@[idx].facts(topology); }
    match dependency(topology, pending, idx) {
        Some(dep) => {
            let ghost previous = *queues;
            queues.block(dep, idx);
            assert(!eligible(topology, pending@, idx));
            assert forall|d: NodeIdx, n: NodeIdx| n != idx implies
                waiting(queues, d, n) == waiting(&previous, d, n) by {
                if d == dep {
                    let row = if previous.blocked().contains_key(dep) { previous.blocked()[dep] } else { Seq::empty() };
                    vstd::seq_lib::lemma_seq_contains_after_push(row, idx, n);
                }
            }
        },
        None => queues.make_ready(topology.nodes()[idx.index()].hash, idx),
    }
}

/// Reconsider a detached waiter list once each, preserving unrelated queue entries.
pub(crate) fn reschedule<P>(topology: &Topology<P>, pending: &BTreeMap<NodeIdx, PendingChain>, queues: &mut Queues, waiters: Vec<NodeIdx>)
    requires valid(topology, pending@), old(queues).valid(), waiters@.no_duplicates(),
        forall|i: int| 0 <= i < waiters.len() ==> pending@.contains_key(waiters[i]) && !old(queues).queued(waiters[i]),
    ensures final(queues).valid(),
        forall|n: NodeIdx| final(queues).queued(n) == (old(queues).queued(n) || waiters@.contains(n)),
        forall|n: NodeIdx| waiters@.contains(n) ==> scheduled(topology, pending@, final(queues), n),
        forall|key: super::schedule_queues::ReadyKey| !waiters@.contains(key.1) ==>
            final(queues).ready().contains(key) == old(queues).ready().contains(key),
        forall|dep: NodeIdx, n: NodeIdx| !waiters@.contains(n) ==>
            waiting(final(queues), dep, n)
                == waiting(old(queues), dep, n),
{
    let ghost before = *queues;
    let mut i = 0;
    while i < waiters.len()
        invariant valid(topology, pending@), queues.valid(), waiters@.no_duplicates(), i <= waiters.len(),
            forall|j: int| 0 <= j < waiters.len() ==> pending@.contains_key(waiters[j]) && !before.queued(waiters[j]),
            forall|n: NodeIdx| queues.queued(n) == (before.queued(n) || waiters@.take(i as int).contains(n)),
            forall|j: int| 0 <= j < i ==> scheduled(topology, pending@, queues, waiters[j]),
            forall|key: super::schedule_queues::ReadyKey| !waiters@.take(i as int).contains(key.1) ==>
                queues.ready().contains(key) == before.ready().contains(key),
            forall|dep: NodeIdx, n: NodeIdx| !waiters@.take(i as int).contains(n) ==>
                waiting(queues, dep, n)
                    == waiting(&before, dep, n),
        decreases waiters.len() - i,
    {
        proof {
            assert(!waiters@.take(i as int).contains(waiters[i as int])) by {
                if waiters@.take(i as int).contains(waiters[i as int]) {
                    let j = choose|j: int| 0 <= j < i && waiters[j] == waiters[i as int];
                }
            }
        }
        schedule(topology, pending, queues, waiters[i]);
        proof {
            assert(waiters@.take(i + 1) =~= waiters@.take(i as int).push(waiters[i as int]));
            broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
        }
        i += 1;
    }
    assert(waiters@.take(i as int) == waiters@);
    assert forall|n: NodeIdx| waiters@.contains(n) implies scheduled(topology, pending@, queues, n) by {
        let j = choose|j: int| 0 <= j < waiters.len() && waiters[j] == n;
    }
}

/// Detach the complete row before rescheduling; no released waiter is lost or duplicated.
pub(crate) fn wake<P>(topology: &Topology<P>, pending: &BTreeMap<NodeIdx, PendingChain>, queues: &mut Queues, emitted: NodeIdx)
    requires valid(topology, pending@), old(queues).valid(),
        forall|n: NodeIdx| waiting(old(queues), emitted, n)
            ==> pending@.contains_key(n),
    ensures final(queues).valid(),
        forall|n: NodeIdx| final(queues).queued(n) == old(queues).queued(n),
        forall|n: NodeIdx| waiting(old(queues), emitted, n)
            ==> scheduled(topology, pending@, final(queues), n),
        forall|key: super::schedule_queues::ReadyKey|
            !waiting(old(queues), emitted, key.1) ==>
                final(queues).ready().contains(key) == old(queues).ready().contains(key),
        forall|dep: NodeIdx, n: NodeIdx|
            !waiting(old(queues), emitted, n) ==>
                waiting(final(queues), dep, n)
                    == waiting(old(queues), dep, n),
        !selected(topology, pending@, emitted) ==> !final(queues).blocked().contains_key(emitted),
{
    let ghost before = *queues;
    let waiters = queues.take_waiters(emitted);
    let ghost released = waiters@;
    let ghost detached = *queues;
    proof {
        assert forall|i: int| 0 <= i < waiters.len() implies
            pending@.contains_key(waiters[i]) && !queues.queued(waiters[i]) by {
            let n = waiters[i];
            assert(released.contains(n));
            assert(waiting(&before, emitted, n));
        }
    }
    reschedule(topology, pending, queues, waiters);
    proof {
        assert forall|n: NodeIdx| queues.queued(n) == before.queued(n) by {
            if released.contains(n) {
                assert(before.blocked().contains_key(emitted));
                assert(before.blocked()[emitted].contains(n));
                assert(before.queued(n));
            }
        }
        if !selected(topology, pending@, emitted) && queues.blocked().contains_key(emitted) {
            let n = queues.blocked()[emitted][0];
            assert(queues.blocked()[emitted].contains(n));
            assert(waiting(queues, emitted, n));
            assert(!waiting(&detached, emitted, n));
            assert(released.contains(n));
            assert(scheduled(topology, pending@, queues, n));
        }
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{author::Author, commit_hash::CommitHash};

    #[test]
    fn shared_dependency_wakeup_reblocks_and_retires_exactly_once() {
        let mut topology = Topology::new();
        let hashes = [8, 9, 1, 0, 2].map(|b| CommitHash([b; 32]));
        let a = topology
            .insert(hashes[0], Author([1; 32]), &[], (), 0)
            .unwrap();
        let tip = topology
            .insert(hashes[1], Author([1; 32]), &[hashes[0]], (), 0)
            .unwrap();
        let b = topology
            .insert(hashes[2], Author([2; 32]), &[hashes[1]], (), 0)
            .unwrap();
        let c = topology
            .insert(hashes[3], Author([3; 32]), &[hashes[1], hashes[2]], (), 0)
            .unwrap();
        let independent = topology
            .insert(hashes[4], Author([4; 32]), &[], (), 0)
            .unwrap();
        let mut pending = BTreeMap::new();
        for (node, end) in [(a, 2), (b, 1), (c, 1), (independent, 1)] {
            pending.insert(
                node,
                PendingChain {
                    chain: topology.nodes()[node.index()].chain,
                    intervals: Intervals::new(vec![0..end]),
                },
            );
        }
        let mut queues = Queues::new();
        for &node in pending.keys() {
            schedule(&topology, &pending, &mut queues, node);
        }
        assert_eq!(queues.retained_counts(), (2, 2));
        // Small hashes cannot bypass ancestry; only the independent root wins.
        assert_eq!(queues.pop_ready(), Some((hashes[4], independent)));
        assert_eq!(advance(&topology, &mut pending, independent), None);
        wake(&topology, &pending, &mut queues, independent);
        assert_eq!(queues.pop_ready(), Some((hashes[0], a)));
        assert_eq!(advance(&topology, &mut pending, a), Some(tip));
        assert_eq!(pending.len(), 3);
        schedule(&topology, &pending, &mut queues, tip);
        wake(&topology, &pending, &mut queues, a);
        assert_eq!(queues.retained_counts(), (1, 2));
        assert_eq!(queues.pop_ready(), Some((hashes[1], tip)));
        assert_eq!(advance(&topology, &mut pending, tip), None);
        wake(&topology, &pending, &mut queues, tip);
        assert_eq!(queues.retained_counts(), (1, 1));
        wake(&topology, &pending, &mut queues, tip);
        assert_eq!(queues.retained_counts(), (1, 1));
        assert_eq!(queues.pop_ready(), Some((hashes[2], b)));
        assert_eq!(advance(&topology, &mut pending, b), None);
        wake(&topology, &pending, &mut queues, b);
        assert_eq!(queues.pop_ready(), Some((hashes[3], c)));
        assert_eq!(advance(&topology, &mut pending, c), None);
        wake(&topology, &pending, &mut queues, c);
        assert!(pending.is_empty());
        assert_eq!(queues.retained_counts(), (0, 0));
        assert_eq!(queues.pop_ready(), None);
    }

    #[test]
    fn dependency_skips_gaps_unrelated_roots_and_later_selected_descendants() {
        let mut topology = Topology::new();
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let carol = Author([3; 32]);
        let hashes = [0, 1, 2, 3, 4, 5].map(|b| CommitHash([b; 32]));
        let root = topology.insert(hashes[0], alice, &[], (), 0).unwrap();
        topology
            .insert(hashes[1], alice, &[hashes[0]], (), 0)
            .unwrap();
        let tip = topology
            .insert(hashes[2], alice, &[hashes[1]], (), 0)
            .unwrap();
        let unrelated = topology.insert(hashes[3], bob, &[], (), 0).unwrap();
        let waiter = topology
            .insert(hashes[4], carol, &[hashes[2]], (), 0)
            .unwrap();
        let later = topology
            .insert(hashes[5], alice, &[hashes[4]], (), 0)
            .unwrap();
        let alice_chain = topology.nodes()[root.index()].chain;
        assert_eq!(
            topology.chain_nodes()[alice_chain as usize],
            vec![root, NodeIdx::new(1), tip, later]
        );
        let mut pending = BTreeMap::from([
            (
                root,
                PendingChain {
                    chain: alice_chain,
                    intervals: Intervals::new(vec![0..1, 2..4]),
                },
            ),
            (
                unrelated,
                PendingChain {
                    chain: topology.nodes()[unrelated.index()].chain,
                    intervals: Intervals::new(vec![0..1]),
                },
            ),
            (
                waiter,
                PendingChain {
                    chain: topology.nodes()[waiter.index()].chain,
                    intervals: Intervals::new(vec![0..1]),
                },
            ),
        ]);
        assert_eq!(dependency(&topology, &pending, root), None);
        assert_eq!(dependency(&topology, &pending, unrelated), None);
        assert_eq!(dependency(&topology, &pending, waiter), Some(tip));
        let mut queues = Queues::new();
        for &idx in pending.keys() {
            schedule(&topology, &pending, &mut queues, idx);
        }
        assert_eq!(queues.pop_ready(), Some((hashes[0], root)));
        assert_eq!(queues.pop_ready(), Some((hashes[3], unrelated)));
        assert_eq!(queues.pop_ready(), None);
        assert!(queues.take_waiters(root).is_empty());
        assert_eq!(queues.take_waiters(tip), vec![waiter]);
        assert_eq!(advance(&topology, &mut pending, root), Some(tip));
        assert_eq!(advance(&topology, &mut pending, tip), Some(later));
        assert_eq!(dependency(&topology, &pending, waiter), None);
        pending.remove(&unrelated);
        assert_eq!(dependency(&topology, &pending, waiter), None);
        schedule(&topology, &pending, &mut queues, waiter);
        assert_eq!(queues.pop_ready(), Some((hashes[4], waiter)));
    }
}
