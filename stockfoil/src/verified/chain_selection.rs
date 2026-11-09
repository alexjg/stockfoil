//! The production direct-parent and ancestry-tip chain searches.
use super::{author::Author, clock::NodeIdx, storage::Node, tree::Clock};
#[cfg(verus_keep_ghost)]
use super::{difference::tables_view, storage};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn direct_candidate<H, P, C>(nodes: Seq<Node<H, P, C>>, tips: Seq<NodeIdx>,
    owners: Seq<Author>, parents: Seq<NodeIdx>, author: Author, chain: u32) -> bool {
    chain < tips.len() && chain < owners.len() && owners[chain as int] == author
        && exists|i: int| 0 <= i < parents.len() && parents[i]@ < nodes.len()
            && nodes[parents[i]@ as int].chain == chain && tips[chain as int] == parents[i]
}

/// Choose the lowest eligible chain ID, independently of parent ordering.
pub(crate) fn direct<H, P, C>(nodes: &[Node<H, P, C>], tips: &[NodeIdx], owners: &[Author],
    parents: &[NodeIdx], author: &Author) -> (out: Option<u32>)
    requires owners.len() == tips.len(),
        forall|i: int| 0 <= i < parents.len() ==> parents[i]@ < nodes.len()
            && nodes[parents[i]@ as int].chain < tips.len(),
    ensures match out {
        Some(c) => direct_candidate(nodes@, tips@, owners@, parents@, *author, c)
            && forall|d: u32| direct_candidate(nodes@, tips@, owners@, parents@, *author, d) ==> c <= d,
        None => forall|c: u32| !direct_candidate(nodes@, tips@, owners@, parents@, *author, c),
    },
{
    let mut best: Option<u32> = None;
    let mut i = 0;
    while i < parents.len()
        invariant i <= parents.len(), owners.len() == tips.len(),
            forall|j: int| 0 <= j < parents.len() ==> parents[j]@ < nodes.len()
                && nodes[parents[j]@ as int].chain < tips.len(),
            match best {
                Some(c) => direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c)
                    && forall|d: u32| direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, d) ==> c <= d,
                None => forall|c: u32| !direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c),
            },
        decreases parents.len() - i,
    {
        let parent = parents[i];
        let chain = nodes[parent.index()].chain;
        let tip = tips[chain as usize];
        let matches = owners[chain as usize].same_as(author) && tip.index() == parent.index();
        proof { if tip@ == parent@ { NodeIdx::same_index(tip, parent); } }
        assert(matches == (owners@[chain as int] == *author && tips@[chain as int] == parent));
        proof {
            assert forall|c: u32| direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, c)
                == (direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c)
                    || (c == chain && matches)) by {
                if direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, c) {
                    let j = choose|j: int| 0 <= j < i + 1 && parents@[j]@ < nodes.len()
                        && nodes@[parents@[j]@ as int].chain == c && tips@[c as int] == parents@[j];
                    if j < i { assert(parents@.subrange(0, i as int)[j] == parents@[j]); }
                }
                if direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c) {
                    let j = choose|j: int| 0 <= j < i && parents@[j]@ < nodes.len()
                        && nodes@[parents@[j]@ as int].chain == c && tips@[c as int] == parents@[j];
                    assert(parents@.subrange(0, i + 1)[j] == parents@[j]);
                }
                if c == chain && matches {
                    assert(parents@.subrange(0, i + 1)[i as int] == parent);
                }
            }
        }
        let ghost previous = best;
        if matches {
            best = match best { None => Some(chain), Some(c) => Some(if chain < c { chain } else { c }) };
        }
        proof {
            match best {
                Some(c) => {
                    if c == chain && matches {
                        assert(parents@.subrange(0, i + 1)[i as int] == parent);
                        assert(nodes@[parent@ as int].chain == chain);
                        assert(tips@[chain as int] == parent && owners@[chain as int] == *author);
                        assert(exists|j: int| #![trigger parents@.subrange(0, i + 1)[j]] 0 <= j < parents@.subrange(0, i + 1).len()
                            && parents@.subrange(0, i + 1)[j]@ < nodes.len()
                            && nodes@[parents@.subrange(0, i + 1)[j]@ as int].chain == chain
                            && tips@[chain as int] == parents@.subrange(0, i + 1)[j]);
                        assert(direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, chain));
                    } else {
                        assert(previous == Some(c));
                        assert(direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c));
                    }
                    assert(direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, c));
                    assert forall|d: u32| #[trigger] direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, d)
                        implies c <= d by {
                        if direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, d) {
                            assert(previous is Some);
                            assert(previous.unwrap() <= d);
                            assert(c <= previous.unwrap());
                        } else {
                            assert(d == chain && matches);
                            assert(c <= chain);
                        }
                    }
                },
                None => {
                    assert(previous is None && !matches);
                    assert forall|c: u32| !(#[trigger] direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i + 1), *author, c)) by {
                        assert(!direct_candidate(nodes@, tips@, owners@, parents@.subrange(0, i as int), *author, c));
                    }
                },
            }
        }
        i += 1;
    }
    assert(parents@.subrange(0, i as int) == parents@);
    best
}

