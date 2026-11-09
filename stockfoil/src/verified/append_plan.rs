//! Reserve first, then publish packed storage and its indexes without unwinding.
use super::{
    author::Author,
    clock::NodeIdx,
    commit_hash::CommitHash,
    indexes::{AuthorIndex, HashIndex},
    stdlib,
    storage::Node,
};
#[cfg(verus_keep_ghost)]
use super::{chain_tables, difference::tables_view, storage};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) struct Prepared {
    chain_row: Vec<NodeIdx>,
    author_row: Vec<u32>,
}

pub(crate) closed spec fn ready<P, C>(nodes: Vec<Node<CommitHash, P, C>>, ids: Vec<NodeIdx>,
    tips: Vec<NodeIdx>, tables: Vec<Vec<NodeIdx>>, owners: Vec<Author>,
    hashes: HashIndex, authors: AuthorIndex, prepared: Prepared,
    chain: u32, author: Author, incoming: usize) -> bool {
    &&& stdlib::vec_spare(nodes) > 0
    &&& stdlib::vec_spare(ids) >= incoming
    &&& hashes.reserved()
    &&& if chain < tips.len() {
        chain < tables.len() && stdlib::vec_spare(tables[chain as int]) > 0
    } else {
        &&& stdlib::vec_spare(tips) > 0
        &&& stdlib::vec_spare(tables) > 0
        &&& stdlib::vec_spare(owners) > 0
        &&& authors.reserved(author)
        &&& prepared.chain_row@.len() == 0 && stdlib::vec_spare(prepared.chain_row) > 0
        &&& prepared.author_row@.len() == 0 && stdlib::vec_spare(prepared.author_row) > 0
    }
}

/// Reservation can change capacities, never the logical topology or indexes.
/// Failure is returned before the first logical update, even after earlier reserves succeed.
pub(crate) fn prepare<P, C>(nodes: &mut Vec<Node<CommitHash, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    hashes: &mut HashIndex, authors: &mut AuthorIndex,
    chain: u32, author: Author, incoming: usize) -> (out: Option<Prepared>)
    requires chain <= old(tips).len(), old(tips).len() == old(tables).len(),
    ensures final(nodes)@ == old(nodes)@, final(ids)@ == old(ids)@,
        final(tips)@ == old(tips)@, tables_view(final(tables)@) == tables_view(old(tables)@),
        final(owners)@ == old(owners)@, final(hashes).contents() == old(hashes).contents(),
        final(authors).contents() == old(authors).contents(),
        out is Some ==> ready(*final(nodes), *final(ids), *final(tips), *final(tables),
            *final(owners), *final(hashes), *final(authors), out->Some_0, chain, author, incoming),
    no_unwind
{
    let mut chain_row = stdlib::empty_vec();
    let mut author_row = stdlib::empty_vec();
    if !stdlib::reserve_vec(nodes, 1) { return None; }
    if !stdlib::reserve_vec(ids, incoming) { return None; }
    if !hashes.reserve() { return None; }
    if (chain as usize) < tips.len() {
        let ghost before = tables_view(tables@);
        let success = stdlib::reserve_row(tables, chain as usize, 1);
        assert(tables_view(tables@) =~= before);
        if !success { return None; }
    } else {
        if !stdlib::reserve_vec(tips, 1) { return None; }
        if !stdlib::reserve_vec(tables, 1) { return None; }
        if !stdlib::reserve_vec(owners, 1) { return None; }
        if !authors.reserve(author) { return None; }
        if !stdlib::reserve_vec(&mut chain_row, 1) { return None; }
        if !stdlib::reserve_vec(&mut author_row, 1) { return None; }
    }
    Some(Prepared { chain_row, author_row })
}

