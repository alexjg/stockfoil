//! Parent-edge meaning and append stability of the real cached walk's output.
use vstd::prelude::*;
use std::sync::Arc;
use crate::{ancestry_walk::{self, summarizes}, graph, storage::{self, Node},
    storage_refinement::{graph_of, parent_set}, clock::{NodeIdx, below}, tree::Clock,
    chain_selection_refinement::labels, difference::tables_view};

verus! {

pub(crate) proof fn singleton_history(g: graph::Graph, a: int, d: int)
    ensures graph::history(g, ISet::empty().insert(d)).contains(a) == graph::reaches(g, a, d),
{
    let heads = ISet::empty().insert(d);
    if graph::reaches(g, a, d) {
        assert(heads.contains(d));
        assert(exists|h: int| heads.contains(h) && graph::reaches(g, a, h));
    }
    if graph::history(g, heads).contains(a) {
        let h = choose|h: int| heads.contains(h) && graph::reaches(g, a, h);
        assert(h == d);
    }
}

/// Summary construction does not assume that a chain-prefix query is exact.
/// The separately maintained chain invariant supplies precisely that fact.
pub(crate) proof fn exact_membership<H, P, C, A>(clock: &Clock,
    nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>, owners: Seq<A>, heads: ISet<int>)
    requires clock.valid(), graph::chain_invariant(graph_of(nodes, ids), labels(nodes), owners),
        summarizes(clock.contents(), storage::labels(nodes), graph::history(graph_of(nodes, ids), heads)),
    ensures forall|a: usize| a < nodes.len() ==> below(clock.lookup(nodes[a as int].chain), a)
        == graph::history(graph_of(nodes, ids), heads).contains(a as int),
{
    let g = graph_of(nodes, ids);
    let h = graph::history(g, heads);
    assert forall|a: usize| a < nodes.len() implies below(clock.lookup(nodes[a as int].chain), a)
        == h.contains(a as int) by {
        let c = nodes[a as int].chain;
        if below(clock.lookup(c), a) {
            let tip = clock.contents()[c]@;
            assert(nodes[tip as int].chain == c);
            assert(labels(nodes)[a as int] == labels(nodes)[tip as int]);
            assert(graph::reaches(g, a as int, tip as int));
            graph::history_closed(g, heads, a as int, tip as int);
        }
    }
}

/// Previously computed clocks remain correct without rescanning or refreshing
/// them when the packed parent storage and chain-label sequence are extended.
pub(crate) proof fn cache_survives_append<H, P, C>(clock: &Clock,
    nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>, incoming: Seq<NodeIdx>, node: Node<H, P, C>, head: NodeIdx)
    requires storage::linked(nodes, ids), head@ < nodes.len(),
        node.parents_start == ids.len(), node.parents_len == incoming.len(),
        summarizes(clock.contents(), storage::labels(nodes),
            graph::history(graph_of(nodes, ids), ISet::empty().insert(head@ as int))),
    ensures summarizes(clock.contents(), storage::labels(nodes.push(node)),
        graph::history(graph_of(nodes.push(node), ids + incoming), ISet::empty().insert(head@ as int))),
{
    crate::storage_refinement::append_preserves_graph(nodes, ids, incoming, node);
    let before = graph_of(nodes, ids);
    let after = graph_of(nodes.push(node), ids + incoming);
    let heads = ISet::empty().insert(head@ as int);
    assert forall|a: int| graph::history(before, heads).contains(a) == graph::history(after, heads).contains(a) by {
        if graph::history(before, heads).contains(a) {
            let h = choose|h: int| heads.contains(h) && graph::reaches(before, a, h);
            assert(h == head@);
            assert(graph::reaches(after, a, h));
        }
        if graph::history(after, heads).contains(a) {
            let h = choose|h: int| heads.contains(h) && graph::reaches(after, a, h);
            assert(h == head@);
            assert(graph::reaches(before, a, h));
        }
    }
    assert(graph::history(before, heads) =~= graph::history(after, heads));
    assert(storage::labels(nodes.push(node)) =~= storage::labels(nodes).push(node.chain));
}

spec fn fork_graph() -> graph::Graph {
    seq![ISet::empty(), ISet::empty().insert(0int), ISet::empty().insert(0int),
        ISet::empty().insert(1int).insert(2int), ISet::empty()]
}

#[verifier::rlimit(30)]
fn fork_fixture() -> (out: (Vec<Node<u64, u64, ()>>, Vec<NodeIdx>, Vec<NodeIdx>, Vec<Vec<NodeIdx>>))
    ensures storage::valid(out.0@, out.1@, out.2@, tables_view(out.3@)), out.0.len() == 5, out.1.len() == 6,
        graph_of(out.0@, out.1@) == fork_graph(), storage::labels(out.0@) == seq![0u32, 0u32, 1u32, 0u32, 2u32],
{
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<u64, u64, ()>();
    let root = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![], Node::new(10, 100, 0, 0, 1, ()));
    let left = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![root, root], Node::new(20, 200, 0, 0, 2, ()));
    let right = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![root], Node::new(30, 300, 1, 0, 2, ()));
    let merge = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![left, right, left], Node::new(40, 400, 0, 0, 3, ()));
    let unrelated = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &vec![], Node::new(50, 500, 2, 0, 1, ()));
    assert(ids@ =~= seq![root, root, root, left, right, left]);
    assert forall|d: int| 0 <= d < 5 implies graph_of(nodes@, ids@)[d] == fork_graph()[d] by {
        assert forall|p: int| graph_of(nodes@, ids@)[d].contains(p) == fork_graph()[d].contains(p) by {
            if graph_of(nodes@, ids@)[d].contains(p) {
                let node = nodes@[d];
                let j = choose|j: int| node.parents_start <= j < node.parents_start + node.parents_len && ids@[j]@ == p;
                assert(0 <= j < 6);
            }
            if fork_graph()[d].contains(p) {
                if d == 1 { assert(ids@[0]@ == p); }
                if d == 2 { assert(ids@[2]@ == p); }
                if d == 3 && p == 1 { assert(ids@[3]@ == p); }
                if d == 3 && p == 2 { assert(ids@[4]@ == p); }
            }
        }
        assert(graph_of(nodes@, ids@)[d] =~= fork_graph()[d]);
    }
    assert(graph_of(nodes@, ids@) =~= fork_graph());
    assert(storage::labels(nodes@) =~= seq![0u32, 0u32, 1u32, 0u32, 2u32]);
    (nodes, ids, tips, tables)
}

