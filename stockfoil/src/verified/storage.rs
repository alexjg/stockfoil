//! Concrete append-only node storage and its packed parent/chain tables.
//! Hashes, payload IDs and caches are opaque: these proofs concern topology.
use super::chain_tables;
use super::clock::NodeIdx;
#[cfg(verus_keep_ghost)]
use super::difference::tables_view;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[derive(Debug)]
pub(crate) struct Node<H, P, C> {
    pub(crate) hash: H,
    pub(crate) payload: P,
    pub(crate) parents_start: usize,
    pub(crate) parents_len: u32,
    pub(crate) chain: u32,
    pub(crate) depth: u8,
    pub(crate) admitted: bool,
    /// Member of some admitted fragment certificate.
    pub(crate) in_fragment: bool,
    /// Longest path since a node with an eagerly cached clock.
    pub(crate) uncached_depth: u8,
    pub(crate) clock: C,
}

pub(crate) open spec fn labels<H, P, C>(nodes: Seq<Node<H, P, C>>) -> Seq<u32> {
    nodes.map(|i: int, node: Node<H, P, C>| node.chain)
}

pub(crate) open spec fn parents<H, P, C>(node: Node<H, P, C>, ids: Seq<NodeIdx>) -> Seq<NodeIdx> {
    ids.subrange(node.parents_start as int, node.parents_start + node.parents_len)
}

pub(crate) open spec fn linked<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>) -> bool {
    &&& forall|i: int| #![trigger nodes[i]] 0 <= i < nodes.len() ==> {
        let n = nodes[i];
        &&& n.parents_start + n.parents_len <= ids.len()
        &&& n.parents_start == if i == 0 { 0int }
            else { nodes[i - 1].parents_start + nodes[i - 1].parents_len }
        &&& forall|j: int| n.parents_start <= j < n.parents_start + n.parents_len
            ==> ids[j]@ < i
    }
    &&& ids.len() == if nodes.len() == 0 { 0int }
        else { nodes.last().parents_start + nodes.last().parents_len }
}

pub(crate) open spec fn valid<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    tips: Seq<NodeIdx>, tables: Seq<Seq<NodeIdx>>) -> bool {
    &&& nodes.len() <= u32::MAX as nat + 1
    &&& linked(nodes, ids)
    &&& chain_tables::valid(tips, tables)
    &&& chain_tables::table_layout(tables, labels(nodes))
}

pub(crate) fn empty<H, P, C>() -> (out: (Vec<Node<H, P, C>>, Vec<NodeIdx>, Vec<NodeIdx>, Vec<Vec<NodeIdx>>))
    ensures out.0.len() == 0, out.1.len() == 0, out.2.len() == 0, out.3.len() == 0,
        valid(out.0@, out.1@, out.2@, tables_view(out.3@)),
{
    (Vec::new(), Vec::new(), Vec::new(), Vec::new())
}

/// Build an unlinked node. Append assigns its packed parent span.
impl<H, P, C> Node<H, P, C> {
    pub(crate) fn new(hash: H, payload: P, chain: u32, depth: u8, uncached_depth: u8, clock: C) -> (out: Self)
        ensures out.hash == hash, out.payload == payload, out.chain == chain,
            out.depth == depth, out.uncached_depth == uncached_depth, out.clock == clock,
            !out.admitted, !out.in_fragment, out.parents_start == 0, out.parents_len == 0,
    {
        Self { hash, payload, chain, depth, uncached_depth, clock,
            admitted: false, in_fragment: false, parents_start: 0, parents_len: 0 }
    }
}

pub(crate) open spec fn with_span<H, P, C>(node: Node<H, P, C>, start: usize, len: u32) -> Node<H, P, C> {
    Node { parents_start: start, parents_len: len, ..node }
}

