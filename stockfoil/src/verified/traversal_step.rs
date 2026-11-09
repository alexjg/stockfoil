//! One production traversal step, checked against the joint scheduler invariant.
#[cfg(verus_keep_ghost)]
use super::schedule_queues::ReadyKey;
use super::{
    clock::NodeIdx,
    pending::{self, PendingChain},
    schedule_queues::Queues,
    topology::Topology,
};
#[cfg(verus_keep_ghost)]
use crate::{graph, scheduler_refinement};
use std::collections::BTreeMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
use vstd::std_specs::iter::IteratorSpec;

verus! {

/// Between advancement and wakeup, only the just-emitted dependency may be stale.
pub(crate) open spec fn sound<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, q: &Queues, retired: Option<NodeIdx>) -> bool {
    &&& forall|key: ReadyKey| q.ready().contains(key) ==> p.contains_key(key.1)
        && pending::eligible(t, p, key.1) && key.0 == t.nodes_view()[key.1@ as int].hash
    &&& forall|d: NodeIdx, n: NodeIdx| pending::waiting(q, d, n) ==> p.contains_key(n)
        && (pending::selected(t, p, d) || retired == Some(d))
        && graph::reaches(t.graph(), d@ as int, n@ as int)
}

pub(crate) open spec fn valid<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, q: &Queues, remaining: usize) -> bool {
    &&& pending::valid(t, p)
    &&& q.valid()
    &&& sound(t, p, q, None)
    &&& forall|n: NodeIdx| q.queued(n) == p.contains_key(n)
    &&& remaining == pending::selected_set(t, p).len()
}

/// Retain only the chain frontiers during initialization, never all selected records.
fn frontiers(p: &BTreeMap<NodeIdx, PendingChain>) -> (out: Vec<NodeIdx>)
    ensures out@.to_set() == p@.dom(), out@.no_duplicates(),
{
    proof { super::stdlib::node_order_model(); }
    let keys = p.keys();
    let ghost input = keys.remaining();
    let mapped = keys.map(|key: &NodeIdx| -> (out: NodeIdx) ensures out == *key { *key });
    let out: Vec<NodeIdx> = mapped.collect();
    proof {
        broadcast use vstd::std_specs::iter::map_postcondition;
        assert(out.len() == input.len());
        assert forall|i: int| 0 <= i < out.len() implies out[i] == *input[i] by {};
        assert(out@ =~= input.unref());
        assert forall|i: int, j: int| 0 <= i < j < out.len() implies out[i] != out[j] by {
            assert(input[i] != input[j]);
        }
    }
    out
}

pub(crate) fn initialize<P>(t: &Topology<P>, p: &BTreeMap<NodeIdx, PendingChain>, _remaining: usize) -> (out: Queues)
    requires pending::valid(t, p@), _remaining == pending::selected_set(t, p@).len(),
    ensures valid(t, p@, &out, _remaining),
{
    let mut q = Queues::new();
    let keys = frontiers(p);
    let ghost keys_view = keys@;
    pending::reschedule(t, p, &mut q, keys);
    proof {
        assert forall|key: ReadyKey| q.ready().contains(key) implies p@.contains_key(key.1)
            && pending::eligible(t, p@, key.1) && key.0 == t.nodes_view()[key.1@ as int].hash by {
            assert(keys_view.contains(key.1));
            assert(pending::scheduled(t, p@, &q, key.1));
        }
        assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(&q, d, n) implies p@.contains_key(n)
            && (pending::selected(t, p@, d) || None::<NodeIdx> == Some(d))
            && graph::reaches(t.graph(), d@ as int, n@ as int) by {
            assert(q.queued(n));
            assert(keys_view.contains(n));
            assert(pending::scheduled(t, p@, &q, n));
        }
        assert forall|n: NodeIdx| q.queued(n) == p@.contains_key(n) by {};
    }
    q
}

