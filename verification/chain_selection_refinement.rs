//! Connect the real selectors and append to the stored parent-edge graph.
use vstd::prelude::*;
use crate::{author::Author, clock::{NodeIdx, below}, storage::{self, Node}, chain_selection,
    chain_tables, difference::tables_view, graph, tree::Clock};
use crate::storage_refinement::{graph_of, parent_set, append_preserves_graph};

verus! {

pub(crate) open spec fn labels<H, P, C>(nodes: Seq<Node<H, P, C>>) -> Seq<usize> {
    nodes.map(|i: int, n: Node<H, P, C>| n.chain as usize)
}

pub(crate) open spec fn authors<H, P, C>(nodes: Seq<Node<H, P, C>>, owners: Seq<Author>) -> Seq<Author> {
    nodes.map(|i: int, n: Node<H, P, C>| owners[n.chain as int])
}

pub(crate) open spec fn sound<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    tips: Seq<NodeIdx>, owners: Seq<Author>, parents: Seq<NodeIdx>, chain: u32, author: Author) -> bool {
    &&& chain <= tips.len()
    &&& chain < tips.len() ==> owners[chain as int] == author
        && graph::history(graph_of(nodes, ids), parent_set(parents)).contains(tips[chain as int]@ as int)
}

pub(crate) proof fn node_chain_bound<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    tips: Seq<NodeIdx>, tables: Seq<Seq<NodeIdx>>, i: int)
    requires storage::valid(nodes, ids, tips, tables), 0 <= i < nodes.len(),
    ensures nodes[i].chain < tips.len(),
{
    let index = NodeIdx::spec_new(i as u32);
    NodeIdx::spec_new_index(i as u32);
    assert(index@ == i);
    assert(storage::labels(nodes)[i] < tables.len());
}

pub(crate) proof fn tip_is_maximal<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    tips: Seq<NodeIdx>, tables: Seq<Seq<NodeIdx>>, chain: u32)
    requires storage::valid(nodes, ids, tips, tables), chain < tips.len(),
    ensures tips[chain as int]@ < nodes.len(), nodes[tips[chain as int]@ as int].chain == chain,
        forall|i: int| 0 <= i < nodes.len() && nodes[i].chain == chain ==> i <= tips[chain as int]@,
{
    let row = tables[chain as int];
    assert(chain_tables::row_valid(row, tips[chain as int]));
    assert(row.contains(tips[chain as int]));
    assert forall|i: int| 0 <= i < nodes.len() && nodes[i].chain == chain implies i <= tips[chain as int]@ by {
        let index = NodeIdx::spec_new(i as u32);
        NodeIdx::spec_new_index(i as u32);
        assert(row.contains(index));
        let j = choose|j: int| 0 <= j < row.len() && row[j] == index;
        if j < row.len() - 1 { assert(row[j]@ < row.last()@); }
    }
}

pub(crate) fn direct_for_graph<H, P, C>(nodes: &Vec<Node<H, P, C>>, ids: &Vec<NodeIdx>,
    tips: &Vec<NodeIdx>, tables: &Vec<Vec<NodeIdx>>, owners: &Vec<Author>,
    parents: &[NodeIdx], author: &Author) -> (out: Option<u32>)
    requires storage::valid(nodes@, ids@, tips@, tables_view(tables@)), owners.len() == tips.len(),
        forall|i: int| 0 <= i < parents.len() ==> parents[i]@ < nodes.len(),
    ensures out is Some ==> sound(nodes@, ids@, tips@, owners@, parents@, out.unwrap(), *author),
        match out {
            Some(c) => chain_selection::direct_candidate(nodes@, tips@, owners@, parents@, *author, c)
                && forall|d: u32| chain_selection::direct_candidate(nodes@, tips@, owners@, parents@, *author, d) ==> c <= d,
            None => forall|c: u32| !chain_selection::direct_candidate(nodes@, tips@, owners@, parents@, *author, c),
        },
{
    proof {
        assert forall|i: int| 0 <= i < parents.len() implies parents[i]@ < nodes.len()
            && nodes@[parents[i]@ as int].chain < tips.len() by {
            node_chain_bound(nodes@, ids@, tips@, tables_view(tables@), parents[i]@ as int);
        }
    }
    let out = chain_selection::direct(nodes, tips, owners, parents, author);
    proof {
        if let Some(c) = out {
            let j = choose|j: int| 0 <= j < parents.len() && parents[j]@ < nodes.len()
                && nodes@[parents[j]@ as int].chain == c && tips@[c as int] == parents[j];
            let tip = tips@[c as int]@ as int;
            assert(parent_set(parents@).contains(tip));
            graph::unfold_reaches(graph_of(nodes@, ids@), tip, tip);
            assert(graph::history(graph_of(nodes@, ids@), parent_set(parents@)).contains(tip));
        }
    }
    out
}

