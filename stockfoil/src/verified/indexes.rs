//! Hash resolution and author-to-chain indexing against actual stored nodes.
#[cfg(any(test, verus_keep_ghost))]
use super::storage::Node;
use super::{author::Author, clock::NodeIdx, commit_hash::CommitHash};
#[cfg(verus_keep_ghost)]
use super::{difference::tables_view, storage};
use std::collections::HashMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[derive(Debug)]
pub(crate) struct HashIndex {
    entries: HashMap<CommitHash, NodeIdx>,
}

impl HashIndex {
    pub(crate) closed spec fn contents(&self) -> Map<CommitHash, NodeIdx> { self.entries@ }

    pub(crate) closed spec fn reserved(&self) -> bool { super::stdlib::hash_spare(self.entries) > 0 }

    pub(crate) fn reserve(&mut self) -> (out: bool)
        ensures final(self).contents() == old(self).contents(), out ==> final(self).reserved(),
        no_unwind
    { super::stdlib::reserve_hashes(&mut self.entries) }

    pub(crate) fn insert_reserved(&mut self, hash: CommitHash, position: NodeIdx)
        requires old(self).reserved(),
        ensures final(self).contents() == old(self).contents().insert(hash, position),
        no_unwind
    { super::stdlib::insert_hash_reserved(&mut self.entries, hash, position); }

    pub(crate) open spec fn matches<P, C>(&self, nodes: Seq<Node<CommitHash, P, C>>) -> bool {
        &&& forall|hash: CommitHash| self.contents().contains_key(hash) ==>
            self.contents()[hash]@ < nodes.len() && nodes[self.contents()[hash]@ as int].hash == hash
        &&& forall|i: int| #![trigger nodes[i]] 0 <= i < nodes.len() ==>
            self.contents().contains_key(nodes[i].hash) && self.contents()[nodes[i].hash]@ == i
    }

    pub(crate) fn new() -> (out: Self)
        ensures out.contents() == Map::<CommitHash, NodeIdx>::empty(),
    {
        Self { entries: HashMap::new() }
    }

    pub(crate) fn get<'a>(&'a self, hash: &CommitHash) -> (out: Option<&'a NodeIdx>)
        ensures match out {
            Some(idx) => self.contents().contains_key(*hash) && *idx == self.contents()[*hash],
            None => !self.contents().contains_key(*hash),
        },
    {
        proof { super::stdlib::commit_key_model(); }
        self.entries.get(hash)
    }

    pub(crate) fn contains_key(&self, hash: &CommitHash) -> (out: bool)
        ensures out == self.contents().contains_key(*hash),
    {
        proof { super::stdlib::commit_key_model(); }
        self.entries.contains_key(hash)
    }

    pub(crate) fn delivered(&self, hash: &CommitHash) -> (out: NodeIdx)
        requires self.contents().contains_key(*hash),
        ensures out == self.contents()[*hash],
    {
        *self.get(hash).expect("delivered commit")
    }

    #[cfg(any(test, verus_keep_ghost))]
    pub(crate) fn insert(&mut self, hash: CommitHash, position: NodeIdx)
        requires !old(self).contents().contains_key(hash),
        ensures final(self).contents() == old(self).contents().insert(hash, position),
    {
        proof { super::stdlib::commit_key_model(); }
        self.entries.insert(hash, position);
    }

    /// Preserve input order and duplicates; reject an unknown hash explicitly.
    pub(crate) fn resolve(&self, hashes: &[CommitHash]) -> (out: Result<Vec<NodeIdx>, CommitHash>)
        ensures match out {
            Ok(indices) => indices@ == hashes@.map(|i: int, h: CommitHash| self.contents()[h])
                && forall|i: int| #![trigger hashes@[i]] 0 <= i < hashes.len() ==> self.contents().contains_key(hashes@[i]),
            Err(hash) => hashes@.contains(hash) && !self.contents().contains_key(hash),
        },
    {
        let ghost requested = hashes@;
        let ghost map = self.contents();
        let mut indices = Vec::with_capacity(hashes.len());
        let mut i = 0;
        while i < hashes.len()
            invariant i <= hashes.len(), hashes@ == requested, self.contents() == map,
                indices@ == requested.subrange(0, i as int).map(|j: int, h: CommitHash| map[h]),
                forall|j: int| 0 <= j < i ==> map.contains_key(requested[j]),
            decreases hashes.len() - i,
        {
            let hash = hashes[i];
            match self.get(&hash) {
                Some(idx) => indices.push(*idx),
                None => { assert(requested.contains(hash)); return Err(hash); },
            }
            i += 1;
            assert(indices@ =~= requested.subrange(0, i as int).map(|j: int, h: CommitHash| map[h]));
        }
        assert forall|j: int| #![trigger indices@[j]] 0 <= j < hashes.len() implies
            self.contents().contains_key(hashes@[j]) && indices@[j] == self.contents()[hashes@[j]] by {
            assert(indices@[j] == map[requested[j]]);
        }
        Ok(indices)
    }

    pub(crate) proof fn append_matches<P, C>(&self, nodes: Seq<Node<CommitHash, P, C>>,
        node: Node<CommitHash, P, C>, position: NodeIdx, next: &Self)
        requires self.matches(nodes), !self.contents().contains_key(node.hash),
            position@ == nodes.len(), next.contents() == self.contents().insert(node.hash, position),
        ensures next.matches(nodes.push(node)),
    {
        assert forall|hash: CommitHash| next.contents().contains_key(hash) implies
            next.contents()[hash]@ < nodes.push(node).len()
                && nodes.push(node)[next.contents()[hash]@ as int].hash == hash by {
            if hash != node.hash { assert(self.contents().contains_key(hash)); }
        }
        assert forall|i: int| #![trigger nodes.push(node)[i]] 0 <= i < nodes.push(node).len() implies
            next.contents().contains_key(nodes.push(node)[i].hash)
                && next.contents()[nodes.push(node)[i].hash]@ == i by {
            if i < nodes.len() { assert(nodes[i].hash != node.hash); }
        }
    }
}

