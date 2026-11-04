//! Nonempty policy witnesses, independent of chain labels and AVL representation.
use vstd::prelude::*;
use crate::graph::*;
use crate::authorization::*;

verus! {

// Alice(0) -> Bob(1) -> Alice(2), plus an unrelated Alice root(3).
// The view has two heads; permission frontiers can include duplicates in the
// concrete API, whose resolution to sets remains a refinement obligation.
pub proof fn multiple_heads_and_cross_author_frontiers() {
    let g: Graph = seq![ISet::empty(), ISet::empty().insert(0int),
        ISet::empty().insert(1int), ISet::empty()];
    let authors = seq![0usize, 1usize, 0usize, 0usize];
    let heads = ISet::empty().insert(2int).insert(3int);
    assert forall|a: int| 0 <= a < 4 implies history(g, heads).contains(a) by {
        unfold_reaches(g, a, 0);
        unfold_reaches(g, a, 1);
        unfold_reaches(g, a, 2);
        unfold_reaches(g, a, 3);
        assert(heads.contains(2) && heads.contains(3));
    }
    let frontier = ISet::empty().insert(1int).insert(3int).insert(99int);
    let perms = IMap::empty().insert(0usize, Permission::Bounded(frontier));
    // Bob's frontier authorizes Alice's ancestor but not Alice's later child.
    unfold_reaches(g, 0, 0);
    unfold_reaches(g, 0, 1);
    assert(frontier.contains(1));
    assert(history(g, heads).contains(0));
    assert(history(g, heads).contains(1));
    assert(allowed(g, history(g, heads), Permission::Bounded(frontier), 0));
    assert(visible(g, authors, heads, perms, 0));
    assert(!visible(g, authors, heads, perms, 1));
    assert forall|f: int| frontier.contains(f) implies !reaches(g, 2, f) by {
        unfold_reaches(g, 2, 0);
        unfold_reaches(g, 2, 1);
        unfold_reaches(g, 2, 3);
    }
    assert(!visible(g, authors, heads, perms, 2));
    available_bound_includes_itself(g, heads, frontier, 3);
    assert(visible(g, authors, heads, perms, 3));
    let expanded = perms.insert(0usize, Permission::Bounded(frontier.insert(2int)));
    available_bound_includes_itself(g, heads, frontier.insert(2int), 2);
    assert(visible(g, authors, heads, expanded, 2));
    // Traversal crosses denied Bob; Alice's root remains authorized.
    assert(visible(g, authors, heads, expanded, 0));
    assert(!visible(g, authors, heads, expanded, 1));
}

pub proof fn unbounded_does_not_escape_the_selected_history() {
    let g: Graph = seq![ISet::empty(), ISet::empty()];
    let authors = seq![0usize, 0usize];
    let heads = ISet::empty().insert(0int);
    let perms = IMap::empty().insert(0usize, Permission::Unbounded);
    unfold_reaches(g, 0, 0);
    unfold_reaches(g, 1, 0);
    assert(heads.contains(0));
    assert(history(g, heads).contains(0));
    assert(visible(g, authors, heads, perms, 0));
    assert(!visible(g, authors, heads, perms, 1));
    assert(!visible(g, authors, heads, IMap::empty(), 0));
    assert(!visible(g, authors, heads, perms.insert(0usize, Permission::Denied), 0));
}

pub proof fn a_future_frontier_cannot_change_a_captured_view() {
    let g: Graph = seq![ISet::empty()];
    let authors = seq![0usize];
    let heads = ISet::empty().insert(0int);
    let perms = IMap::empty().insert(0usize, Permission::Bounded(ISet::empty().insert(1int)));
    unfold_reaches(g, 0, 0);
    unfold_reaches(g, 1, 0);
    assert(!visible(g, authors, heads, perms, 0));
    captured_visibility_survives_append(g, ISet::empty().insert(0int), authors, 0, heads, perms, 0);
    let next = g.push(ISet::empty().insert(0int));
    assert(!visible(next, authors.push(0usize), heads, perms, 0));
    // A fresh view that includes the now-known frontier legitimately grants it.
    let new_heads = ISet::empty().insert(1int);
    unfold_reaches(next, 0, 0);
    unfold_reaches(next, 0, 1);
    unfold_reaches(next, 1, 1);
    assert(new_heads.contains(1));
    assert(history(next, new_heads).contains(0));
    assert(history(next, new_heads).contains(1));
    assert(ISet::empty().insert(1int).contains(1));
    assert(visible(next, authors.push(0usize), new_heads, perms, 0));
    assert(visible(next, authors.push(0usize), new_heads, perms, 1));
}

} // verus!
