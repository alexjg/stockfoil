//! Concrete, nonempty witnesses for the proof contracts and policy edge cases.
use vstd::prelude::*;
use crate::clock::Entry;
use crate::graph::*;
use crate::visibility::*;

verus! {

pub proof fn out_of_history_bound_must_not_grant_ancestor()
{
    let g: Graph = seq![ISet::empty(), ISet::empty().insert(0int)];
    let heads = ISet::empty().insert(0int);
    let frontier = ISet::empty().insert(1int);
    let h = history(g, heads);
    unfold_reaches(g, 0, 0);
    unfold_reaches(g, 0, 1);
    unfold_reaches(g, 1, 0);
    assert(heads.contains(0));
    assert(frontier.contains(1));
    assert(h.contains(0));
    assert(!h.contains(1));
    unavailable_frontier_grants_nothing(g, h, frontier, 0);
    assert(!allowed(g, h, Permission::Bounded(frontier), 0));
    // The incorrect union-first/intersect-later policy would grant this ancestor.
    assert(h.contains(0) && history(g, frontier).contains(0));
}

pub proof fn visibility_can_skip_a_denied_intermediate_author()
{
    let g0: Graph = Seq::empty();
    let c0: Seq<usize> = Seq::empty();
    let a0: Seq<usize> = Seq::empty();
    assert(chain_invariant(g0, c0, a0));
    extend_chain(g0, c0, a0, ISet::empty(), 0, 0, None);
    let g1 = g0.push(ISet::empty());
    let c1 = c0.push(0);
    let a1 = a0.push(0);
    extend_chain(g1, c1, a1, ISet::empty().insert(0int), 1, 1, None);
    let g2 = g1.push(ISet::empty().insert(0int));
    let c2 = c1.push(1);
    let a2 = a1.push(1);
    unfold_reaches(g2, 0, 0);
    unfold_reaches(g2, 0, 1);
    assert(ISet::empty().insert(1int).contains(1));
    assert(history(g2, ISet::empty().insert(1int)).contains(0));
    extend_chain(g2, c2, a2, ISet::empty().insert(1int), 0, 0, Some(0int));
    let g = g2.push(ISet::empty().insert(1int));
    let chains = c2.push(0);
    let authors = a2.push(0);
    let heads = ISet::empty().insert(2int);
    let perms = IMap::empty().insert(0usize, Permission::Unbounded).insert(1usize, Permission::Denied);
    let entries: Seq<Entry> = seq![(0usize, 2usize), (1usize, 1usize)];
    let bounds: Seq<Option<usize>> = seq![Some(2usize), None];
    let frontiers: IMap<usize, Seq<Entry>> = IMap::empty();
    assert forall|a: int| 0 <= a <= 2 implies reaches(g, a, 2) by {
        unfold_reaches(g, a, 0);
        unfold_reaches(g, a, 1);
        unfold_reaches(g, a, 2);
    }
    assert forall|a: int| history(g, heads).contains(a) implies 0 <= a <= 2 by {
        let h = choose|h: int| heads.contains(h) && reaches(g, a, h);
        reaches_order(g, a, h);
    }
    assert(heads.contains(2));
    assert(history(g, heads).contains(0));
    assert(history(g, heads).contains(1));
    assert(history(g, heads).contains(2));
    assert(summarizes(entries, chains, history(g, heads)));
    assert(compiled_bounds(g, chains, authors, heads, perms, entries, bounds, frontiers));
    mask_matches_policy(g, chains, authors, heads, perms, entries, bounds, frontiers, 0);
    mask_matches_policy(g, chains, authors, heads, perms, entries, bounds, frontiers, 1);
    mask_matches_policy(g, chains, authors, heads, perms, entries, bounds, frontiers, 2);
    assert(visible(g, authors, heads, perms, 0));
    assert(!visible(g, authors, heads, perms, 1));
    assert(visible(g, authors, heads, perms, 2));
}

} // verus!
