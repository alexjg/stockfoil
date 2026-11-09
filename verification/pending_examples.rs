//! Nonempty construction of the actual topology, pending map and scheduling steps.
use vstd::prelude::*;
use std::{collections::BTreeMap, ops::Range};
use crate::{topology::Topology, author::Author, commit_hash::CommitHash, clock::NodeIdx,
    chain_intervals::Intervals, pending::{self, PendingChain}, schedule_queues::Queues};

verus! {

pub(crate) fn singleton_pending(topology: &Topology<u64>, root: NodeIdx) -> (pending: BTreeMap<NodeIdx, PendingChain>)
    requires topology.valid(), topology.nodes_view().len() == 1, root@ == 0,
    ensures pending::valid(topology, pending@), pending@.dom() == Set::empty().insert(root),
        forall|n: NodeIdx| pending::selected(topology, pending@, n) == (n == root),
{
    let chain = topology.nodes()[root.index()].chain;
    assert(chain < topology.tables_view().len() && topology.tables_view()[chain as int].len() == 1
        && topology.tables_view()[chain as int][0] == root) by {
        topology.traversal_facts(); topology.traversal_node(root);
        let row = topology.tables_view()[chain as int]@;
        assert(row.len() > 0);
        assert(row[0]@ == 0);
        NodeIdx::same_index(row[0], root);
        if row.len() > 1 {
            assert(row[1]@ == 0);
            assert(row[0]@ < row[1]@);
        }
        assert(row.len() == 1);
    }
    let ranges = vec![Range { start: 0usize, end: 1usize }];
    let ghost input = ranges@;
    assert(input[0].start == 0 && input[0].end == 1);
    assert forall|p: int| crate::chain_intervals::selected(input, p) == (p == 0) by {
        if p == 0 { assert(input[0].start <= p < input[0].end); }
    }
    let intervals = Intervals::new(ranges);
    proof { intervals.facts(); }
    assert forall|p: int| intervals.contains(p) == (p == 0) by {};
    assert(intervals.contains(0));
    let empty = intervals.is_empty();
    assert(!empty);
    let first = intervals.first();
    assert(first == 0);
    let entry = PendingChain { chain, intervals };
    assert(entry.valid(topology) && entry.frontier(topology) == root);
    let mut pending = BTreeMap::new();
    proof { crate::stdlib::node_order_model(); }
    pending.insert(root, entry);
    assert(pending::valid(topology, pending@));
    assert(entry.selected(topology, root));
    assert(pending@.contains_key(root));
    assert(pending@[root].selected(topology, root));
    assert(pending::selected(topology, pending@, root));
    proof {
        assert forall|n: NodeIdx| pending::selected(topology, pending@, n) implies n == root by {
            let f = choose|f: NodeIdx| pending@.contains_key(f) && pending@[f].selected(topology, n);
            let p = choose|p: int| entry.intervals.contains(p) && topology.tables_view()[chain as int][p] == n;
            assert(f == root && p == 0);
        }
    }
    pending
}

pub fn constructed_frontier_is_emitted_and_retired() {
    let mut topology = Topology::new();
    let hash = CommitHash([7u8; 32]);
    let root = crate::topology_examples::add(&mut topology, hash, Author([1u8; 32]), &vec![]);
    let mut pending = singleton_pending(&topology, root);
    let mut queues = Queues::new();
    pending::schedule(&topology, &pending, &mut queues, root);
    assert(pending::eligible(&topology, pending@, root));
    let key = queues.pop_ready().unwrap();
    assert(key.1 == root);
    let next = pending::advance(&topology, &mut pending, root);
    assert(next is None) by {
        if next is Some {
            let n = next.unwrap();
            assert(pending@.contains_key(n));
            pending@[n].facts(&topology);
            assert(pending::selected(&topology, pending@, n));
        }
    }
    assert(pending@.dom().is_empty());
    assert(pending::selected_set(&topology, pending@).is_empty());
    pending::wake(&topology, &pending, &mut queues, root);
    let done = queues.pop_ready();
    assert(done is None);
}
}