/// Preserve the author's chain-list order and return its first covered tip.
/// The caller must establish that the supplied list belongs to this author.
pub(crate) fn ancestral(chains: &[u32], tips: &[NodeIdx], reached: &Clock) -> (out: Option<u32>)
    requires reached.valid(),
        forall|i: int| 0 <= i < chains.len() ==> chains[i] < tips.len(),
    ensures match out {
        Some(c) => c < tips.len()
            && super::clock::below(reached.lookup(c), tips[c as int]@ as usize)
            && exists|i: int| 0 <= i < chains.len() && chains[i] == c
                && forall|j: int| 0 <= j < i ==> !super::clock::below(reached.lookup(chains[j]), tips[chains[j] as int]@ as usize),
        None => forall|i: int| 0 <= i < chains.len() ==>
            !super::clock::below(reached.lookup(chains[i]), tips[chains[i] as int]@ as usize),
    },
{
    let mut i = 0;
    while i < chains.len()
        invariant i <= chains.len(), reached.valid(),
            forall|j: int| 0 <= j < chains.len() ==> chains[j] < tips.len(),
            forall|j: int| 0 <= j < i ==>
                !super::clock::below(reached.lookup(chains[j]), tips[chains[j] as int]@ as usize),
        decreases chains.len() - i,
    {
        let chain = chains[i];
        if reached.contains(chain, tips[chain as usize]) {
            assert(exists|j: int| 0 <= j < chains.len() && chains[j] == chain
                && forall|k: int| 0 <= k < j ==> !super::clock::below(reached.lookup(chains[k]), tips[chains[k] as int]@ as usize));
            return Some(chain);
        }
        i += 1;
    }
    None
}

/// Record a new chain's author, or preserve an existing same-author entry.
#[cfg(any(test, verus_keep_ghost))]
pub(crate) fn register(owners: &mut Vec<Author>, chain: u32, author: Author)
    requires chain <= old(owners).len(),
        chain < old(owners).len() ==> old(owners)@[chain as int] == author,
    ensures final(owners)@ == if chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        chain < final(owners).len(), final(owners)@[chain as int] == author,
{
    if chain as usize == owners.len() { owners.push(author); }
}

/// Append topology and its chain-author entry together. The selector must
/// establish same-author ancestral reuse; this function preserves its choice.
#[cfg(any(test, verus_keep_ghost))]
pub(crate) fn append<H, P, C>(nodes: &mut Vec<Node<H, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    incoming: &[NodeIdx], node: Node<H, P, C>, author: Author) -> (position: NodeIdx)
    requires storage::valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
        old(owners).len() == old(tips).len(), node.chain <= old(tips).len(),
        node.chain < old(owners).len() ==> old(owners)@[node.chain as int] == author,
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, tables_view(final(tables)@)),
        final(owners).len() == final(tips).len(), position@ == old(nodes).len(),
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, position) } else { old(tips)@.push(position) },
        final(owners)@ == if node.chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        final(owners)@[node.chain as int] == author,
        final(nodes)@ == old(nodes)@.push(storage::with_span(node, old(ids).len(), incoming.len() as u32)),
        final(ids)@ == old(ids)@ + incoming@,
        tables_view(final(tables)@) == super::chain_tables::extended(tables_view(old(tables)@), node.chain, position),
{
    register(owners, node.chain, author);
    super::storage::append(nodes, ids, tips, tables, incoming, node)
}

} // verus!
