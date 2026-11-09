//! Stored hash/author indexes discharge resolution and selector-list premises.
use vstd::prelude::*;
use crate::{author::Author, clock::{NodeIdx, below}, commit_hash::CommitHash,
    indexes::{self, HashIndex, AuthorIndex}, storage::{self, Node}, tree::Clock,
    difference::tables_view, graph};
use crate::storage_refinement::{graph_of, parent_set};
use crate::chain_selection_refinement::{self as selection, labels, authors, sound};

verus! {

pub(crate) fn resolve_for_nodes<P, C>(index: &HashIndex, nodes: &Vec<Node<CommitHash, P, C>>,
    hashes: &[CommitHash]) -> (out: Result<Vec<NodeIdx>, CommitHash>)
    requires index.matches(nodes@),
    ensures match out {
        Ok(indices) => indices.len() == hashes.len()
            && forall|i: int| #![trigger indices@[i]] 0 <= i < indices.len() ==>
                indices@[i]@ < nodes.len() && nodes@[indices@[i]@ as int].hash == hashes@[i],
        Err(hash) => hashes@.contains(hash)
            && forall|i: int| 0 <= i < nodes.len() ==> nodes@[i].hash != hash,
    },
{
    match index.resolve(hashes) {
        Ok(indices) => {
            proof {
                assert forall|i: int| #![trigger indices@[i]] 0 <= i < indices.len() implies
                    indices@[i]@ < nodes.len() && nodes@[indices@[i]@ as int].hash == hashes@[i] by {
                    assert(i < hashes.len());
                    assert(indices@[i] == index.contents()[hashes@[i]]);
                    assert(index.contents().contains_key(hashes@[i]));
                }
            }
            Ok(indices)
        },
        Err(hash) => {
            proof {
                assert forall|i: int| 0 <= i < nodes.len() implies nodes@[i].hash != hash by {
                    assert(index.contents().contains_key(nodes@[i].hash));
                }
            }
            Err(hash)
        },
    }
}

pub(crate) fn fallback_for_graph<P, C>(index: &AuthorIndex, nodes: &Vec<Node<CommitHash, P, C>>,
    ids: &Vec<NodeIdx>, tips: &Vec<NodeIdx>, owners: &Vec<Author>, parents: &[NodeIdx],
    author: &Author, reached: &Clock) -> (out: Option<u32>)
    requires index.matches(owners@), owners.len() == tips.len(), reached.valid(),
        forall|c: u32| c < tips.len() ==> below(reached.lookup(c), tips@[c as int]@ as usize)
            == graph::history(graph_of(nodes@, ids@), parent_set(parents@)).contains(tips@[c as int]@ as int),
    ensures out is Some ==> sound(nodes@, ids@, tips@, owners@, parents@, out.unwrap(), *author),
        out is None ==> forall|c: u32| c < owners.len() && owners@[c as int] == *author ==>
            !graph::history(graph_of(nodes@, ids@), parent_set(parents@)).contains(tips@[c as int]@ as int),
{
    match index.get(author) {
        None => None,
        Some(chains) => {
            let out = selection::ancestral_for_graph(nodes, ids, tips, owners, chains, parents, author, reached);
            proof {
                if out is None {
                    assert forall|c: u32| c < owners.len() && owners@[c as int] == *author implies
                        !graph::history(graph_of(nodes@, ids@), parent_set(parents@)).contains(tips@[c as int]@ as int) by {
                        assert(chains@.contains(c));
                        let i = choose|i: int| 0 <= i < chains.len() && chains@[i] == c;
                        assert(!below(reached.lookup(c), tips@[c as int]@ as usize));
                    }
                }
            }
            out
        },
    }
}

pub(crate) fn append_for_graph<P, C>(nodes: &mut Vec<Node<CommitHash, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    hashes: &mut HashIndex, index: &mut AuthorIndex, incoming: &[NodeIdx],
    node: Node<CommitHash, P, C>, author: Author) -> (position: NodeIdx)
    requires storage::valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
        old(hashes).matches(old(nodes)@), old(index).matches(old(owners)@),
        !old(hashes).contents().contains_key(node.hash),
        old(owners).len() == old(tips).len(),
        graph::chain_invariant(graph_of(old(nodes)@, old(ids)@), labels(old(nodes)@), authors(old(nodes)@, old(owners)@)),
        sound(old(nodes)@, old(ids)@, old(tips)@, old(owners)@, incoming@, node.chain, author),
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, tables_view(final(tables)@)),
        final(hashes).matches(final(nodes)@), final(index).matches(final(owners)@),
        final(owners).len() == final(tips).len(), position@ == old(nodes).len(),
        graph::chain_invariant(graph_of(final(nodes)@, final(ids)@), labels(final(nodes)@), authors(final(nodes)@, final(owners)@)),
{
    proof {
        selection::extend_selected(nodes@, ids@, tips@, tables_view(tables@), owners@, incoming@,
            storage::with_span(node, ids.len(), incoming.len() as u32), author);
    }
    indexes::append(nodes, ids, tips, tables, owners, hashes, index, incoming, node, author)
}

pub fn indexed_interleaved_authors_and_duplicate_parents() {
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<CommitHash, u64, ()>();
    let mut owners = Vec::new();
    let mut hashes = HashIndex::new();
    let mut index = AuthorIndex::new();
    let alice = Author([1u8; 32]);
    let bob = Author([2u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let h2 = CommitHash([2u8; 32]);
    let h3 = CommitHash([3u8; 32]);
    let missing = CommitHash([255u8; 32]);
    assert(alice.0@[0] == 1 && bob.0@[0] == 2);
    assert(alice != bob);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && h2.0@[0] == 2 && h3.0@[0] == 3 && missing.0@[0] == 255);
    assert(h0 != h1 && h0 != h2 && h0 != h3 && h1 != h2 && h1 != h3 && h2 != h3);
    let empty = Vec::new();
    assert(hashes.matches(nodes@));
    let root = indexes::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners, &mut hashes, &mut index,
        &empty, Node::new(h0, 100, 0, 0, 1, ()), alice);
    let child = indexes::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners, &mut hashes, &mut index,
        &vec![root, root], Node::new(h1, 200, 0, 0, 2, ()), alice);
    let other = indexes::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners, &mut hashes, &mut index,
        &empty, Node::new(h2, 300, 1, 0, 1, ()), bob);
    let fork = indexes::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners, &mut hashes, &mut index,
        &vec![root], Node::new(h3, 400, 2, 0, 2, ()), alice);
    let order = hashes.resolve(&vec![h1, h0, h1, h2, h3]).unwrap();
    assert(order@ =~= seq![child, root, child, other, fork]);
    let alice_chains = index.get(&alice).unwrap();
    let bob_chains = index.get(&bob).unwrap();
    assert(alice_chains@ =~= seq![0u32, 2u32]);
    assert(bob_chains@ =~= seq![1u32]);
    assert(!hashes.contents().contains_key(missing));
    let absent = hashes.resolve(&vec![h0, missing]);
    assert(absent is Err);
    let known = resolve_for_nodes(&hashes, &nodes, &vec![h0, h3]);
    assert(known is Ok);
}

} // verus!