pub(crate) fn append<P, C>(nodes: &mut Vec<Node<CommitHash, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    hashes: &mut HashIndex, authors: &mut AuthorIndex, incoming: &[NodeIdx],
    node: Node<CommitHash, P, C>, author: Author, prepared: Prepared) -> (position: NodeIdx)
    requires ready(*old(nodes), *old(ids), *old(tips), *old(tables), *old(owners), *old(hashes), *old(authors),
            prepared, node.chain, author, incoming.len()),
        storage::valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
        old(hashes).matches(old(nodes)@), old(authors).matches(old(owners)@),
        !old(hashes).contents().contains_key(node.hash),
        old(owners).len() == old(tips).len(), node.chain <= old(tips).len(),
        node.chain < old(owners).len() ==> old(owners)@[node.chain as int] == author,
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, tables_view(final(tables)@)),
        final(hashes).matches(final(nodes)@), final(authors).matches(final(owners)@),
        final(hashes).contents() == old(hashes).contents().insert(node.hash, position),
        final(authors).contents() == if node.chain < old(owners).len() { old(authors).contents() }
            else { old(authors).contents().insert(author,
                (if old(authors).contents().contains_key(author) { old(authors).contents()[author] } else { Seq::empty() }).push(node.chain)) },
        final(owners).len() == final(tips).len(), position@ == old(nodes).len(),
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, position) } else { old(tips)@.push(position) },
        final(owners)@ == if node.chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        final(owners)@[node.chain as int] == author,
        final(nodes)@ == old(nodes)@.push(storage::with_span(node, old(ids).len(), incoming.len() as u32)),
        final(ids)@ == old(ids)@ + incoming@,
        tables_view(final(tables)@) == super::chain_tables::extended(tables_view(old(tables)@), node.chain, position),
    no_unwind
{
    let ghost old_hashes = *hashes;
    let ghost old_authors = *authors;
    let ghost old_owners = owners@;
    let ghost old_nodes = nodes@;
    let ghost old_ids = ids@;
    let ghost old_tables = tables_view(tables@);
    let ghost linked = storage::with_span(node, ids.len(), incoming.len() as u32);
    let chain = node.chain;
    let hash = node.hash;
    let new_chain = chain as usize == tips.len();
    let position = NodeIdx::new(nodes.len() as u32);
    let mut node = node;
    node.parents_start = ids.len();
    node.parents_len = incoming.len() as u32;
    assert(node == linked);
    let mut i = 0;
    while i < incoming.len()
        invariant i <= incoming.len(), ids@ == old_ids + incoming@.subrange(0, i as int),
            stdlib::vec_spare(*ids) >= incoming.len() - i,
        decreases incoming.len() - i,
    {
        stdlib::push_reserved(ids, incoming[i]);
        i += 1;
        assert(ids@ =~= old_ids + incoming@.subrange(0, i as int));
    }
    assert(ids@ == old_ids + incoming@);
    proof {
        if chain < tips.len() {
            let row = old_tables[chain as int];
            assert(chain_tables::row_valid(row, tips@[chain as int]));
            assert(row.contains(tips@[chain as int]));
            assert(tips@[chain as int]@ < old_nodes.len());
        }
        storage::append_links(old_nodes, old_ids, incoming@, node);
        chain_tables::extension_preserves_layout(old_tables, storage::labels(old_nodes), chain, position);
        chain_tables::extension_valid(tips@, old_tables, chain, position);
    }
    let mut chain_row = prepared.chain_row;
    let author_row = prepared.author_row;
    if new_chain {
        stdlib::push_reserved(tips, position);
        stdlib::push_reserved(&mut chain_row, position);
        assert(chain_row@ =~= seq![position]);
        stdlib::push_reserved(tables, chain_row);
        stdlib::push_reserved(owners, author);
        authors.register_reserved(author, chain, author_row);
        proof { old_authors.registration_matches(old_owners, author, chain, authors); }
    } else {
        stdlib::set_reserved(tips, chain as usize, position);
        stdlib::push_row_reserved(tables, chain as usize, position);
    }
    assert(tables_view(tables@) =~= chain_tables::extended(old_tables, chain, position));
    stdlib::push_reserved(nodes, node);
    assert(storage::labels(nodes@) =~= storage::labels(old_nodes).push(chain));
    hashes.insert_reserved(hash, position);
    proof { old_hashes.append_matches(old_nodes, linked, position, hashes); }
    position
}

} // verus!

#[cfg(test)]
mod tests {
    use super::super::ancestry_cache::{self, Cache};
    use super::super::storage;
    use super::*;

