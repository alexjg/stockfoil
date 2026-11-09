//! The actual delivered-head set and its incremental update.
use super::{clock::NodeIdx, commit_hash::CommitHash, indexes::HashIndex, storage::Node};
#[cfg(verus_keep_ghost)]
use crate::{
    graph,
    head_refinement::{self, leaf},
    storage_refinement::{graph_of, parent_set},
};
use std::collections::BTreeSet;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[derive(Debug)]
pub(crate) struct Heads { entries: BTreeSet<CommitHash> }

impl Heads {
    pub(crate) closed spec fn contents(&self) -> Set<CommitHash> { self.entries@ }

    #[verifier::opaque]
    pub(crate) open spec fn matches<P, C>(&self, nodes: Seq<Node<CommitHash, P, C>>, ids: Seq<NodeIdx>) -> bool {
        &&& forall|h: CommitHash| self.contents().contains(h) ==> exists|i: int|
            0 <= i < nodes.len() && nodes[i].hash == h
        &&& forall|i: int| #![trigger nodes[i]] 0 <= i < nodes.len() ==>
            self.contents().contains(nodes[i].hash) == leaf(graph_of(nodes, ids), i)
    }

    pub(crate) proof fn empty_matches<P, C>(&self, nodes: Seq<Node<CommitHash, P, C>>, ids: Seq<NodeIdx>)
        requires self.contents().is_empty(), nodes.len() == 0,
        ensures self.matches(nodes, ids),
    { reveal(Heads::matches); }

    pub(crate) fn new() -> (out: Self)
        ensures out.contents() == Set::<CommitHash>::empty(),
    { Self { entries: BTreeSet::new() } }

    pub(crate) fn as_set(&self) -> (out: &BTreeSet<CommitHash>)
        ensures out@ == self.contents(),
    { &self.entries }

    pub(crate) fn advance(&mut self, hash: CommitHash, parents: &[CommitHash])
        ensures final(self).contents() == old(self).contents().difference(parents@.to_set()).insert(hash),
        no_unwind
    {
        proof { super::stdlib::commit_order_model(); }
        let ghost before = self.contents();
        let mut i = 0;
        while i < parents.len()
            invariant i <= parents.len(),
                self.contents() == before.difference(parents@.subrange(0, i as int).to_set()),
                vstd::laws_cmp::obeys_cmp::<CommitHash>(),
            decreases parents.len() - i,
        {
            let ghost prior = self.contents();
            let ghost prefix = parents@.subrange(0, i as int);
            super::stdlib::remove_head(&mut self.entries, &parents[i]);
            assert(self.contents() == prior.remove(parents@[i as int]));
            assert(parents@.subrange(0, i + 1) =~= prefix.push(parents@[i as int]));
            proof { prefix.lemma_push_to_set_commute(parents@[i as int]); }
            i += 1;
            assert(self.contents() =~= before.difference(parents@.subrange(0, i as int).to_set()));
        }
        assert(parents@.subrange(0, i as int) == parents@);
        super::stdlib::insert_head(&mut self.entries, hash);
    }

    /// Soundness of the production count-and-membership fast-path guard.
    pub(crate) fn covers_all<P, C>(&self, nodes: &[Node<CommitHash, P, C>], _ids: &[NodeIdx],
        _index: &HashIndex, indices: &[NodeIdx]) -> (out: bool)
        requires self.matches(nodes@, _ids@), _index.matches(nodes@),
            super::candidates::strictly_ordered(indices@),
            forall|i: int| 0 <= i < indices.len() ==> indices[i]@ < nodes.len(),
        ensures out ==> parent_set(indices@) == head_refinement::heads(graph_of(nodes@, _ids@)),
    {
        proof { super::stdlib::commit_order_model(); reveal(Heads::matches); }
        if indices.len() != self.entries.len() { return false; }
        let mut i = 0;
        while i < indices.len()
            invariant i <= indices.len(),
                forall|j: int| 0 <= j < indices.len() ==> indices[j]@ < nodes.len(),
                forall|j: int| 0 <= j < i ==> self.contents().contains(nodes@[indices[j]@ as int].hash),
                vstd::laws_cmp::obeys_cmp::<CommitHash>(),
            decreases indices.len() - i,
        {
            if !self.entries.contains(&nodes[indices[i].index()].hash) { return false; }
            i += 1;
        }
        proof {
            let selected = indices@.map(|i: int, idx: NodeIdx| nodes@[idx@ as int].hash);
            assert forall|i: int, j: int| 0 <= i < j < selected.len() implies selected[i] != selected[j] by {
                assert(indices[i]@ < indices[j]@);
                assert(_index.contents()[selected[i]]@ == indices[i]@);
                assert(_index.contents()[selected[j]]@ == indices[j]@);
            }
            assert(selected.no_duplicates());
            selected.unique_seq_to_set();
            assert(selected.to_set().subset_of(self.contents())) by {
                assert forall|h: CommitHash| selected.to_set().contains(h) implies self.contents().contains(h) by {
                    let j = choose|j: int| 0 <= j < selected.len() && selected[j] == h;
                }
            }
            vstd::set_lib::lemma_subset_equality(selected.to_set(), self.contents());
            assert forall|n: int| parent_set(indices@).contains(n)
                == leaf(graph_of(nodes@, _ids@), n) by {
                if parent_set(indices@).contains(n) {
                    let j = choose|j: int| 0 <= j < indices.len() && indices[j]@ == n;
                    assert(self.contents().contains(nodes@[n].hash));
                }
                if leaf(graph_of(nodes@, _ids@), n) {
                    assert(self.contents().contains(nodes@[n].hash));
                    let j = choose|j: int| 0 <= j < selected.len() && selected[j] == nodes@[n].hash;
                    assert(_index.contents()[selected[j]]@ == n);
                    assert(_index.contents()[selected[j]]@ == indices[j]@);
                    assert(parent_set(indices@).contains(n));
                }
            }
            assert(parent_set(indices@) =~= head_refinement::heads(graph_of(nodes@, _ids@)));
        }
        true
    }

