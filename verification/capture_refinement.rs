//! Read coordinates preserved by append; no runtime graph snapshot is needed.
use vstd::prelude::*;
use crate::{clock::{NodeIdx, below}, commit_hash::CommitHash, tree::Clock};

verus! {

pub struct ReadState {
    pub labels: Seq<u32>,
    pub hashes: Map<CommitHash, NodeIdx>,
    pub graph: crate::graph::Graph,
    pub tables: Seq<Seq<NodeIdx>>,
}

impl ReadState {
    pub(crate) open spec fn valid(self) -> bool {
        self.labels.len() <= u32::MAX as nat + 1
            && forall|h: CommitHash| self.hashes.contains_key(h) ==> self.hashes[h]@ < self.labels.len()
    }

    #[verifier::opaque]
    pub(crate) open spec fn extends(self, old: Self) -> bool {
        &&& old.graph.is_prefix_of(self.graph)
        &&& old.tables.len() <= self.tables.len()
        &&& forall|c: int| 0 <= c < old.tables.len() ==> old.tables[c].is_prefix_of(#[trigger] self.tables[c])
        &&& old.labels.len() <= self.labels.len()
        &&& self.labels.subrange(0, old.labels.len() as int) == old.labels
        &&& (forall|h: CommitHash| #[trigger] old.hashes.contains_key(h) ==>
            self.hashes.contains_key(h) && self.hashes[h] == old.hashes[h])
        &&& (forall|h: CommitHash| #[trigger] self.hashes.contains_key(h) && self.hashes[h]@ < old.labels.len()
            ==> old.hashes.contains_key(h))
    }

    pub(crate) proof fn reflexive(self)
        ensures self.extends(self),
    { reveal(ReadState::extends); assert(self.labels.subrange(0, self.labels.len() as int) =~= self.labels); }

    pub(crate) proof fn transitive(self, middle: Self, old: Self)
        requires self.extends(middle), middle.extends(old),
        ensures self.extends(old),
    {
        reveal(ReadState::extends);
        assert(self.labels.subrange(0, old.labels.len() as int) =~= old.labels);
        assert forall|c: int| 0 <= c < old.tables.len() implies old.tables[c].is_prefix_of(#[trigger] self.tables[c]) by {
            assert(old.tables[c].is_prefix_of(middle.tables[c]));
            assert(middle.tables[c].is_prefix_of(self.tables[c]));
        }
        assert forall|h: CommitHash| old.hashes.contains_key(h) implies
            self.hashes.contains_key(h) && self.hashes[h] == old.hashes[h] by {
            assert(middle.hashes.contains_key(h));
        }
        assert forall|h: CommitHash| self.hashes.contains_key(h) && self.hashes[h]@ < old.labels.len()
            implies old.hashes.contains_key(h) by {
            assert(middle.hashes.contains_key(h));
            assert(middle.hashes[h] == self.hashes[h]);
        }
    }

    pub(crate) proof fn appended(self, old: Self, hash: CommitHash, idx: NodeIdx, chain: u32)
        requires old.labels.len() == idx@, self.labels.len() == old.labels.len() + 1,
            self.labels.subrange(0, old.labels.len() as int) == old.labels,
            self.hashes == old.hashes.insert(hash, idx), !old.hashes.contains_key(hash),
            old.graph.is_prefix_of(self.graph), chain <= old.tables.len(),
            self.tables == crate::chain_tables::extended(old.tables, chain, idx),
        ensures self.extends(old),
    {
        reveal(ReadState::extends);
        assert forall|c: int| 0 <= c < old.tables.len() implies old.tables[c].is_prefix_of(#[trigger] self.tables[c]) by {
            if c == chain { assert(self.tables[c] == old.tables[c].push(idx)); }
        }
    }
}

pub(crate) open spec fn frozen(clock: &Clock, state: ReadState, members: Set<CommitHash>) -> bool {
    &&& clock.valid()
    &&& state.valid()
    &&& forall|c: u32| clock.contents().contains_key(c) ==> clock.contents()[c]@ < state.labels.len()
    &&& members.subset_of(state.hashes.dom())
    &&& forall|h: CommitHash| state.hashes.contains_key(h) ==> members.contains(h)
        == below(clock.lookup(state.labels[state.hashes[h]@ as int]), state.hashes[h]@ as usize)
}

pub(crate) proof fn frozen_membership(clock: &Clock, old: ReadState, current: ReadState,
    members: Set<CommitHash>, hash: CommitHash)
    requires frozen(clock, old, members), current.valid(), current.extends(old),
    ensures members.contains(hash) == (current.hashes.contains_key(hash)
        && below(clock.lookup(current.labels[current.hashes[hash]@ as int]), current.hashes[hash]@ as usize)),
{
    reveal(ReadState::extends);
    if current.hashes.contains_key(hash) {
        let n = current.hashes[hash]@;
        if n < old.labels.len() {
            assert(old.hashes.contains_key(hash));
            assert(current.labels.subrange(0, old.labels.len() as int)[n as int] == old.labels[n as int]);
        } else {
            assert(!old.hashes.contains_key(hash));
            let c = current.labels[n as int];
            if clock.contents().contains_key(c) { assert(clock.contents()[c]@ < n); }
        }
    } else { assert(!old.hashes.contains_key(hash)); }
}

} // verus!