    #[test]
    fn reservation_and_reserved_push_do_not_drop_generic_elements() {
        use std::{cell::Cell, rc::Rc};
        struct Payload(Rc<Cell<usize>>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let mut values = vec![Payload(drops.clone())];
        assert!(!stdlib::reserve_vec(&mut values, usize::MAX));
        assert_eq!(values.len(), 1);
        assert_eq!(drops.get(), 0);
        assert!(stdlib::reserve_vec(&mut values, 1));
        let capacity = values.capacity();
        stdlib::push_reserved(&mut values, Payload(drops.clone()));
        assert_eq!(values.capacity(), capacity);
        assert_eq!(values.len(), 2);
        assert_eq!(drops.get(), 0);
        drop(values);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn failed_preparation_preserves_nonempty_storage_and_allows_retry() {
        let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<CommitHash, u64, Cache>();
        let mut owners = Vec::new();
        let mut hashes = HashIndex::new();
        let mut authors = AuthorIndex::new();
        let alice = Author([1; 32]);
        let root = CommitHash([2; 32]);
        let idx = ancestry_cache::append(
            &mut nodes,
            &mut ids,
            &mut tips,
            &mut tables,
            &mut owners,
            &mut hashes,
            &mut authors,
            &[],
            Node::new(root, 42, 0, 0, 1, ()),
            alice,
        );
        let old_tips = tips.clone();
        let old_tables = tables.clone();
        let old_owners = owners.clone();
        let old_author_row = authors.get(&alice).unwrap().clone();
        // Node reservation succeeds first; this request then deterministically
        // overflows the parent vector's byte capacity without allocating it.
        assert!(
            prepare(
                &mut nodes,
                &mut ids,
                &mut tips,
                &mut tables,
                &mut owners,
                &mut hashes,
                &mut authors,
                0,
                alice,
                usize::MAX
            )
            .is_none()
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!((nodes[0].hash, nodes[0].payload), (root, 42));
        assert!(ids.is_empty());
        assert_eq!(tips, old_tips);
        assert_eq!(tables, old_tables);
        assert_eq!(owners, old_owners);
        assert_eq!(hashes.get(&root), Some(&idx));
        assert_eq!(authors.get(&alice), Some(&old_author_row));

        let next = CommitHash([3; 32]);
        let prepared = prepare(
            &mut nodes,
            &mut ids,
            &mut tips,
            &mut tables,
            &mut owners,
            &mut hashes,
            &mut authors,
            0,
            alice,
            2,
        )
        .unwrap();
        let child = ancestry_cache::append_prepared(
            &mut nodes,
            &mut ids,
            &mut tips,
            &mut tables,
            &mut owners,
            &mut hashes,
            &mut authors,
            &[idx, idx],
            Node::new(next, 43, 0, 0, 2, ()),
            alice,
            prepared,
        );
        assert_eq!(ids, vec![idx, idx]);
        assert_eq!(tables, vec![vec![idx, child]]);
        assert_eq!(tips, vec![child]);
        assert_eq!(hashes.get(&next), Some(&child));
        assert_eq!(authors.get(&alice), Some(&old_author_row));
    }

    #[test]
    fn prepared_new_chains_register_existing_and_new_authors() {
        let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<CommitHash, u64, Cache>();
        let mut owners = Vec::new();
        let mut hashes = HashIndex::new();
        let mut authors = AuthorIndex::new();
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        for (chain, author) in [alice, alice, bob].into_iter().enumerate() {
            let hash = CommitHash([chain as u8; 32]);
            let prepared = prepare(
                &mut nodes,
                &mut ids,
                &mut tips,
                &mut tables,
                &mut owners,
                &mut hashes,
                &mut authors,
                chain as u32,
                author,
                0,
            )
            .unwrap();
            let idx = ancestry_cache::append_prepared(
                &mut nodes,
                &mut ids,
                &mut tips,
                &mut tables,
                &mut owners,
                &mut hashes,
                &mut authors,
                &[],
                Node::new(hash, chain as u64, chain as u32, 0, 1, ()),
                author,
                prepared,
            );
            assert_eq!(tables[chain], vec![idx]);
            assert_eq!(tips[chain], idx);
            assert_eq!(owners[chain], author);
            assert_eq!(hashes.get(&hash), Some(&idx));
        }
        assert_eq!(authors.get(&alice).unwrap(), &[0, 1]);
        assert_eq!(authors.get(&bob).unwrap(), &[2]);
    }
}
