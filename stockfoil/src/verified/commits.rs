//! Public borrowed difference iterator, including its standard Iterator contract.
#[cfg(verus_keep_ghost)]
use super::{canonical_order, clock::NodeIdx, topology::Topology, traversal_init};
use super::{commit_hash::CommitHash, history::History, traversal::Traversal, tree::Clock};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn project<P>(t: &Topology<P>, nodes: Set<NodeIdx>) -> Set<CommitHash> {
    nodes.map(|n: NodeIdx| t.nodes_view()[n@ as int].hash)
}

proof fn injective<P>(t: &Topology<P>, a: NodeIdx, b: NodeIdx)
    requires t.valid(), a@ < t.nodes_view().len(), b@ < t.nodes_view().len(),
        t.nodes_view()[a@ as int].hash == t.nodes_view()[b@ as int].hash,
    ensures a == b,
{ t.traversal_facts(); NodeIdx::same_index(a, b); }

pub(crate) proof fn projection<P>(t: &Topology<P>, nodes: Set<NodeIdx>)
    requires t.valid(), canonical_order::bounded(t, nodes),
    ensures project(t, nodes).len() == nodes.len(), project(t, nodes).subset_of(t.hashes().dom()),
        forall|h: CommitHash| project(t, nodes).contains(h) == (t.hashes().contains_key(h) && nodes.contains(t.hashes()[h])),
{
    t.traversal_facts();
    let f = |n: NodeIdx| t.nodes_view()[n@ as int].hash;
    assert forall|a: NodeIdx, b: NodeIdx| nodes.contains(a) && nodes.contains(b) && a != b implies f(a) != f(b) by {
        if f(a) == f(b) { injective(t, a, b); }
    }
    assert(nodes.injective_on(f));
    vstd::set_lib::lemma_map_size(nodes, project(t, nodes), f);
    assert forall|h: CommitHash| project(t, nodes).contains(h) == (t.hashes().contains_key(h) && nodes.contains(t.hashes()[h])) by {
        if project(t, nodes).contains(h) {
            let n = choose|n: NodeIdx| nodes.contains(n) && f(n) == h;
            NodeIdx::same_index(t.hashes()[h], n);
        }
        if t.hashes().contains_key(h) && nodes.contains(t.hashes()[h]) {
            let n = t.hashes()[h];
            assert(t.hashes().contains_key(f(n)));
            // The index is a two-sided inverse of node hashes.
            t.hash_facts();
            assert(f(n) == h);
            assert(project(t, nodes).contains(f(n)));
        }
    }
    assert(project(t, nodes).subset_of(t.hashes().dom()));
}

pub(crate) proof fn projection_remove<P>(t: &Topology<P>, nodes: Set<NodeIdx>, n: NodeIdx)
    requires t.valid(), canonical_order::bounded(t, nodes), nodes.contains(n),
    ensures project(t, nodes.remove(n)) == project(t, nodes).remove(t.nodes_view()[n@ as int].hash),
{
    projection(t, nodes); projection(t, nodes.remove(n));
    t.traversal_facts();
    NodeIdx::same_index(t.hashes()[t.nodes_view()[n@ as int].hash], n);
    assert(project(t, nodes.remove(n)) =~= project(t, nodes).remove(t.nodes_view()[n@ as int].hash));
}

pub(crate) proof fn delta_at<P>(t: &Topology<P>, after: &Clock, before: &Clock, n: NodeIdx)
    requires t.valid(), n@ < t.nodes_view().len(),
    ensures super::difference::delta_member(super::difference::tables_view(t.tables_view()), after, before, n) ==
        (super::clock::below(after.lookup(t.nodes_view()[n@ as int].chain), n@ as usize)
            && !super::clock::below(before.lookup(t.nodes_view()[n@ as int].chain), n@ as usize)),
{
    t.traversal_facts(); t.traversal_node(n);
    let tables = super::difference::tables_view(t.tables_view());
    let chain = t.nodes_view()[n@ as int].chain;
    if super::difference::delta_member(tables, after, before, n) {
        let c = choose|c: u32| c < tables.len() && tables[c as int].contains(n)
            && after.contents().contains_key(c) && super::difference::selected(n, after.contents()[c], before.lookup(c));
        let p = choose|p: int| 0 <= p < tables[c as int].len() && tables[c as int][p] == n;
        assert(t.tables_view()[c as int][p] == n);
        assert(c == chain);
    }
    if super::clock::below(after.lookup(chain), n@ as usize) && !super::clock::below(before.lookup(chain), n@ as usize) {
        assert(tables[chain as int].contains(n));
        assert(super::difference::selected(n, after.contents()[chain], before.lookup(chain)));
        assert(super::difference::delta_member(tables, after, before, n));
    }
}