proof fn progress<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, q: &Queues, remaining: usize)
    requires t.valid(), valid(t, p, q, remaining),
    ensures q.ready().is_empty() ==> p.dom().is_empty() && remaining == 0,
{
    pending::selection_facts(t, p);
    assert forall|d: NodeIdx| q.blocked().contains_key(d) implies
        exists|f: NodeIdx| p.dom().contains(f) && f@ <= d@ by {
        let n = q.blocked()[d][0];
        assert(q.blocked()[d].contains(n));
        assert(pending::waiting(q, d, n));
        assert(pending::selected(t, p, d));
        let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(t, d);
        p[f].facts(t);
        assert(p.dom().contains(f) && f@ <= d@);
    }
    assert(scheduler_refinement::complete_frontiers(q, p.dom()));
    if !p.dom().is_empty() { scheduler_refinement::some_frontier_ready(q, p.dom()); }
}

proof fn eligible_frontier<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, n: NodeIdx)
    requires t.valid(), pending::valid(t, p), pending::eligible(t, p, n),
    ensures p.contains_key(n),
{
    let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(t, n);
    p[f].facts(t);
    assert(pending::selected(t, p, f));
    assert(graph::reaches(t.graph(), f@ as int, n@ as int));
    assert(n == f);
}

proof fn eligible_ready<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, q: &Queues, remaining: usize, n: NodeIdx)
    requires t.valid(), valid(t, p, q, remaining), pending::eligible(t, p, n),
    ensures n@ < t.nodes_view().len(), q.ready().contains((t.nodes_view()[n@ as int].hash, n)),
{
    eligible_frontier(t, p, n);
    p[n].facts(t);
    assert(q.queued(n));
    if exists|d: NodeIdx| pending::waiting(q, d, n) {
        let d = choose|d: NodeIdx| pending::waiting(q, d, n);
        let i = choose|i: int| 0 <= i < q.blocked()[d].len() && q.blocked()[d][i] == n;
        assert(d@ < n@);
        assert(pending::selected(t, p, d));
        assert(graph::reaches(t.graph(), d@ as int, n@ as int));
        assert(!graph::reaches(t.graph(), d@ as int, n@ as int));
    }
    assert(!(exists|d: NodeIdx| q.blocked().contains_key(d) && q.blocked()[d].contains(n))) by {
        if exists|d: NodeIdx| q.blocked().contains_key(d) && q.blocked()[d].contains(n) {
            let d = choose|d: NodeIdx| q.blocked().contains_key(d) && q.blocked()[d].contains(n);
            assert(pending::waiting(q, d, n));
        }
    }
    let key = choose|key: ReadyKey| q.ready().contains(key) && key.1 == n;
    assert(key.0 == t.nodes_view()[n@ as int].hash);
}

proof fn eligible_survives<P>(t: &Topology<P>, before: Map<NodeIdx, PendingChain>, after: Map<NodeIdx, PendingChain>, n: NodeIdx, emitted: NodeIdx)
    requires pending::eligible(t, before, n), n != emitted,
        forall|a: NodeIdx| pending::selected(t, after, a) == (pending::selected(t, before, a) && a != emitted),
    ensures pending::eligible(t, after, n),
{}

proof fn after_advance<P>(t: &Topology<P>, before: Map<NodeIdx, PendingChain>, after: Map<NodeIdx, PendingChain>, old_q: &Queues, q: &Queues, emitted: NodeIdx)
    requires t.valid(), pending::valid(t, before), pending::valid(t, after),
        sound(t, before, old_q, None), q.valid(), !q.queued(emitted),
        q.ready().subset_of(old_q.ready()), q.blocked() == old_q.blocked(),
        forall|n: NodeIdx| before.contains_key(n) && n != emitted ==> after.contains_key(n),
        forall|n: NodeIdx| pending::selected(t, after, n) == (pending::selected(t, before, n) && n != emitted),
    ensures sound(t, after, q, Some(emitted)),
{
    assert forall|key: ReadyKey| q.ready().contains(key) implies after.contains_key(key.1)
        && pending::eligible(t, after, key.1) && key.0 == t.nodes_view()[key.1@ as int].hash by {
        assert(q.queued(key.1));
        eligible_survives(t, before, after, key.1, emitted);
    }
    assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(q, d, n) implies after.contains_key(n)
        && (pending::selected(t, after, d) || Some(emitted) == Some(d))
        && graph::reaches(t.graph(), d@ as int, n@ as int) by {
        assert(pending::waiting(old_q, d, n));
        assert(q.queued(n));
    }
}

