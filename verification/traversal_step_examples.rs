//! Composition consumers: constructed nonempty state and arbitrary valid-state draining.
use vstd::prelude::*;
use std::collections::BTreeMap;
use crate::{clock::NodeIdx, topology::Topology, pending::{self, PendingChain},
    schedule_queues::Queues, traversal_step as step, graph};

verus! {

pub(crate) open spec fn drain_state(g: graph::Graph, original: Set<NodeIdx>, remaining: Set<NodeIdx>, out: Seq<NodeIdx>) -> bool {
    &&& out.no_duplicates()
    &&& out.to_set().subset_of(original)
    &&& remaining == original.difference(out.to_set())
    &&& forall|i: int, j: int| 0 <= i < j < out.len() ==> !graph::reaches(g, out[j]@ as int, out[i]@ as int)
    &&& forall|i: int, n: NodeIdx| 0 <= i < out.len() && remaining.contains(n) ==>
        !graph::reaches(g, n@ as int, out[i]@ as int)
}

proof fn append_emission(g: graph::Graph, original: Set<NodeIdx>, remaining: Set<NodeIdx>, out: Seq<NodeIdx>, idx: NodeIdx)
    requires drain_state(g, original, remaining, out), remaining.contains(idx),
        forall|n: NodeIdx| remaining.contains(n) && n != idx ==> !graph::reaches(g, n@ as int, idx@ as int),
    ensures drain_state(g, original, remaining.remove(idx), out.push(idx)),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    assert(!out.contains(idx));
    assert(out.push(idx).to_set() =~= out.to_set().insert(idx));
    assert(remaining.remove(idx) =~= original.difference(out.push(idx).to_set()));
    assert forall|i: int, j: int| 0 <= i < j < out.push(idx).len() implies
        !graph::reaches(g, out.push(idx)[j]@ as int, out.push(idx)[i]@ as int) by {
        if j == out.len() { assert(remaining.contains(idx)); }
    }
    assert forall|i: int, n: NodeIdx| 0 <= i < out.push(idx).len() && remaining.remove(idx).contains(n) implies
        !graph::reaches(g, n@ as int, out.push(idx)[i]@ as int) by {
        if i == out.len() { assert(n != idx); }
        else { assert(remaining.contains(n)); }
    }
}

proof fn eligible_selection<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, idx: NodeIdx)
    requires t.valid(), pending::valid(t, p), pending::eligible(t, p, idx),
    ensures pending::selected_set(t, p).contains(idx),
        forall|n: NodeIdx| pending::selected_set(t, p).contains(n) && n != idx ==>
            !graph::reaches(t.graph(), n@ as int, idx@ as int),
{ pending::selection_facts(t, p); }

/// At a fixed topology, exact one-record progress proves termination and complete enumeration.
pub(crate) fn drain<P>(t: &Topology<P>, p: &mut BTreeMap<NodeIdx, PendingChain>, q: &mut Queues, remaining: &mut usize) -> (out: Vec<NodeIdx>)
    requires step::valid(t, old(p)@, old(q), *old(remaining)),
    ensures step::valid(t, final(p)@, final(q), *final(remaining)), *final(remaining) == 0,
        out@.to_set() == pending::selected_set(t, old(p)@), out@.no_duplicates(), out.len() == *old(remaining),
        forall|i: int, j: int| 0 <= i < j < out.len() ==> !graph::reaches(t.graph(), out[j]@ as int, out[i]@ as int),
{
    proof { t.established(); }
    let ghost original = pending::selected_set(t, p@);
    let ghost total = *remaining;
    let mut out: Vec<NodeIdx> = Vec::new();
    assert(out@.to_set() =~= Set::<NodeIdx>::empty());
    assert(original.difference(out@.to_set()) =~= original);
    assert(drain_state(t.graph(), original, pending::selected_set(t, p@), out@));
    while *remaining > 0
        invariant t.valid(), step::valid(t, p@, q, *remaining),
            drain_state(t.graph(), original, pending::selected_set(t, p@), out@),
            *remaining + out.len() == total,
        decreases *remaining,
    {
        let ghost before = p@;
        let ghost prior = out@;
        let idx = step::next(t, p, q, remaining).unwrap();
        proof {
            eligible_selection(t, before, idx);
            append_emission(t.graph(), original, pending::selected_set(t, before), prior, idx);
        }
        out.push(idx);
    }
    assert(pending::selected_set(t, p@).is_empty());
    assert(out@.to_set() =~= original);
    out
}

pub fn constructed_joint_step_and_stable_exhaustion() {
    let mut topology = Topology::new();
    let root = crate::topology_examples::add(&mut topology, crate::commit_hash::CommitHash([9u8; 32]),
        crate::author::Author([1u8; 32]), &vec![]);
    let mut p = crate::pending_examples::singleton_pending(&topology, root);
    proof {
        pending::selection_facts(&topology, p@);
        assert(pending::selected_set(&topology, p@) =~= Set::empty().insert(root));
    }
    let mut remaining = 1;
    let mut q = step::initialize(&topology, &p, remaining);
    let first = step::next(&topology, &mut p, &mut q, &mut remaining);
    assert(first == Some(root) && remaining == 0);
    let done = step::next(&topology, &mut p, &mut q, &mut remaining);
    assert(done is None && remaining == 0);
    let still_done = step::next(&topology, &mut p, &mut q, &mut remaining);
    assert(still_done is None && remaining == 0);
    assert(step::valid(&topology, p@, &q, remaining));
}
}