pub(crate) proof fn eligibility(history: &History, nodes: Set<NodeIdx>, n: NodeIdx)
    requires history.topology.valid(), canonical_order::bounded(&history.topology, nodes), nodes.contains(n),
    ensures crate::scheduler_refinement::eligible(history.topology.graph(), nodes, n) ==
        (forall|h: CommitHash| project(&history.topology, nodes).contains(h) && h != history.topology.nodes_view()[n@ as int].hash ==>
            !history.strict_ancestor(h, history.topology.nodes_view()[n@ as int].hash)),
{
    history.representation();
    let t = &history.topology;
    t.traversal_facts(); t.hash_facts(); projection(t, nodes);
    let hash = t.nodes_view()[n@ as int].hash;
    NodeIdx::same_index(t.hashes()[hash], n);
    if crate::scheduler_refinement::eligible(t.graph(), nodes, n) {
        assert forall|h: CommitHash| project(t, nodes).contains(h) && h != hash implies !history.strict_ancestor(h, hash) by {
            let a = t.hashes()[h];
            assert(nodes.contains(a));
            assert(a != n);
        }
    } else {
        let a = choose|a: NodeIdx| nodes.contains(a) && a != n && crate::graph::reaches(t.graph(), a@ as int, n@ as int);
        crate::graph::reaches_order(t.graph(), a@ as int, n@ as int);
        if a@ == n@ { NodeIdx::same_index(a, n); }
        let h = t.nodes_view()[a@ as int].hash;
        NodeIdx::same_index(t.hashes()[h], a);
        if h == hash { injective(t, a, n); }
        assert(project(t, nodes).contains(h));
        assert(history.strict_ancestor(h, hash));
    }
}

/// A lazy, captured commit-set difference from [`super::history::Ancestry::difference`]
/// or [`History::commits_since`]. No commit list is expanded up front.
/// Ancestors come first, smallest hash first among ready selected commits.
/// Each commit is returned once, including drafts, regardless of permissions.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct Commits<'h> {
    history: &'h History,
    // Taking the cursor leaves a valid exhausted iterator if a transition unwinds.
    traversal: Option<Traversal>,
}

impl<'h> Commits<'h> {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { self.traversal is Some ==> self.traversal.unwrap().valid(self.history) }

    closed spec fn selected(&self) -> Set<NodeIdx> { match self.traversal { None => Set::empty(), Some(t) => t.selected(self.history) } }
    pub closed spec fn history_view(&self) -> &History { self.history }
    pub closed spec fn remaining_hashes(&self) -> Set<CommitHash> { project(&self.history.topology, self.selected()) }
    pub closed spec fn future(&self) -> Seq<CommitHash> {
        let nodes = self.selected();
        canonical_order::order(&self.history.topology, nodes, nodes.len()).map(|i: int, n: NodeIdx| self.history.topology.nodes_view()[n@ as int].hash)
    }
    pub closed spec fn count(&self) -> usize { match self.traversal { None => 0, Some(t) => t.count() } }
    pub open spec fn eligible(&self, hash: CommitHash) -> bool {
        self.remaining_hashes().contains(hash) && forall|a: CommitHash| self.remaining_hashes().contains(a) && a != hash ==>
            !self.history_view().strict_ancestor(a, hash)
    }
    pub closed spec fn hash_le(a: CommitHash, b: CommitHash) -> bool { a == b || super::stdlib::hash_less(a, b) }

    pub proof fn sequence_facts(tracked &self)
        ensures self.count() == self.remaining_hashes().len() == self.future().len(), self.future().no_duplicates(),
            self.future().to_set() == self.remaining_hashes(),
            forall|i: int, j: int| 0 <= i < j < self.future().len() ==> !self.history_view().strict_ancestor(self.future()[j], self.future()[i]),
            forall|h: CommitHash| self.remaining_hashes().contains(h) ==> self.history_view().known(h),
    {
        use_type_invariant(self);
        self.history.topology.established(); self.history.representation();
        if let Some(cursor) = &self.traversal { cursor.facts(self.history); }
        let t = &self.history.topology;
        let nodes = self.selected();
        projection(t, nodes);
        canonical_order::order_facts(t, nodes);
        let order = canonical_order::order(t, nodes, nodes.len());
        assert forall|h: CommitHash| self.future().contains(h) == self.remaining_hashes().contains(h) by {
            if self.future().contains(h) {
                let i = choose|i: int| 0 <= i < self.future().len() && self.future()[i] == h;
                assert(nodes.contains(order[i]));
                assert(self.remaining_hashes().contains(t.nodes_view()[order[i]@ as int].hash));
            }
            if self.remaining_hashes().contains(h) {
                let n = choose|n: NodeIdx| nodes.contains(n) && t.nodes_view()[n@ as int].hash == h;
                assert(order.contains(n));
                let i = choose|i: int| 0 <= i < order.len() && order[i] == n;
                assert(self.future()[i] == h);
            }
        }
        assert(self.future().to_set() =~= self.remaining_hashes());
        t.traversal_facts();
        assert forall|i: int, j: int| 0 <= i < j < self.future().len() implies !self.history_view().strict_ancestor(self.future()[j], self.future()[i]) by {
            assert(nodes.contains(order[i]) && nodes.contains(order[j]));
            NodeIdx::same_index(t.hashes()[self.future()[i]], order[i]);
            NodeIdx::same_index(t.hashes()[self.future()[j]], order[j]);
        }
        assert forall|i: int, j: int| 0 <= i < j < self.future().len() implies self.future()[i] != self.future()[j] by {
            assert(nodes.contains(order[i]) && nodes.contains(order[j]));
            if self.future()[i] == self.future()[j] { injective(t, order[i], order[j]); }
        }
    }

