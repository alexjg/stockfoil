//! Consumers start with the owned production state, not separately assumed indexes or chains.
use vstd::prelude::*;
use std::sync::Arc;
use crate::{topology::{Topology, InsertError}, author::Author, commit_hash::CommitHash,
    graph, clock::{NodeIdx, below}, storage_refinement::parent_set};

verus! {

pub fn capacity_boundaries() {
    let last_node = crate::topology::can_append(u32::MAX as usize, 0, 0);
    let full_parents = crate::topology::can_append(0, usize::MAX, 0);
    let overflow = crate::topology::can_append(0, usize::MAX, 1);
    let last_chain = crate::topology::fresh_chain(u32::MAX as usize - 1);
    let exhausted = crate::topology::fresh_chain(u32::MAX as usize);
    assert(last_node && full_parents && !overflow);
    assert(last_chain == Some((u32::MAX - 1) as u32) && exhausted is None);
}

pub(crate) proof fn singleton_parent(p: NodeIdx)
    ensures parent_set(seq![p]) == ISet::empty().insert(p@ as int),
{
    let ps = seq![p];
    assert forall|a: int| parent_set(ps).contains(a) == (a == p@) by {
        if a == p@ { assert(ps[0]@ == a); }
        if parent_set(ps).contains(a) {
            let i = choose|i: int| 0 <= i < ps.len() && ps[i]@ == a;
            assert(i == 0);
        }
    }
    assert(parent_set(ps) =~= ISet::empty().insert(p@ as int));
}

pub(crate) fn add(t: &mut Topology<u64>, hash: CommitHash, author: Author, parents: &[CommitHash]) -> (idx: NodeIdx)
    requires old(t).valid(), old(t).nodes_view().len() < 100, old(t).ids_view().len() < 100,
        old(t).tips_view().len() < 100, parents.len() < 10, !old(t).hashes().contains_key(hash),
        forall|i: int| 0 <= i < parents.len() ==> old(t).hashes().contains_key(parents[i]),
    ensures final(t).valid(), final(t).lineage_id() == old(t).lineage_id(),
        final(t).history_id() == old(t).history_id(), idx@ == old(t).nodes_view().len(),
        final(t).nodes_view()[idx@ as int].payload == 42,
        final(t).read_state().extends(old(t).read_state()),
        final(t).head_hashes() == old(t).head_hashes().difference(parents@.to_set()).insert(hash),
        final(t).nodes_view().len() == old(t).nodes_view().len() + 1,
        final(t).tips_view().len() <= old(t).tips_view().len() + 1,
        final(t).ids_view().len() == old(t).ids_view().len() + parents.len(),
        final(t).hashes() == old(t).hashes().insert(hash, idx),
        final(t).graph() == old(t).graph().push(parent_set(parents@.map(|i: int, h: CommitHash| old(t).hashes()[h]))),
        final(t).nodes_view().subrange(0, old(t).nodes_view().len() as int) == old(t).nodes_view(),
        final(t).owners_view().subrange(0, old(t).owners_view().len() as int) == old(t).owners_view(),
        old(t).owners_view().len() <= final(t).owners_view().len() <= old(t).owners_view().len() + 1,
        final(t).owners_view()[final(t).nodes_view()[idx@ as int].chain as int] == author,
{
    t.insert(hash, author, parents, 42, 0).unwrap()
}

pub fn constructed_topology_has_exact_ancestry() {
    let alice = Author([1u8; 32]);
    let bob = Author([2u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let h2 = CommitHash([2u8; 32]);
    let h3 = CommitHash([3u8; 32]);
    let missing = CommitHash([255u8; 32]);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && h2.0@[0] == 2 && h3.0@[0] == 3 && missing.0@[0] == 255);
    assert(h0 != h1 && h0 != h2 && h0 != h3 && h1 != h2 && h1 != h3 && h2 != h3);
    assert(missing != h0 && missing != h1 && missing != h2 && missing != h3);
    let mut t = Topology::new();
    let root = add(&mut t, h0, alice, &vec![]);
    assert(t.graph()[0] =~= ISet::<int>::empty());
    let ps = vec![h0, h0];
    let ghost resolved = ps@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(resolved =~= seq![root, root]);
    assert(parent_set(resolved) =~= ISet::empty().insert(0int));
    let child = add(&mut t, h1, bob, &ps);
    assert(t.graph()[1] =~= ISet::empty().insert(0int));
    let ps = vec![h1];
    let ghost resolved = ps@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(resolved =~= seq![child]);
    proof { singleton_parent(child); }
    assert(parent_set(resolved) =~= ISet::empty().insert(1int));
    let next = add(&mut t, h2, alice, &ps);
    assert(t.graph()[2] =~= ISet::empty().insert(1int));
    let saved = Arc::clone(t.clock(next));
    let ghost g = t.graph();
    proof {
        graph::unfold_reaches(g, 0, 0);
        graph::unfold_reaches(g, 0, 1);
        graph::unfold_reaches(g, 0, 2);
        assert(g[1].contains(0) && g[2].contains(1));
        assert(graph::reaches(g, 0, 2));
        assert(ISet::empty().insert(2int).contains(2int));
        assert(graph::history(g, ISet::empty().insert(2int)).contains(0int));
    }
    assert(below(saved.lookup(t.nodes_view()[0].chain), 0));
    let ps = vec![h0];
    let ghost resolved = ps@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(resolved =~= seq![root]);
    proof { singleton_parent(root); }
    assert(parent_set(resolved) =~= ISet::empty().insert(0int));
    let fork = add(&mut t, h3, alice, &ps);
    assert(t.graph()[3] =~= ISet::empty().insert(0int));
    proof { graph::append_preserves_reachability(g, t.graph()[3], 0, 2); }
    let fork_clock = t.clock(fork);
    proof {
        let g = t.graph();
        graph::unfold_reaches(g, 1, 0);
        graph::unfold_reaches(g, 1, 3);
        assert forall|p: int| g[3].contains(p) implies p == 0 by {};
        assert(!graph::reaches(g, 1, 3));
        assert forall|h: int| ISet::empty().insert(3int).contains(h) implies h == 3 by {};
        assert(!graph::history(g, ISet::empty().insert(3int)).contains(1int));
    }
    assert(!below(fork_clock.lookup(t.nodes_view()[1].chain), 1));
    let root_precedes_next = t.is_ancestor(&h0, &h2);
    let child_precedes_fork = t.is_ancestor(&h1, &h3);
    let reflexive = t.is_ancestor(&h0, &h0);
    let unknown = t.is_ancestor(&missing, &h3);
    assert(root_precedes_next && !child_precedes_fork && !reflexive && !unknown);
    let both = t.clock_for_heads(&vec![next, fork, next]);
    assert(both.valid());
    let ghost before = t;
    let duplicate = t.insert(h0, bob, &vec![h3], 99, 0);
    assert(duplicate is Err && t == before);
    let absent = t.insert(missing, alice, &vec![missing], 99, 0);
    assert(absent is Err && t == before);
    t.mark_admitted(root);
    t.mark_in_fragment(root);
    assert(t.nodes_view()[0].admitted && t.nodes_view()[0].in_fragment);
    assert(t.graph() == before.graph() && t.hashes() == before.hashes());
    let after_flags = t.clock_for_heads(&vec![next, fork]);
    assert(after_flags.valid());
}

} // verus!
