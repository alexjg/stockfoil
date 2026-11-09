//! Conditions still to be maintained by the production pending-chain scheduler.
//! Queue operations are production code; this module adds conditional progress/order theorems.
use vstd::prelude::*;
use crate::{clock::NodeIdx, commit_hash::CommitHash, schedule_queues::{self, Queues, ReadyKey}, graph};

verus! {

pub(crate) open spec fn complete_frontiers(queues: &Queues, frontiers: Set<NodeIdx>) -> bool {
    &&& queues.valid()
    &&& forall|n: NodeIdx| frontiers.contains(n) == queues.queued(n)
    // A dependency may be later than its own chain's frontier. It must still
    // have a pending frontier at or before it, rather than name an emitted node.
    &&& forall|d: NodeIdx| queues.blocked().contains_key(d) ==> exists|f: NodeIdx| frontiers.contains(f) && f@ <= d@
}

pub(crate) proof fn minimum(frontiers: Set<NodeIdx>) -> (out: NodeIdx)
    requires !frontiers.is_empty(),
    ensures frontiers.contains(out), forall|n: NodeIdx| frontiers.contains(n) ==> out@ <= n@,
    decreases frontiers.len(),
{
    let n = frontiers.choose();
    if frontiers.remove(n).is_empty() { n }
    else {
        let other = minimum(frontiers.remove(n));
        if n@ <= other@ { n } else { other }
    }
}

pub(crate) proof fn some_frontier_ready(queues: &Queues, frontiers: Set<NodeIdx>)
    requires complete_frontiers(queues, frontiers), !frontiers.is_empty(),
    ensures !queues.ready().is_empty(),
{
    let first = minimum(frontiers);
    if queues.ready().is_empty() {
        assert(queues.queued(first));
        let dependency = choose|d: NodeIdx| queues.blocked().contains_key(d) && queues.blocked()[d].contains(first);
        let i = choose|i: int| 0 <= i < queues.blocked()[dependency].len() && queues.blocked()[dependency][i] == first;
        assert(dependency@ < first@);
        let f = choose|f: NodeIdx| frontiers.contains(f) && f@ <= dependency@;
        assert(first@ <= f@);
    }
}

pub(crate) open spec fn eligible(g: graph::Graph, selected: Set<NodeIdx>, node: NodeIdx) -> bool {
    selected.contains(node) && forall|ancestor: NodeIdx| selected.contains(ancestor) && ancestor != node ==>
        !graph::reaches(g, ancestor@ as int, node@ as int)
}

pub(crate) open spec fn ready_matches(queues: &Queues, g: graph::Graph,
    selected: Set<NodeIdx>, hashes: Map<NodeIdx, CommitHash>) -> bool
{
    &&& selected.subset_of(hashes.dom())
    &&& forall|key: ReadyKey| queues.ready().contains(key) ==> eligible(g, selected, key.1) && key.0 == hashes[key.1]
    &&& forall|n: NodeIdx| eligible(g, selected, n) ==> queues.ready().contains((hashes[n], n))
}

/// The actual queue pop has the public ordering needed once the scheduler
/// establishes complete frontier coverage and exact causal readiness.
pub(crate) fn pop_with_scheduler_invariant(queues: &mut Queues,
    Ghost(frontiers): Ghost<Set<NodeIdx>>, Ghost(g): Ghost<graph::Graph>,
    Ghost(selected): Ghost<Set<NodeIdx>>, Ghost(hashes): Ghost<Map<NodeIdx, CommitHash>>)
    -> (out: Option<ReadyKey>)
    requires complete_frontiers(old(queues), frontiers), !frontiers.is_empty(),
        ready_matches(old(queues), g, selected, hashes),
    ensures out is Some, eligible(g, selected, out.unwrap().1), out.unwrap().0 == hashes[out.unwrap().1],
        forall|n: NodeIdx| eligible(g, selected, n) ==> crate::stdlib::ready_le(out.unwrap(), (hashes[n], n)),
        !final(queues).queued(out.unwrap().1), final(queues).valid(),
{
    proof { some_frontier_ready(queues, frontiers); }
    queues.pop_ready()
}

pub(crate) proof fn ready_stays_ready(g: graph::Graph, before: Set<NodeIdx>, after: Set<NodeIdx>, node: NodeIdx)
    requires eligible(g, before, node), after.subset_of(before), after.contains(node),
    ensures eligible(g, after, node),
{}

pub(crate) proof fn selected_dependency_blocks(g: graph::Graph, selected: Set<NodeIdx>, dependency: NodeIdx, waiter: NodeIdx)
    requires selected.contains(dependency), selected.contains(waiter), dependency != waiter,
        graph::reaches(g, dependency@ as int, waiter@ as int),
    ensures !eligible(g, selected, waiter),
{}

/// Concrete use of the actual queue transitions, including two waiters on one
/// dependency, reblocking after wakeup, and hashes ordered differently from IDs.
pub fn ready_order_and_waiter_lifecycle() {
    let a = NodeIdx::new(0);
    let b = NodeIdx::new(1);
    let c = NodeIdx::new(3);
    let d = NodeIdx::new(4);
    let high = CommitHash([255u8; 32]);
    let low = CommitHash([0u8; 32]);
    assert(high.0@[0] == 255 && low.0@[0] == 0);
    assert(high != low);
    let mut queues = Queues::new();
    queues.make_ready(high, a);
    queues.make_ready(low, b);
    queues.block(a, c);
    queues.block(a, d);
    proof {
        let frontiers = set![a, b, c, d];
        assert forall|dependency: NodeIdx| queues.blocked().contains_key(dependency) implies
            exists|f: NodeIdx| frontiers.contains(f) && f@ <= dependency@ by {
            assert(dependency == a);
            assert(frontiers.contains(a) && a@ <= dependency@);
        }
        assert(complete_frontiers(&queues, frontiers));
        some_frontier_ready(&queues, frontiers);
    }
    assert(queues.ready().contains((low, b)));
    let first = queues.pop_ready();
    assert(first is Some);
    assert(crate::stdlib::ready_le(first.unwrap(), (low, b)));
    assert(!crate::stdlib::hash_less(high, low));
    assert(first == Some((low, b)));
    let none = queues.take_waiters(b);
    assert(none@.len() == 0);
    let second = queues.pop_ready();
    assert(second == Some((high, a)));
    let waiters = queues.take_waiters(a);
    assert(waiters@ == seq![c, d]);
    assert(!queues.queued(c) && !queues.queued(d));
    queues.make_ready(low, c);
    queues.block(c, d);
    let third = queues.pop_ready();
    assert(third == Some((low, c)));
    let last_waiter = queues.take_waiters(c);
    assert(last_waiter@ == seq![d]);
    queues.make_ready(high, d);
    let fourth = queues.pop_ready();
    assert(fourth == Some((high, d)));
    let no_more = queues.pop_ready();
    assert(no_more is None);
    let duplicate_wakeup = queues.take_waiters(a);
    assert(duplicate_wakeup@.len() == 0);
}

pub fn equal_hash_tie_breaks_by_node() {
    let mut queues = Queues::new();
    let hash = CommitHash([7u8; 32]);
    let a = NodeIdx::new(2);
    let b = NodeIdx::new(8);
    queues.make_ready(hash, b);
    queues.make_ready(hash, a);
    assert(queues.ready().contains((hash, a)));
    let first = queues.pop_ready();
    assert(first is Some);
    assert(crate::stdlib::ready_le(first.unwrap(), (hash, a)));
    assert(!crate::stdlib::hash_less(hash, hash));
    assert(first == Some((hash, a)));
    let second = queues.pop_ready();
    assert(second == Some((hash, b)));
}

} // verus!