#[derive(Debug)]
pub(crate) struct AuthorIndex {
    entries: HashMap<Author, Vec<u32>>,
}

impl AuthorIndex {
    pub(crate) closed spec fn contents(&self) -> Map<Author, Seq<u32>> {
        Map::new(self.entries@.dom(), |a: Author| self.entries@[a]@)
    }

    pub(crate) closed spec fn reserved(&self, author: Author) -> bool {
        super::stdlib::hash_spare(self.entries) > 0
            && (self.entries@.contains_key(author) ==> super::stdlib::vec_spare(self.entries@[author]) > 0)
    }

    pub(crate) fn reserve(&mut self, author: Author) -> (out: bool)
        ensures final(self).contents() == old(self).contents(), out ==> final(self).reserved(author),
        no_unwind
    {
        let ghost before = self.contents();
        let out = super::stdlib::reserve_author(&mut self.entries, author);
        assert(self.contents() =~= before);
        out
    }

    pub(crate) fn register_reserved(&mut self, author: Author, chain: u32, empty: Vec<u32>)
        requires old(self).reserved(author), empty@.len() == 0, super::stdlib::vec_spare(empty) > 0,
        ensures final(self).contents() == old(self).contents().insert(author,
            (if old(self).contents().contains_key(author) { old(self).contents()[author] } else { Seq::empty() }).push(chain)),
        no_unwind
    {
        let ghost before = self.contents();
        super::stdlib::push_author_reserved(&mut self.entries, author, chain, empty);
        assert(self.contents() =~= before.insert(author,
            (if before.contains_key(author) { before[author] } else { Seq::empty() }).push(chain)));
    }

    pub(crate) open spec fn matches(&self, owners: Seq<Author>) -> bool {
        &&& forall|a: Author| self.contents().contains_key(a) <==>
            exists|c: int| 0 <= c < owners.len() && owners[c] == a
        &&& forall|a: Author| self.contents().contains_key(a) ==> {
            let row = self.contents()[a];
            &&& forall|i: int| 0 <= i < row.len() ==> row[i] < owners.len() && owners[row[i] as int] == a
            &&& forall|c: int| 0 <= c < owners.len() && owners[c] == a ==> row.contains(c as u32)
            &&& forall|i: int, j: int| 0 <= i < j < row.len() ==> row[i] < row[j]
        }
    }

    pub(crate) fn new() -> (out: Self)
        ensures out.contents() == Map::<Author, Seq<u32>>::empty(), out.matches(Seq::empty()),
    {
        Self { entries: HashMap::new() }
    }