    proof fn ready_facts(tracked &self)
        ensures canonical_order::bounded(&self.history.topology, self.selected()),
            self.traversal is Some ==> (forall|n: NodeIdx| self.traversal.unwrap().eligible(self.history, n)
                == crate::scheduler_refinement::eligible(self.history.topology.graph(), self.selected(), n)),
            forall|n: NodeIdx| self.selected().contains(n) ==> self.eligible(self.history.topology.nodes_view()[n@ as int].hash)
                == crate::scheduler_refinement::eligible(self.history.topology.graph(), self.selected(), n),
    {
        use_type_invariant(self); self.history.topology.established();
        if let Some(cursor) = &self.traversal { cursor.facts(self.history); }
        projection(&self.history.topology, self.selected());
        assert forall|n: NodeIdx| self.selected().contains(n) implies self.eligible(self.history.topology.nodes_view()[n@ as int].hash)
            == crate::scheduler_refinement::eligible(self.history.topology.graph(), self.selected(), n) by {
            eligibility(self.history, self.selected(), n);
            assert(self.remaining_hashes().contains(self.history.topology.nodes_view()[n@ as int].hash));
        }
    }

    pub(crate) fn new(history: &'h History, after: &Clock, before: &Clock) -> (out: Self)
        requires after.ordered(), before.ordered(),
            forall|c: u32| after.contents().contains_key(c) ==> c < history.topology.tables_view().len(),
        ensures out.history_view() == history,
            forall|h: CommitHash| out.remaining_hashes().contains(h) == (history.topology.hashes().contains_key(h)
                && super::difference::delta_member(super::difference::tables_view(history.topology.tables_view()), after, before, history.topology.hashes()[h])),
    {
        let differences = [(after, before)];
        proof { history.topology.established(); history.topology.traversal_facts(); }
        let traversal = Traversal::new(history, &differences);
        proof {
            traversal.facts(history);
            projection(&history.topology, traversal.selected(history));
            assert forall|n: NodeIdx| traversal.selected(history).contains(n) == super::difference::delta_member(
                super::difference::tables_view(history.topology.tables_view()), after, before, n) by {
                traversal_init::requested_difference(&history.topology, differences@, n);
            }
        }
        Self { history, traversal: Some(traversal) }
    }
}

#[cfg(verus_keep_ghost)]
impl<'h> vstd::std_specs::iter::IteratorSpecImpl for Commits<'h> {
    open spec fn obeys_prophetic_iter_laws(&self) -> bool { true }
    open spec fn remaining(&self) -> Seq<CommitHash> { self.future() }
    open spec fn produces(&self) -> Seq<bool> { Seq::new(self.future().len(), |i: int| true) }
    open spec fn decrease(&self) -> Option<nat> { Some(self.future().len()) }
    open spec fn peek(&self, i: int) -> Option<CommitHash> { if 0 <= i < self.future().len() { Some(self.future()[i]) } else { None } }
}

impl Iterator for Commits<'_> {
    type Item = CommitHash;
    fn next(&mut self) -> (out: Option<CommitHash>)
        ensures final(self).history_view() == old(self).history_view(),
            (out is None) == old(self).remaining_hashes().is_empty(),
            match out {
                None => final(self).remaining_hashes() == old(self).remaining_hashes(),
                Some(h) => old(self).remaining_hashes().contains(h)
                    && final(self).remaining_hashes() == old(self).remaining_hashes().remove(h)
                    && old(self).eligible(h)
                    && (forall|other: CommitHash| old(self).eligible(other) ==> Self::hash_le(h, other)),
            },
    {
        proof { use_type_invariant(&*self); self.history.topology.established(); self.sequence_facts(); self.ready_facts(); if let Some(cursor) = &self.traversal { cursor.facts(self.history); } }
        let ghost before = *self;
        let ghost nodes = self.selected();
        let mut traversal = match self.traversal.take() { None => return None, Some(cursor) => cursor };
        let next = traversal.next(self.history);
        self.traversal = Some(traversal);
        let Some(n) = next else { return None; };
        let hash = self.history.topology.nodes()[n.index()].hash;
        proof {
            // Both the set contract and the standard Iterator prediction follow the real step.
            projection_remove(&self.history.topology, nodes, n);
            assert forall|a: NodeIdx| crate::scheduler_refinement::eligible(self.history.topology.graph(), nodes, a) implies
                super::stdlib::ready_le(canonical_order::key(&self.history.topology, n), canonical_order::key(&self.history.topology, a)) by {
                assert(before.traversal.unwrap().eligible(self.history, a));
            }
            canonical_order::order_step(&self.history.topology, nodes, n);
            projection(&self.history.topology, nodes);
            self.history.topology.hash_facts();
            assert(before.eligible(hash));
            assert forall|other: CommitHash| before.eligible(other) implies Self::hash_le(hash, other) by {
                let a = self.history.topology.hashes()[other];
                assert(nodes.contains(a));
                assert(self.history.topology.nodes_view()[a@ as int].hash == other);
                assert(before.traversal.unwrap().eligible(self.history, a));
            }
        }
        Some(hash)
    }