pub(crate) fn ancestral_for_graph<H, P, C>(nodes: &Vec<Node<H, P, C>>, ids: &Vec<NodeIdx>,
    tips: &Vec<NodeIdx>, owners: &Vec<Author>, chains: &[u32], parents: &[NodeIdx],
    author: &Author, reached: &Clock) -> (out: Option<u32>)
    requires reached.valid(), owners.len() == tips.len(),
        forall|i: int| 0 <= i < chains.len() ==> chains[i] < tips.len() && owners@[chains[i] as int] == *author,
        forall|c: u32| c < tips.len() ==> below(reached.lookup(c), tips@[c as int]@ as usize)
            == graph::history(graph_of(nodes@, ids@), parent_set(parents@)).contains(tips@[c as int]@ as int),
    ensures out is Some ==> sound(nodes@, ids@, tips@, owners@, parents@, out.unwrap(), *author),
        out is None ==> forall|i: int| 0 <= i < chains.len() ==> !below(reached.lookup(chains[i]), tips@[chains[i] as int]@ as usize),
        out is Some ==> chains@.contains(out.unwrap()),
{
    let out = chain_selection::ancestral(chains, tips, reached);
    proof {
        if let Some(c) = out {
            let i = choose|i: int| 0 <= i < chains.len() && chains[i] == c;
            assert(owners@[c as int] == *author);
        }
    }
    out
}

/// Discharge the maximal-tip/fresh-chain premises of graph::extend_chain from
/// the concrete stored tables and the checked selector's semantic result.
pub(crate) proof fn extend_selected<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>, tips: Seq<NodeIdx>,
    tables: Seq<Seq<NodeIdx>>, owners: Seq<Author>, parents: Seq<NodeIdx>, node: Node<H, P, C>, author: Author)
    requires storage::valid(nodes, ids, tips, tables), owners.len() == tips.len(),
        graph::chain_invariant(graph_of(nodes, ids), labels(nodes), authors(nodes, owners)),
        sound(nodes, ids, tips, owners, parents, node.chain, author),
        forall|i: int| 0 <= i < parents.len() ==> parents[i]@ < nodes.len(),
        node.parents_start == ids.len(), node.parents_len == parents.len(),
    ensures graph::chain_invariant(graph_of(nodes.push(node), ids + parents), labels(nodes.push(node)),
        authors(nodes.push(node), if node.chain < owners.len() { owners } else { owners.push(author) })),
{
    let g = graph_of(nodes, ids);
    let chain = node.chain;
    let reuse = if chain < tips.len() { Some(tips[chain as int]@ as int) } else { None };
    if chain < tips.len() {
        tip_is_maximal(nodes, ids, tips, tables, chain);
    } else {
        assert forall|i: int| 0 <= i < nodes.len() implies labels(nodes)[i] != chain by {
            node_chain_bound(nodes, ids, tips, tables, i);
        }
    }
    assert forall|p: int| parent_set(parents).contains(p) implies 0 <= p < nodes.len() by {
        let j = choose|j: int| 0 <= j < parents.len() && parents[j]@ == p;
    }
    graph::extend_chain(g, labels(nodes), authors(nodes, owners), parent_set(parents), chain as usize, author, reuse);
    append_preserves_graph(nodes, ids, parents, node);
    let next_owners = if chain < owners.len() { owners } else { owners.push(author) };
    assert(labels(nodes.push(node)) =~= labels(nodes).push(chain as usize));
    assert forall|i: int| 0 <= i < nodes.push(node).len() implies
        authors(nodes.push(node), next_owners)[i] == authors(nodes, owners).push(author)[i] by {
        if i < nodes.len() { node_chain_bound(nodes, ids, tips, tables, i); }
    }
    assert(authors(nodes.push(node), next_owners) =~= authors(nodes, owners).push(author));
}

pub(crate) fn append_for_graph<H, P, C>(nodes: &mut Vec<Node<H, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    parents: &[NodeIdx], node: Node<H, P, C>, author: Author) -> (out: NodeIdx)
    requires storage::valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
        old(owners).len() == old(tips).len(),
        graph::chain_invariant(graph_of(old(nodes)@, old(ids)@), labels(old(nodes)@), authors(old(nodes)@, old(owners)@)),
        sound(old(nodes)@, old(ids)@, old(tips)@, old(owners)@, parents@, node.chain, author),
        old(nodes).len() <= u32::MAX, parents.len() <= u32::MAX,
        old(ids).len() + parents.len() <= usize::MAX,
        forall|i: int| 0 <= i < parents.len() ==> parents[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, tables_view(final(tables)@)),
        final(owners).len() == final(tips).len(),
        final(owners)@ == if node.chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        final(nodes)@ == old(nodes)@.push(storage::with_span(node, old(ids).len(), parents.len() as u32)),
        final(ids)@ == old(ids)@ + parents@, out@ == old(nodes).len(),
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, out) } else { old(tips)@.push(out) },
        tables_view(final(tables)@) == chain_tables::extended(tables_view(old(tables)@), node.chain, out),
        graph::chain_invariant(graph_of(final(nodes)@, final(ids)@), labels(final(nodes)@), authors(final(nodes)@, final(owners)@)),
{
    proof {
        extend_selected(nodes@, ids@, tips@, tables_view(tables@), owners@, parents@,
            storage::with_span(node, ids.len(), parents.len() as u32), author);
    }
    chain_selection::append(nodes, ids, tips, tables, owners, parents, node, author)
}