    pub(crate) proof fn unchanged<P, C>(&self, before: Seq<Node<CommitHash, P, C>>, ids: Seq<NodeIdx>,
        after: Seq<Node<CommitHash, P, C>>)
        requires self.matches(before, ids), after.len() == before.len(),
            graph_of(after, ids) == graph_of(before, ids),
            forall|i: int| 0 <= i < before.len() ==> after[i].hash == before[i].hash,
        ensures self.matches(after, ids),
    {
        reveal(Heads::matches);
        assert forall|h: CommitHash| self.contents().contains(h) implies exists|i: int|
            0 <= i < after.len() && after[i].hash == h by {
            let i = choose|i: int| 0 <= i < before.len() && before[i].hash == h;
            assert(after[i].hash == h);
        }
    }

    pub(crate) proof fn appended<P, C>(&self, before: Seq<Node<CommitHash, P, C>>, ids: Seq<NodeIdx>,
        after: Seq<Node<CommitHash, P, C>>, next_ids: Seq<NodeIdx>, index: &HashIndex,
        parents: Seq<CommitHash>, incoming: Seq<NodeIdx>, next: &Self)
        requires self.matches(before, ids), index.matches(before),
            graph::dag(graph_of(before, ids)), after.len() == before.len() + 1,
            after.subrange(0, before.len() as int) == before,
            !index.contents().contains_key(after.last().hash),
            incoming == parents.map(|i: int, h: CommitHash| index.contents()[h]),
            forall|i: int| 0 <= i < parents.len() ==> index.contents().contains_key(parents[i]),
            graph_of(after, next_ids) == graph_of(before, ids).push(parent_set(incoming)),
            next.contents() == self.contents().difference(parents.to_set()).insert(after.last().hash),
        ensures next.matches(after, next_ids),
    {
        reveal(Heads::matches);
        let g = graph_of(before, ids);
        assert forall|p: int| parent_set(incoming).contains(p) implies 0 <= p < before.len() by {
            let j = choose|j: int| 0 <= j < incoming.len() && incoming[j]@ == p;
            assert(index.contents().contains_key(parents[j]));
        }
        head_refinement::appended_heads(g, parent_set(incoming));
        assert forall|i: int| #![trigger before[i]] 0 <= i < before.len() implies
            parents.to_set().contains(before[i].hash) == parent_set(incoming).contains(i) by {
            if parents.to_set().contains(before[i].hash) {
                let j = choose|j: int| 0 <= j < parents.len() && parents[j] == before[i].hash;
                assert(incoming[j]@ == i);
            }
            if parent_set(incoming).contains(i) {
                let j = choose|j: int| 0 <= j < incoming.len() && incoming[j]@ == i;
                assert(before[i].hash == parents[j]);
                assert(parents.contains(before[i].hash));
            }
        }
        assert forall|h: CommitHash| next.contents().contains(h) implies exists|i: int|
            0 <= i < after.len() && after[i].hash == h by {
            if h == after.last().hash { assert(after[before.len() as int].hash == h); }
            else {
                let i = choose|i: int| 0 <= i < before.len() && before[i].hash == h;
                assert(after[i].hash == h);
            }
        }
        assert forall|i: int| #![trigger after[i]] 0 <= i < after.len() implies
            next.contents().contains(after[i].hash) == leaf(graph_of(after, next_ids), i) by {
            if i < before.len() {
                assert(before[i].hash != after.last().hash);
                assert(head_refinement::heads(g.push(parent_set(incoming))).contains(i)
                    == (head_refinement::heads(g).contains(i) && !parent_set(incoming).contains(i)));
            } else {
                assert(head_refinement::heads(g.push(parent_set(incoming))).contains(i));
            }
        }
    }
}

} // verus!