    fn size_hint(&self) -> (out: (usize, Option<usize>))
        ensures out == (self.count(), Some(self.count())), self.count() == self.remaining_hashes().len(),
    {
        proof { self.sequence_facts(); }
        let remaining = match &self.traversal { None => 0, Some(cursor) => cursor.remaining() };
        (remaining, Some(remaining))
    }
}

#[cfg(verus_keep_ghost)]
impl<'h> vstd::std_specs::iter::ExactSizeIteratorSpecImpl for Commits<'h> {
    open spec fn exact_len(&self) -> usize { self.count() }
}
impl ExactSizeIterator for Commits<'_> {
    fn len(&self) -> (out: usize)
        ensures out == self.count() == self.remaining_hashes().len(),
    { proof { self.sequence_facts(); } match &self.traversal { None => 0, Some(cursor) => cursor.remaining() } }
}
impl std::iter::FusedIterator for Commits<'_> {}

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, Error, WritePermissions};

    #[test]
    fn public_difference_keeps_old_capture_order_counts_and_independent_handle_lifetimes() {
        let mut history = History::new();
        let (a, b, c, later) = (
            CommitHash([8; 32]),
            CommitHash([1; 32]),
            CommitHash([0; 32]),
            CommitHash([2; 32]),
        );
        history
            .topology
            .insert(a, Author([1; 32]), &[], 0, 0)
            .unwrap();
        history
            .topology
            .insert(b, Author([2; 32]), &[a], 0, 0)
            .unwrap();
        history
            .topology
            .insert(c, Author([3; 32]), &[], 0, 0)
            .unwrap();
        let permissions = WritePermissions::new();
        let empty = history.ancestry(&[], &permissions).unwrap();
        let before = history.ancestry(&[a], &permissions).unwrap();
        let after = history.ancestry(&[b, c], &permissions).unwrap();
        history
            .topology
            .insert(later, Author([2; 32]), &[b], 0, 0)
            .unwrap();
        assert!(!after.participates(&history, &b).unwrap());
        assert_eq!(
            after
                .difference(&before, &history)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![c, b]
        );
        let mut iter = after.difference(&empty, &history).unwrap();
        drop(after);
        drop(empty);
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.next(), Some(c));
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.by_ref().collect::<Vec<_>>(), vec![a, b]);
        for _ in 0..3 {
            assert_eq!(iter.next(), None);
            assert_eq!(iter.len(), 0);
            assert_eq!(iter.size_hint(), (0, Some(0)));
        }
    }

    #[test]
    fn public_difference_checks_both_capture_identities_even_with_equal_contents() {
        let history = History::new();
        let other = History::new();
        let permissions = WritePermissions::new();
        let here = history.ancestry(&[], &permissions).unwrap();
        let there = other.ancestry(&[], &permissions).unwrap();
        assert!(matches!(
            there.difference(&here, &history),
            Err(Error::WrongHistory)
        ));
        assert!(matches!(
            here.difference(&there, &history),
            Err(Error::WrongHistory)
        ));
        let mut empty = here.difference(&here, &history).unwrap();
        assert_eq!(empty.size_hint(), (0, Some(0)));
        assert_eq!(empty.next(), None);
        assert_eq!(empty.next(), None);
    }
}
