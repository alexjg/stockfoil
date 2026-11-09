//! Erased framing of the production scheduler across arbitrary History growth.
#[cfg(verus_keep_ghost)]
use super::{
    clock::NodeIdx,
    pending::{self, PendingChain},
    schedule_queues::{Queues, ReadyKey},
    topology::Topology,
};
#[cfg(verus_keep_ghost)]
use crate::{capture_refinement::ReadState, graph};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Any number of appends leaves every old node's ancestors unchanged.
pub(crate) proof fn reachability(before: graph::Graph, after: graph::Graph, a: int, d: int)
    requires before.is_prefix_of(after), 0 <= d < before.len(),
    ensures graph::reaches(before, a, d) == graph::reaches(after, a, d),
    decreases after.len() - before.len(),
{
    if before.len() == after.len() {
        assert(before =~= after);
    } else {
        let prefix = after.drop_last();
        assert(before.is_prefix_of(prefix));
        assert(after =~= prefix.push(after.last()));
        reachability(before, prefix, a, d);
        graph::append_preserves_reachability(prefix, after.last(), a, d);
    }
}

proof fn chain<P>(before: &Topology<P>, after: &Topology<P>, cursor: &PendingChain)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()), cursor.valid(before),
    ensures cursor.valid(after), cursor.frontier(after) == cursor.frontier(before),
        forall|n: NodeIdx| cursor.selected(after, n) == cursor.selected(before, n),
{
    reveal(ReadState::extends);
    let c = cursor.chain as int;
    let old_row = before.tables_view()[c]@;
    let new_row = after.tables_view()[c]@;
    assert(before.read_state().tables[c] == old_row);
    assert(after.read_state().tables[c] == new_row);
    assert(before.read_state().tables[c].is_prefix_of(after.read_state().tables[c]));
    assert(old_row.is_prefix_of(new_row));
    assert forall|n: NodeIdx| cursor.selected(after, n) == cursor.selected(before, n) by {
        if cursor.selected(after, n) {
            let p = choose|p: int| cursor.intervals.contains(p) && new_row[p] == n;
            assert(old_row[p] == n);
            assert(cursor.selected(before, n));
        }
        if cursor.selected(before, n) {
            let p = choose|p: int| cursor.intervals.contains(p) && old_row[p] == n;
            assert(new_row[p] == n);
            assert(cursor.selected(after, n));
        }
    }
    assert(cursor.intervals.contains(cursor.intervals.view()[0].start as int));
}

pub(crate) proof fn selected_hash<P>(before: &Topology<P>, after: &Topology<P>, p: Map<NodeIdx, PendingChain>, n: NodeIdx)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()),
        pending::valid(before, p), pending::valid(after, p), pending::selected(before, p, n), pending::selected(after, p, n),
    ensures n@ < before.nodes_view().len() && n@ < after.nodes_view().len()
        && after.nodes_view()[n@ as int].hash == before.nodes_view()[n@ as int].hash,
{
    reveal(ReadState::extends);
    before.traversal_facts(); after.hash_facts();
    let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(before, n);
    let g = choose|g: NodeIdx| p.contains_key(g) && p[g].selected(after, n);
    p[f].facts(before); p[g].facts(after);
    let h = before.nodes_view()[n@ as int].hash;
    assert(before.hashes().contains_key(h));
    assert(before.read_state().hashes.contains_key(h));
    assert(after.read_state().hashes.contains_key(h));
    assert(after.hashes()[h]@ == n@);
}

proof fn selection_frame<P>(before: &Topology<P>, after: &Topology<P>, p: Map<NodeIdx, PendingChain>)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()), pending::valid(before, p),
    ensures pending::valid(after, p),
        forall|n: NodeIdx| pending::selected(after, p, n) == pending::selected(before, p, n),
{
    assert forall|f: NodeIdx| p.contains_key(f) implies p[f].valid(after)
        && p[f].frontier(after) == f
        && (forall|n: NodeIdx| p[f].selected(after, n) == p[f].selected(before, n)) by {
        chain(before, after, &p[f]);
    }
    assert(pending::valid(after, p));
    assert forall|n: NodeIdx| pending::selected(after, p, n) == pending::selected(before, p, n) by {
        if pending::selected(after, p, n) {
            let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(after, n);
            assert(p[f].selected(before, n));
        }
        if pending::selected(before, p, n) {
            let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(before, n);
            assert(p[f].selected(after, n));
        }
    }
}

pub(crate) proof fn eligibility<P>(before: &Topology<P>, after: &Topology<P>, p: Map<NodeIdx, PendingChain>, n: NodeIdx)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()), pending::valid(before, p),
        forall|a: NodeIdx| pending::selected(after, p, a) == pending::selected(before, p, a),
    ensures pending::eligible(after, p, n) == pending::eligible(before, p, n),
{
    if pending::selected(before, p, n) {
        before.traversal_facts();
        let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(before, n);
        p[f].facts(before);
        reveal(ReadState::extends);
        assert(before.read_state().graph.is_prefix_of(after.read_state().graph));
        assert forall|a: NodeIdx| graph::reaches(before.graph(), a@ as int, n@ as int)
            == graph::reaches(after.graph(), a@ as int, n@ as int) by {
            reachability(before.graph(), after.graph(), a@ as int, n@ as int);
        }
    }
}

proof fn membership<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, n: NodeIdx)
    requires t.valid(), pending::valid(t, p),
    ensures pending::selected_set(t, p).contains(n) == pending::selected(t, p, n),
{ pending::selection_facts(t, p); }

pub(crate) proof fn selection<P>(before: &Topology<P>, after: &Topology<P>, p: Map<NodeIdx, PendingChain>)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()), pending::valid(before, p),
    ensures pending::valid(after, p), pending::selected_set(after, p) == pending::selected_set(before, p),
        forall|n: NodeIdx| pending::selected(after, p, n) == pending::selected(before, p, n),
{
    selection_frame(before, after, p);
    assert forall|n: NodeIdx| pending::selected_set(after, p).contains(n) == pending::selected_set(before, p).contains(n) by {
        membership(before, p, n); membership(after, p, n);
    }
    assert(pending::selected_set(after, p) =~= pending::selected_set(before, p));
}

/// The actual retained map, ready queue, waiters, and remaining count need no rebuild.
pub(crate) proof fn scheduler<P>(before: &Topology<P>, after: &Topology<P>, p: Map<NodeIdx, PendingChain>, q: &Queues, remaining: usize)
    requires before.valid(), after.valid(), after.read_state().extends(before.read_state()),
        super::traversal_step::valid(before, p, q, remaining),
    ensures super::traversal_step::valid(after, p, q, remaining),
        pending::selected_set(after, p) == pending::selected_set(before, p),
{
    reveal(ReadState::extends);
    selection(before, after, p);
    before.traversal_facts();
    assert forall|key: ReadyKey| q.ready().contains(key) implies p.contains_key(key.1)
        && pending::eligible(after, p, key.1) && key.0 == after.nodes_view()[key.1@ as int].hash by {
        eligibility(before, after, p, key.1);
        selected_hash(before, after, p, key.1);
    };
    assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(q, d, n) implies p.contains_key(n)
        && (pending::selected(after, p, d) || None::<NodeIdx> == Some(d))
        && graph::reaches(after.graph(), d@ as int, n@ as int) by {
        p[n].facts(before);
        reachability(before.graph(), after.graph(), d@ as int, n@ as int);
    }
}

} // verus!