proof fn after_schedule<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, before: &Queues, after: &Queues, next: NodeIdx, retired: NodeIdx)
    requires sound(t, p, before, Some(retired)), p.contains_key(next),
        pending::scheduled(t, p, after, next),
        forall|key: ReadyKey| key.1 != next ==> after.ready().contains(key) == before.ready().contains(key),
        forall|d: NodeIdx, n: NodeIdx| n != next ==> pending::waiting(after, d, n) == pending::waiting(before, d, n),
    ensures sound(t, p, after, Some(retired)),
{
    assert forall|key: ReadyKey| after.ready().contains(key) implies p.contains_key(key.1)
        && pending::eligible(t, p, key.1) && key.0 == t.nodes_view()[key.1@ as int].hash by {
        if key.1 != next { assert(before.ready().contains(key)); }
    }
    assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(after, d, n) implies p.contains_key(n)
        && (pending::selected(t, p, d) || Some(retired) == Some(d))
        && graph::reaches(t.graph(), d@ as int, n@ as int) by {
        if n != next { assert(pending::waiting(before, d, n)); }
    }
}

proof fn after_wake<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, before: &Queues, after: &Queues, emitted: NodeIdx)
    requires sound(t, p, before, Some(emitted)), !after.blocked().contains_key(emitted),
        forall|n: NodeIdx| pending::waiting(before, emitted, n) ==> pending::scheduled(t, p, after, n),
        forall|key: ReadyKey| !pending::waiting(before, emitted, key.1) ==>
            after.ready().contains(key) == before.ready().contains(key),
        forall|d: NodeIdx, n: NodeIdx| !pending::waiting(before, emitted, n) ==>
            pending::waiting(after, d, n) == pending::waiting(before, d, n),
    ensures sound(t, p, after, None),
{
    assert forall|key: ReadyKey| after.ready().contains(key) implies p.contains_key(key.1)
        && pending::eligible(t, p, key.1) && key.0 == t.nodes_view()[key.1@ as int].hash by {
        if pending::waiting(before, emitted, key.1) { assert(pending::scheduled(t, p, after, key.1)); }
        else { assert(before.ready().contains(key)); }
    }
    assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(after, d, n) implies p.contains_key(n)
        && (pending::selected(t, p, d) || None::<NodeIdx> == Some(d))
        && graph::reaches(t.graph(), d@ as int, n@ as int) by {
        if pending::waiting(before, emitted, n) { assert(pending::scheduled(t, p, after, n)); }
        else {
            assert(d != emitted);
            assert(pending::waiting(before, d, n));
        }
    }
}

