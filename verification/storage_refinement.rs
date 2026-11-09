//! The parent-edge graph and chain labels are projections of stored nodes.
use vstd::prelude::*;
use crate::clock::NodeIdx;
use crate::storage::{self, Node, parents};
use crate::{graph, difference::tables_view};

verus! {

pub(crate) open spec fn graph_of<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>) -> graph::Graph {
    nodes.map(|i: int, node: Node<H, P, C>|
        ISet::new(|p: int| exists|j: int| node.parents_start <= j < node.parents_start + node.parents_len
            && ids[j]@ == p))
}

pub open spec fn parent_set(ids: Seq<NodeIdx>) -> ISet<int> {
    ISet::new(|p: int| exists|j: int| 0 <= j < ids.len() && ids[j]@ == p)
}

proof fn parent_span_set<H, P, C>(node: Node<H, P, C>, ids: Seq<NodeIdx>)
    requires node.parents_start + node.parents_len <= ids.len(),
    ensures parent_set(parents(node, ids)) == ISet::new(|p: int|
        exists|j: int| node.parents_start <= j < node.parents_start + node.parents_len && ids[j]@ == p),
{
    let ps = parents(node, ids);
    let edge = ISet::new(|p: int| exists|j: int|
        node.parents_start <= j < node.parents_start + node.parents_len && ids[j]@ == p);
    assert forall|p: int| parent_set(ps).contains(p) == edge.contains(p) by {
        if parent_set(ps).contains(p) {
            let j = choose|j: int| 0 <= j < ps.len() && ps[j]@ == p;
            assert(ids[node.parents_start + j]@ == p);
            assert(edge.contains(p));
        }
        if edge.contains(p) {
            let j = choose|j: int| node.parents_start <= j < node.parents_start + node.parents_len && ids[j]@ == p;
            assert(ps[j - node.parents_start]@ == p);
            assert(parent_set(ps).contains(p));
        }
    }
    assert(parent_set(ps) =~= edge);
}

pub(crate) proof fn stored_parent_set<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>, n: int)
    requires storage::linked(nodes, ids), 0 <= n < nodes.len(),
    ensures parent_set(parents(nodes[n], ids)) == graph_of(nodes, ids)[n],
{
    parent_span_set(nodes[n], ids);
}

/// Appending storage adds exactly one graph vertex and leaves every old path
/// unchanged. This is about topology, not the correctness of cached clocks.
pub(crate) proof fn append_preserves_graph<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    incoming: Seq<NodeIdx>, node: Node<H, P, C>)
    requires storage::linked(nodes, ids),
        node.parents_start == ids.len(), node.parents_len == incoming.len(),
    ensures graph_of(nodes.push(node), ids + incoming)
            == graph_of(nodes, ids).push(parent_set(incoming)),
        forall|a: int, d: int| 0 <= d < nodes.len() ==>
            graph::reaches(graph_of(nodes.push(node), ids + incoming), a, d)
                == graph::reaches(graph_of(nodes, ids), a, d),
{
    let before = graph_of(nodes, ids);
    let after = graph_of(nodes.push(node), ids + incoming);
    assert forall|d: int| 0 <= d < after.len() implies
        after[d] == before.push(parent_set(incoming))[d] by {
        assert forall|p: int| after[d].contains(p)
            == before.push(parent_set(incoming))[d].contains(p) by {
            if d < nodes.len() {
                let n = nodes[d];
                if after[d].contains(p) {
                    let j = choose|j: int| n.parents_start <= j < n.parents_start + n.parents_len
                        && (ids + incoming)[j]@ == p;
                    assert(j < ids.len());
                    assert(ids[j]@ == p);
                }
                if before[d].contains(p) {
                    let j = choose|j: int| n.parents_start <= j < n.parents_start + n.parents_len
                        && ids[j]@ == p;
                    assert((ids + incoming)[j]@ == p);
                }
            } else {
                if after[d].contains(p) {
                    let j = choose|j: int| node.parents_start <= j < node.parents_start + node.parents_len
                        && (ids + incoming)[j]@ == p;
                    assert(incoming[j - ids.len()]@ == p);
                }
                if parent_set(incoming).contains(p) {
                    let j = choose|j: int| 0 <= j < incoming.len() && incoming[j]@ == p;
                    assert((ids + incoming)[ids.len() + j]@ == p);
                }
            }
        }
        assert(after[d] =~= before.push(parent_set(incoming))[d]);
    }
    assert(after =~= before.push(parent_set(incoming)));
    assert forall|a: int, d: int| 0 <= d < nodes.len() implies
        graph::reaches(after, a, d) == graph::reaches(before, a, d) by {
        graph::append_preserves_reachability(before, parent_set(incoming), a, d);
    }
}

pub(crate) proof fn stored_graph_is_dag<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>)
    requires storage::linked(nodes, ids),
    ensures graph::dag(graph_of(nodes, ids)),
{
    assert forall|d: int, p: int| 0 <= d < nodes.len() && graph_of(nodes, ids)[d].contains(p)
        implies 0 <= p < d by {
        let node = nodes[d];
        let j = choose|j: int| node.parents_start <= j < node.parents_start + node.parents_len && ids[j]@ == p;
        assert(ids[j]@ < d);
    }
}

pub fn stored_fork_and_merge() {
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<u64, u64, ()>();
    let empty: Vec<NodeIdx> = Vec::new();
    assert(storage::valid(nodes@, ids@, tips@, tables_view(tables@)));
    let root = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &empty, Node::new(10, 100, 0, 0, 1, ()));
    let left = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![root], Node::new(20, 200, 0, 0, 2, ()));
    let right = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![root], Node::new(30, 300, 1, 0, 2, ()));
    let ghost before_nodes = nodes@;
    let ghost before_ids = ids@;
    let merge = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![left, right, left], Node::new(40, 400, 0, 0, 3, ()));
    let ps = storage::parent_slice(&nodes[merge.index()], &ids);
    assert(ps@ =~= seq![left, right, left]);
    assert(nodes@[0].hash == 10 && nodes@[0].payload == 100);
    assert(nodes@[3].hash == 40 && nodes@[3].payload == 400);
    assert(parents(nodes@[0], ids@).len() == 0);
    proof {
        stored_graph_is_dag(nodes@, ids@);
        append_preserves_graph(before_nodes, before_ids, seq![left, right, left], nodes@.last());
    }
    assert(graph_of(nodes@, ids@)[3].contains(1));
    assert(graph_of(nodes@, ids@)[3].contains(2));
    assert(!graph_of(nodes@, ids@)[3].contains(3));
    assert(crate::chain_tables::table_layout(tables_view(tables@), storage::labels(nodes@)));
}

} // verus!
