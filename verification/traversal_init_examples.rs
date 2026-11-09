//! Full constructor-to-exhaustion consumers, without assumed pending state or counts.
use vstd::prelude::*;
use crate::{traversal_init as init, traversal_step as step, pending, topology::Topology,
    tree::Clock, clock::NodeIdx, graph};

verus! {

pub(crate) fn construct_and_drain<P>(t: &Topology<P>, differences: &[(&Clock, &Clock)]) -> (out: Vec<NodeIdx>)
    requires init::inputs(t.tables_view(), differences@),
    ensures out@.no_duplicates(),
        forall|n: NodeIdx| out@.contains(n) == init::requested(t, differences@, n),
        forall|n: NodeIdx| out@.contains(n) == (exists|i: int| 0 <= i < differences.len()
            && crate::difference::delta_member(crate::difference::tables_view(t.tables_view()), differences[i].0, differences[i].1, n)),
        forall|i: int, j: int| 0 <= i < j < out.len() ==> !graph::reaches(t.graph(), out[j]@ as int, out[i]@ as int),
{
    proof { t.established(); }
    let (mut p, mut q, mut remaining) = init::new(t, differences);
    let ghost selected = pending::selected_set(t, p@);
    proof {
        pending::selection_facts(t, p@);
        assert forall|n: NodeIdx| selected.contains(n) == init::requested(t, differences@, n) by {};
    }
    let out = crate::traversal_step_examples::drain(t, &mut p, &mut q, &mut remaining);
    let exhausted = step::next(t, &mut p, &mut q, &mut remaining);
    assert(exhausted is None && remaining == 0);
    assert forall|n: NodeIdx| out@.contains(n) == (exists|i: int| 0 <= i < differences.len()
        && crate::difference::delta_member(crate::difference::tables_view(t.tables_view()), differences[i].0, differences[i].1, n)) by {
        init::requested_difference(t, differences@, n);
    }
    out
}

pub fn constructed_duplicate_differences_count_once() {
    let mut t = Topology::new();
    let root = crate::topology_examples::add(&mut t, crate::commit_hash::CommitHash([7; 32]),
        crate::author::Author([1; 32]), &vec![]);
    let chain = t.nodes()[root.index()].chain;
    let mut after = Clock::default();
    after.include(chain, root);
    let empty = Clock::default();
    let differences = vec![(&after, &empty), (&after, &empty), (&after, &after)];
    assert(init::inputs(t.tables_view(), differences@)
        && (forall|n: NodeIdx| init::requested(&t, differences@, n) == (n == root))) by {
        t.traversal_facts();
        t.traversal_node(root);
        assert(init::inputs(t.tables_view(), differences@));
        let p = choose|p: int| 0 <= p < t.tables_view()[chain as int].len() && t.tables_view()[chain as int][p] == root;
        assert(init::delta(t.tables_view(), &after, &empty, chain, p));
        assert(init::union(t.tables_view(), differences@, chain, p));
        assert(init::requested(&t, differences@, root));
        assert forall|n: NodeIdx| init::requested(&t, differences@, n) implies n == root by {
            let (c, p) = choose|c: u32, p: int| init::union(t.tables_view(), differences@, c, p) && t.tables_view()[c as int][p] == n;
            let i = choose|i: int| 0 <= i < differences.len() && init::delta(t.tables_view(), differences[i].0, differences[i].1, c, p);
            assert(n@ <= root@);
            NodeIdx::same_index(n, root);
        }
    }
    let (mut p, mut q, mut remaining) = init::new(&t, &differences);
    assert(pending::selected_set(&t, p@) == Set::empty().insert(root)) by {
        pending::selection_facts(&t, p@);
        assert forall|n: NodeIdx| pending::selected_set(&t, p@).contains(n) == (n == root) by {
            assert(pending::selected_set(&t, p@).contains(n) == pending::selected(&t, p@, n));
            assert(pending::selected(&t, p@, n) == init::requested(&t, differences@, n));
            assert(init::requested(&t, differences@, n) == (n == root));
        }
        assert(pending::selected_set(&t, p@) =~= Set::empty().insert(root));
    }
    assert(remaining == 1);
    let emitted = step::next(&t, &mut p, &mut q, &mut remaining);
    assert(emitted == Some(root) && remaining == 0);
    let exhausted = step::next(&t, &mut p, &mut q, &mut remaining);
    assert(exhausted is None);
}

}