pub fn direct_reuse_and_merge_witness() {
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<u64, u64, ()>();
    let mut owners = Vec::new();
    let alice = Author([1u8; 32]);
    let empty = Vec::new();
    assert(graph::chain_invariant(graph_of(nodes@, ids@), labels(nodes@), authors(nodes@, owners@)));
    let root = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &empty, Node::new(10, 100, 0, 0, 1, ()), alice);
    let ps = vec![root];
    let selected = direct_for_graph(&nodes, &ids, &tips, &tables, &owners, &ps, &alice);
    assert(chain_selection::direct_candidate(nodes@, tips@, owners@, ps@, alice, 0));
    assert(selected == Some(0));
    let child = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &ps, Node::new(20, 200, selected.unwrap(), 0, 2, ()), alice);
    let stale = chain_selection::direct(&nodes, &tips, &owners, &ps, &alice);
    assert(stale is None);
    let other = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &empty, Node::new(30, 300, 1, 0, 1, ()), alice);
    let ps = vec![other, child, other];
    let selected = direct_for_graph(&nodes, &ids, &tips, &tables, &owners, &ps, &alice);
    assert(chain_selection::direct_candidate(nodes@, tips@, owners@, ps@, alice, 0));
    assert(selected == Some(0));
    let merge = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &ps, Node::new(40, 400, selected.unwrap(), 0, 3, ()), alice);
    assert(nodes@[merge@ as int].chain == nodes@[root@ as int].chain);
    assert(graph::reaches(graph_of(nodes@, ids@), root@ as int, merge@ as int));
}

pub fn indirect_tip_search_witness() {
    let alice = Author([1u8; 32]);
    let bob = Author([2u8; 32]);
    assert(alice.0@[0] != bob.0@[0]);
    assert(alice != bob);
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<u64, u64, ()>();
    let mut owners = Vec::new();
    let empty = Vec::new();
    assert(graph::chain_invariant(graph_of(nodes@, ids@), labels(nodes@), authors(nodes@, owners@)));
    let a = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &empty, Node::new(10, 100, 0, 0, 1, ()), alice);
    let b = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &vec![a], Node::new(20, 200, 1, 0, 2, ()), bob);
    let ps = vec![b];
    let no_direct = direct_for_graph(&nodes, &ids, &tips, &tables, &owners, &ps, &alice);
    assert forall|c: u32| !(#[trigger] chain_selection::direct_candidate(nodes@, tips@, owners@, ps@, alice, c)) by {
        if chain_selection::direct_candidate(nodes@, tips@, owners@, ps@, alice, c) {
            let j = choose|j: int| 0 <= j < ps.len() && ps[j]@ < nodes.len()
                && nodes@[ps[j]@ as int].chain == c && tips@[c as int] == ps[j];
            assert(j == 0 && c == 1);
        }
    }
    assert(no_direct is None);
    let mut reached = Clock::default();
    reached.include(0, a);
    reached.include(1, b);
    proof {
        let g = graph_of(nodes@, ids@);
        assert(g[1].contains(0));
        graph::unfold_reaches(g, 0, 0);
        graph::unfold_reaches(g, 0, 1);
        graph::unfold_reaches(g, 1, 1);
        assert(parent_set(ps@).contains(1));
        assert(graph::history(g, parent_set(ps@)).contains(0));
        assert(graph::history(g, parent_set(ps@)).contains(1));
    }
    let selected = ancestral_for_graph(&nodes, &ids, &tips, &owners, &vec![0], &ps, &alice, &reached);
    assert(selected == Some(0));
    let next = append_for_graph(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &ps, Node::new(30, 300, selected.unwrap(), 0, 3, ()), alice);
    assert(graph::reaches(graph_of(nodes@, ids@), a@ as int, next@ as int));
    // The first candidate-list position wins, even if chain IDs are unsorted.
    let both = vec![1, 0];
    let prior_tips = vec![a, b];
    let first = chain_selection::ancestral(&both, &prior_tips, &reached);
    assert(first == Some(1));
    let none = chain_selection::ancestral(&both, &prior_tips, &Clock::default());
    assert(none is None);
}

} // verus!