proof fn fork_semantics(a: usize)
    requires a < 5,
    ensures graph::chain_invariant(fork_graph(), seq![0usize, 0usize, 1usize, 0usize, 2usize],
                seq![0usize, 0usize, 1usize, 0usize, 2usize]),
        graph::history(fork_graph(), ISet::empty().insert(3int)).contains(a as int) == (a <= 3),
{
    let g = fork_graph();
    assert forall|x: int, d: int| 0 <= x <= d < 5 && (seq![0usize, 0usize, 1usize, 0usize, 2usize])[x]
        == (seq![0usize, 0usize, 1usize, 0usize, 2usize])[d] implies graph::reaches(g, x, d) by {
        graph::unfold_reaches(g, x, 0);
        graph::unfold_reaches(g, x, 1);
        graph::unfold_reaches(g, x, 2);
        graph::unfold_reaches(g, x, 3);
        graph::unfold_reaches(g, x, 4);
    }
    graph::unfold_reaches(g, a as int, 0);
    graph::unfold_reaches(g, a as int, 1);
    graph::unfold_reaches(g, a as int, 2);
    graph::unfold_reaches(g, a as int, 3);
    assert(ISet::empty().insert(3int).contains(3int));
}

pub fn cached_fork_merge_duplicates_and_append() {
    let (mut nodes, mut ids, mut tips, mut tables) = fork_fixture();
    let root = NodeIdx::new(0);
    let left = NodeIdx::new(1);
    let right = NodeIdx::new(2);
    let merge = NodeIdx::new(3);
    let unrelated = NodeIdx::new(4);
    proof { storage::labels(nodes@).lemma_cardinality_of_set(); }
    let no_cache = |_n: NodeIdx| -> (out: Option<&Clock>) ensures out is None, { None };
    let root_clock = ancestry_walk::compute(&nodes, &ids, root, no_cache);
    let root_cache = |n: NodeIdx| -> (out: Option<&Clock>)
        ensures out == if n@ == root@ { Some(&root_clock) } else { None },
    { if n.index() == root.index() { Some(&root_clock) } else { None } };
    let merge_clock = Arc::new(ancestry_walk::compute(&nodes, &ids, merge, root_cache));
    let other_clock = Arc::new(ancestry_walk::compute(&nodes, &ids, unrelated, no_cache));
    let ghost g = graph_of(nodes@, ids@);
    proof {
        fork_semantics(0); fork_semantics(1); fork_semantics(2); fork_semantics(3); fork_semantics(4);
        assert(labels(nodes@) =~= seq![0usize, 0usize, 1usize, 0usize, 2usize]);
        exact_membership(&merge_clock, nodes@, ids@, seq![0usize, 0usize, 1usize, 0usize, 2usize], ISet::empty().insert(merge@ as int));
    }
    let has_root = merge_clock.contains(0, root);
    let has_left = merge_clock.contains(0, left);
    let has_right = merge_clock.contains(1, right);
    let has_merge = merge_clock.contains(0, merge);
    let has_unrelated = merge_clock.contains(2, unrelated);
    assert(has_root && has_left && has_right && has_merge && !has_unrelated);
    let get = |n: NodeIdx| -> (out: &Arc<Clock>)
        requires n@ == merge@ || n@ == unrelated@,
        ensures out == if n@ == merge@ { &merge_clock } else { &other_clock },
    { if n.index() == merge.index() { &merge_clock } else { &other_clock } };
    let both = ancestry_walk::combine(&nodes, &ids, &vec![merge, unrelated, merge], get);
    let single_heads = vec![merge];
    let singleton = ancestry_walk::combine(&nodes, &ids, &single_heads, get);
    proof {
        assert(single_heads@[0] == merge);
        assert(parent_set(single_heads@).contains(merge@ as int));
        assert(parent_set(single_heads@) =~= ISet::empty().insert(merge@ as int));
        ancestry_walk::summary_unique(singleton.contents(), merge_clock.contents(), storage::labels(nodes@),
            graph::history(g, ISet::empty().insert(merge@ as int)));
    }
    assert(singleton.contents() == merge_clock.contents());
    let empty = ancestry_walk::combine(&nodes, &ids, &vec![], get);
    assert(empty.contents() == Map::<u32, NodeIdx>::empty());
    let ghost prior_nodes = nodes@;
    let ghost prior_ids = ids@;
    let incoming = vec![merge, unrelated, merge];
    let last = storage::append(&mut nodes, &mut ids, &mut tips, &mut tables,
        &incoming, Node::new(60, 600, 0, 0, 4, ()));
    proof {
        cache_survives_append(&merge_clock, prior_nodes, prior_ids, incoming@, nodes@.last(), merge);
        storage::labels(nodes@).lemma_cardinality_of_set();
    }
    let old_cache = |n: NodeIdx| -> (out: Option<&Clock>)
        ensures out == if n@ == merge@ { Some(&*merge_clock) } else { None },
    { if n.index() == merge.index() { Some(&*merge_clock) } else { None } };
    let after = ancestry_walk::compute(&nodes, &ids, last, old_cache);
    assert(after.valid());
}

} // verus!