proof fn append_links<H, P, C>(nodes: Seq<Node<H, P, C>>, ids: Seq<NodeIdx>,
    incoming: Seq<NodeIdx>, node: Node<H, P, C>)
    requires linked(nodes, ids), node.parents_start == ids.len(), node.parents_len == incoming.len(),
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < nodes.len(),
    ensures linked(nodes.push(node), ids + incoming),
        forall|i: int| 0 <= i < nodes.len() ==> parents(nodes[i], ids + incoming) == parents(nodes[i], ids),
        parents(node, ids + incoming) == incoming,
{
    assert forall|i: int| #![trigger nodes.push(node)[i]] 0 <= i < nodes.push(node).len() implies {
        let n = nodes.push(node)[i];
        &&& n.parents_start + n.parents_len <= (ids + incoming).len()
        &&& n.parents_start == if i == 0 { 0int }
            else { nodes.push(node)[i - 1].parents_start + nodes.push(node)[i - 1].parents_len }
        &&& forall|j: int| n.parents_start <= j < n.parents_start + n.parents_len
            ==> (ids + incoming)[j]@ < i
    } by {
        if i < nodes.len() {
            assert forall|j: int| nodes[i].parents_start <= j < nodes[i].parents_start + nodes[i].parents_len
                implies (ids + incoming)[j]@ < i by { assert(j < ids.len()); }
        } else {
            assert forall|j: int| node.parents_start <= j < node.parents_start + node.parents_len
                implies (ids + incoming)[j]@ < i by {
                assert((ids + incoming)[j] == incoming[j - ids.len()]);
            }
        }
    }
    assert forall|i: int| 0 <= i < nodes.len() implies
        parents(nodes[i], ids + incoming) == parents(nodes[i], ids) by {
        assert(parents(nodes[i], ids + incoming) =~= parents(nodes[i], ids));
    }
    assert(parents(node, ids + incoming) =~= incoming);
}

/// The actual History node/parent/table writes. Selection of a same-author
/// ancestral chain and hash resolution remain caller obligations.
pub(crate) fn append<H, P, C>(nodes: &mut Vec<Node<H, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, incoming: &[NodeIdx],
    mut node: Node<H, P, C>) -> (position: NodeIdx)
    requires valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        node.chain <= old(tips).len(),
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures valid(final(nodes)@, final(ids)@, final(tips)@, tables_view(final(tables)@)),
        position@ == old(nodes).len(),
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, position) } else { old(tips)@.push(position) },
        final(nodes)@ == old(nodes)@.push(with_span(node, old(ids).len(), incoming.len() as u32)),
        final(ids)@ == old(ids)@ + incoming@,
        tables_view(final(tables)@) == chain_tables::extended(tables_view(old(tables)@), node.chain, position),
        forall|i: int| 0 <= i < old(nodes).len() ==>
            parents(final(nodes)@[i], final(ids)@) == parents(old(nodes)@[i], old(ids)@),
        parents(final(nodes)@.last(), final(ids)@) == incoming@,
{
    let position = NodeIdx::new(nodes.len() as u32);
    let ghost prior_nodes = nodes@;
    let ghost prior_ids = ids@;
    let ghost prior_tables = tables_view(tables@);
    let ghost original = node;
    let chain = node.chain;
    node.parents_start = ids.len();
    node.parents_len = incoming.len() as u32;
    assert(node == with_span(original, prior_ids.len() as usize, incoming.len() as u32));
    let mut i = 0;
    while i < incoming.len()
        invariant i <= incoming.len(), ids@ == prior_ids + incoming@.subrange(0, i as int),
            prior_ids.len() + incoming.len() <= usize::MAX,
        decreases incoming.len() - i,
    {
        ids.push(incoming[i]);
        i += 1;
        assert(ids@ =~= prior_ids + incoming@.subrange(0, i as int));
    }
    assert(ids@ == prior_ids + incoming@);
    proof {
        if chain < tips.len() {
            let row = prior_tables[chain as int];
            assert(chain_tables::row_valid(row, tips@[chain as int]));
            assert(row.contains(tips@[chain as int]));
            assert(tips@[chain as int]@ < prior_nodes.len());
        }
        append_links(prior_nodes, prior_ids, incoming@, node);
        chain_tables::extension_preserves_layout(prior_tables, labels(prior_nodes), chain, position);
    }
    chain_tables::append(tips, tables, chain, position);
    nodes.push(node);
    assert(labels(nodes@) =~= labels(prior_nodes).push(chain));
    position
}

pub(crate) fn parent_slice<'a, H, P, C>(node: &Node<H, P, C>, ids: &'a [NodeIdx]) -> (out: &'a [NodeIdx])
    requires node.parents_start + node.parents_len <= ids.len(),
    ensures out@ == parents(*node, ids@),
{
    &ids[node.parents_start..node.parents_start + node.parents_len as usize]
}

} // verus!