    pub(crate) fn get<'a>(&'a self, author: &Author) -> (out: Option<&'a Vec<u32>>)
        ensures match out {
            Some(row) => self.contents().contains_key(*author) && row@ == self.contents()[*author],
            None => !self.contents().contains_key(*author),
        },
    {
        proof { super::stdlib::author_key_model(); }
        self.entries.get(author)
    }

    /// Called exactly once when allocating the next contiguous chain ID.
    #[cfg(any(test, verus_keep_ghost))]
    pub(crate) fn register(&mut self, author: Author, chain: u32)
        ensures final(self).contents() == old(self).contents().insert(author,
            (if old(self).contents().contains_key(author) { old(self).contents()[author] } else { Seq::empty() }).push(chain)),
    {
        proof { super::stdlib::author_key_model(); }
        let ghost before = self.contents();
        let row = self.entries.entry(author).or_insert(Vec::new());
        row.push(chain);
        assert(self.contents() =~= before.insert(author,
            (if before.contains_key(author) { before[author] } else { Seq::empty() }).push(chain)));
    }

    pub(crate) proof fn registration_matches(&self, owners: Seq<Author>, author: Author, chain: u32, next: &Self)
        requires self.matches(owners), owners.len() == chain,
            next.contents() == self.contents().insert(author,
                (if self.contents().contains_key(author) { self.contents()[author] } else { Seq::empty() }).push(chain)),
        ensures next.matches(owners.push(author)),
    {
        broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
        let old = self.contents();
        let row = if old.contains_key(author) { old[author] } else { Seq::empty() };
        assert forall|a: Author| next.contents().contains_key(a) <==>
            exists|c: int| 0 <= c < owners.push(author).len() && owners.push(author)[c] == a by {
            if a == author { assert(owners.push(author)[owners.len() as int] == a); }
            else if next.contents().contains_key(a) {
                let c = choose|c: int| 0 <= c < owners.len() && owners[c] == a;
                assert(owners.push(author)[c] == a);
            }
        }
        assert forall|a: Author| next.contents().contains_key(a) implies {
            let r = next.contents()[a];
            &&& forall|i: int| 0 <= i < r.len() ==> r[i] < owners.push(author).len() && owners.push(author)[r[i] as int] == a
            &&& forall|c: int| 0 <= c < owners.push(author).len() && owners.push(author)[c] == a ==> r.contains(c as u32)
            &&& forall|i: int, j: int| 0 <= i < j < r.len() ==> r[i] < r[j]
        } by {
            if a == author {
                assert forall|i: int, j: int| 0 <= i < j < row.push(chain).len() implies row.push(chain)[i] < row.push(chain)[j] by {
                    if j == row.len() { assert(row[i] < owners.len()); }
                }
            }
        }
    }
}

/// Publish the concrete storage and both indexes in one checked operation.
/// Parent resolution and ancestry-based chain choice are separate checked inputs.
#[cfg(any(test, verus_keep_ghost))]
pub(crate) fn append<P, C>(nodes: &mut Vec<Node<CommitHash, P, C>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<Author>,
    hashes: &mut HashIndex, authors: &mut AuthorIndex, incoming: &[NodeIdx],
    node: Node<CommitHash, P, C>, author: Author) -> (position: NodeIdx)
    requires storage::valid(old(nodes)@, old(ids)@, old(tips)@, tables_view(old(tables)@)),
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
{
    let ghost old_hashes = *hashes;
    let ghost old_authors = *authors;
    let ghost old_owners = owners@;
    let ghost old_nodes = nodes@;
    let ghost linked = storage::with_span(node, ids.len(), incoming.len() as u32);
    let new_chain = node.chain as usize == tips.len();
    let chain = node.chain;
    let hash = node.hash;
    let position = super::chain_selection::append(nodes, ids, tips, tables, owners, incoming, node, author);
    if new_chain {
        authors.register(author, chain);
        proof { old_authors.registration_matches(old_owners, author, chain, authors); }
    }
    hashes.insert(hash, position);
    proof { old_hashes.append_matches(old_nodes, linked, position, hashes); }
    position
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_resolution_distinguishes_every_byte_and_preserves_repeats() {
        let mut index = HashIndex::new();
        let mut keys = Vec::new();
        for byte in 0..32 {
            let mut bytes = [0; 32];
            bytes[byte] = 1;
            let key = CommitHash(bytes);
            index.insert(key, NodeIdx::new(byte as u32));
            keys.push(key);
        }
        let requested: Vec<_> = keys.iter().rev().chain(keys.iter()).copied().collect();
        let resolved: Vec<_> = index
            .resolve(&requested)
            .unwrap()
            .iter()
            .map(|n| n.index())
            .collect();
        let expected: Vec<usize> = (0..32).rev().chain(0..32).collect();
        assert_eq!(resolved, expected);
        assert!(index.resolve(&[]).unwrap().is_empty());
        let unknown = CommitHash([0; 32]);
        assert_eq!(index.resolve(&[keys[0], unknown, keys[1]]), Err(unknown));
        assert!(index.get(&unknown).is_none());
        assert!(!index.contains_key(&unknown));
    }

    #[test]
    fn author_chains_remain_separate_and_in_allocation_order() {
        let mut index = AuthorIndex::new();
        for round in 0..3 {
            for byte in 0..32 {
                let mut bytes = [0; 32];
                bytes[byte] = 1;
                index.register(Author(bytes), round * 32 + byte as u32);
            }
        }
        for byte in 0..32 {
            let mut bytes = [0; 32];
            bytes[byte] = 1;
            let c = byte as u32;
            assert_eq!(index.get(&Author(bytes)).unwrap(), &[c, c + 32, c + 64]);
        }
        assert!(index.get(&Author([0; 32])).is_none());
    }
}