/// Preserve the joint invariant and emit the smallest eligible hash, or report true exhaustion.
pub(crate) fn next<P>(t: &Topology<P>, p: &mut BTreeMap<NodeIdx, PendingChain>, q: &mut Queues, remaining: &mut usize) -> (out: Option<NodeIdx>)
    requires valid(t, old(p)@, old(q), *old(remaining)),
    ensures valid(t, final(p)@, final(q), *final(remaining)),
        (out is None) == (*old(remaining) == 0),
        match out {
            None => final(p)@ == old(p)@ && *final(remaining) == *old(remaining),
            Some(idx) => idx@ < t.nodes_view().len() && pending::eligible(t, old(p)@, idx)
                && pending::selected_set(t, final(p)@) == pending::selected_set(t, old(p)@).remove(idx)
                && *final(remaining) + 1 == *old(remaining)
                && forall|n: NodeIdx| pending::eligible(t, old(p)@, n) ==>
                    n@ < t.nodes_view().len() && super::stdlib::ready_le((t.nodes_view()[idx@ as int].hash, idx), (t.nodes_view()[n@ as int].hash, n)),
        },
{
    proof { t.established(); progress(t, p@, q, *remaining); }
    let ghost before_p = p@;
    let ghost before_q = *q;
    let ghost before_count = *remaining;
    assert(valid(t, before_p, &before_q, before_count));
    let Some((_hash, idx)) = q.pop_ready() else {
        proof {
            assert(q.ready() == before_q.ready());
            assert(q.blocked() == before_q.blocked());
            assert forall|n: NodeIdx| q.queued(n) == before_q.queued(n) by {};
            assert(p@.dom().is_empty());
            assert forall|d: NodeIdx, n: NodeIdx| pending::waiting(q, d, n) implies false by {
                assert(q.queued(n));
                assert(before_q.queued(n));
                assert(before_p.contains_key(n));
                assert(p@ == before_p);
                assert(!p@.contains_key(n));
            }
            assert(sound(t, p@, q, None));
            assert forall|n: NodeIdx| q.queued(n) == p@.contains_key(n) by {
                assert(q.queued(n) == before_q.queued(n));
                assert(before_q.queued(n) == before_p.contains_key(n));
            }
        }
        return None;
    };
    proof {
        assert(before_q.ready().contains((_hash, idx)));
        assert(before_q.queued(idx));
        assert(before_p.contains_key(idx));
        before_p[idx].facts(t);
        assert(_hash == t.nodes_view()[idx@ as int].hash);
        assert forall|n: NodeIdx| pending::eligible(t, before_p, n) implies
            n@ < t.nodes_view().len() && super::stdlib::ready_le((t.nodes_view()[idx@ as int].hash, idx), (t.nodes_view()[n@ as int].hash, n)) by {
            eligible_ready(t, before_p, &before_q, before_count, n);
        }
    }
    let ghost popped = *q;
    let successor = pending::advance(t, p, idx);
    proof { after_advance(t, before_p, p@, &before_q, q, idx); }
    if let Some(next) = successor {
        proof { assert(p@.contains_key(next)); assert(!q.queued(next)); }
        pending::schedule(t, p, q, next);
        proof { after_schedule(t, p@, &popped, q, next, idx); }
    }
    proof {
        assert forall|n: NodeIdx| q.queued(n) == p@.contains_key(n) by {
            if successor is Some { let next = successor.unwrap(); }
        }
    }
    let ghost before_wake = *q;
    pending::wake(t, p, q, idx);
    proof { after_wake(t, p@, &before_wake, q, idx); }
    *remaining -= 1;
    Some(idx)
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{author::Author, chain_intervals::Intervals, commit_hash::CommitHash};

    #[test]
    fn joint_steps_order_shared_dependencies_count_and_remain_exhausted() {
        let mut t = Topology::new();
        let hashes = [8, 9, 1, 0, 2].map(|b| CommitHash([b; 32]));
        let a = t.insert(hashes[0], Author([1; 32]), &[], (), 0).unwrap();
        let tip = t
            .insert(hashes[1], Author([1; 32]), &[hashes[0]], (), 0)
            .unwrap();
        let b = t
            .insert(hashes[2], Author([2; 32]), &[hashes[1]], (), 0)
            .unwrap();
        let c = t
            .insert(hashes[3], Author([3; 32]), &[hashes[1], hashes[2]], (), 0)
            .unwrap();
        let independent = t.insert(hashes[4], Author([4; 32]), &[], (), 0).unwrap();
        let mut p = BTreeMap::new();
        for (node, end) in [(a, 2), (b, 1), (c, 1), (independent, 1)] {
            p.insert(
                node,
                PendingChain {
                    chain: t.nodes()[node.index()].chain,
                    intervals: Intervals::new(vec![0..end]),
                },
            );
        }
        let mut remaining = 5;
        let mut q = initialize(&t, &p, remaining);
        // The independent root wins on hash; the smaller descendant hashes must wait.
        for (i, expected) in [independent, a, tip, b, c].into_iter().enumerate() {
            assert_eq!(next(&t, &mut p, &mut q, &mut remaining), Some(expected));
            assert_eq!(remaining, 4 - i);
        }
        assert!(p.is_empty());
        assert_eq!(q.retained_counts(), (0, 0));
        for _ in 0..3 {
            assert_eq!(next(&t, &mut p, &mut q, &mut remaining), None);
            assert_eq!(remaining, 0);
        }
        // Empty selections also work on a nonempty history.
        let mut empty = BTreeMap::new();
        let mut empty_count = 0;
        let mut empty_q = initialize(&t, &empty, empty_count);
        assert_eq!(next(&t, &mut empty, &mut empty_q, &mut empty_count), None);
        assert_eq!(empty_count, 0);
    }
}
