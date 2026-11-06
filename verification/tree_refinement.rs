//! Refinement from the actual tree to the independent parent-edge policy.
//! History construction and permission compilation remain explicit hypotheses.
use vstd::prelude::*;
use crate::clock::{NodeIdx, below};
use crate::coordinates::{Entry, member, permitted};
use crate::graph::*;
use crate::tree::Clock;
use crate::visibility::*;

verus! {

pub(crate) open spec fn entries(tree: &Clock) -> Seq<Entry> {
    tree.sequence().map(|i: int, e: (u32, NodeIdx)| (e.0 as usize, e.1))
}

pub(crate) open spec fn bounds(tree: &Clock, policy: spec_fn(u32, NodeIdx) -> Option<NodeIdx>) -> Seq<Option<NodeIdx>> {
    tree.sequence().map(|i: int, e: (u32, NodeIdx)| policy(e.0, e.1))
}

pub(crate) proof fn lookup_matches_coordinates(tree: &Clock, chain: u32, position: usize)
    requires tree.ordered(),
    ensures crate::coordinates::sorted(entries(tree)),
        member(entries(tree), chain as usize, position) == below(tree.lookup(chain), position),
{
    tree.sequence_correct();
    assert forall|i: int, j: int| 0 <= i < j < entries(tree).len() implies entries(tree)[i].0 < entries(tree)[j].0 by {
        assert(tree.sequence()[i].0 < tree.sequence()[j].0);
    }
    if member(entries(tree), chain as usize, position) {
        let i = choose|i: int| 0 <= i < entries(tree).len()
            && entries(tree)[i].0 == chain && position <= entries(tree)[i].1@;
        assert(tree.sequence()[i].0 == chain);
        assert(tree.contents().contains_key(tree.sequence()[i].0));
    }
    if below(tree.lookup(chain), position) {
        let i = choose|i: int| 0 <= i < tree.sequence().len() && tree.sequence()[i].0 == chain;
        assert(entries(tree)[i] == (chain as usize, tree.contents()[chain]));
    }
}

pub(crate) proof fn masked_coordinates(
    before: &Clock, after: &Clock, policy: spec_fn(u32, NodeIdx) -> Option<NodeIdx>, chain: u32, position: usize,
)
    requires before.ordered(), after.ordered(),
        forall|c: u32| #[trigger] after.contents().contains_key(c) <==>
            before.contents().contains_key(c) && policy(c, before.contents()[c]) is Some,
        forall|c: u32| after.contents().contains_key(c) ==> #[trigger] after.contents()[c] ==
            Clock::masked_value(before.contents()[c], policy(c, before.contents()[c])).unwrap(),
    ensures below(after.lookup(chain), position) == permitted(entries(before), bounds(before, policy), chain as usize, position),
{
    before.sequence_correct();
    lookup_matches_coordinates(before, chain, position);
    lookup_matches_coordinates(after, chain, position);
    if permitted(entries(before), bounds(before, policy), chain as usize, position) {
        let i = choose|i: int| 0 <= i < entries(before).len() && entries(before)[i].0 == chain
            && position <= entries(before)[i].1@ && below(bounds(before, policy)[i], position);
        assert(before.sequence()[i].0 == chain);
        assert(before.contents().contains_key(before.sequence()[i].0));
        assert(before.contents()[chain] == before.sequence()[i].1);
        assert(bounds(before, policy)[i] == policy(chain, before.contents()[chain]));
    }
    if below(after.lookup(chain), position) {
        let i = choose|i: int| 0 <= i < before.sequence().len() && before.sequence()[i].0 == chain;
        assert(entries(before)[i] == (chain as usize, before.contents()[chain]));
        assert(bounds(before, policy)[i] == policy(chain, before.contents()[chain]));
        assert(below(bounds(before, policy)[i], position));
        assert(permitted(entries(before), bounds(before, policy), chain as usize, position));
    }
}

// This executes the production mask, not a model implementation. The unresolved
// history/compilation premises are precisely the boundary for subsequent PRs.
pub(crate) fn mask_for_policy<F: FnMut(u32, NodeIdx) -> Option<NodeIdx>>(
    tree: &Clock, bound: F,
    Ghost(policy): Ghost<spec_fn(u32, NodeIdx) -> Option<NodeIdx>>,
    Ghost(g): Ghost<Graph>, Ghost(chains): Ghost<Seq<usize>>, Ghost(authors): Ghost<Seq<usize>>,
    Ghost(heads): Ghost<ISet<int>>, Ghost(perms): Ghost<IMap<usize, Permission>>,
    Ghost(frontiers): Ghost<IMap<usize, Seq<Entry>>>,
) -> (out: Clock)
    requires tree.valid(), chain_invariant(g, chains, authors),
        forall|a: int| 0 <= a < chains.len() ==> chains[a] <= u32::MAX,
        summarizes(entries(tree), chains, history(g, heads)),
        compiled_bounds(g, chains, authors, heads, perms, entries(tree),
            bounds(tree, policy), frontiers),
        forall|c: u32, p: NodeIdx| tree.contents().contains_key(c) && tree.contents()[c] == p ==> bound.requires((c, p)),
        forall|c: u32, p: NodeIdx, r: Option<NodeIdx>| bound.ensures((c, p), r) ==> r == policy(c, p),
    ensures out.valid(),
        forall|a: usize| a < g.len() ==> below(out.lookup(chains[a as int] as u32), a)
            == visible(g, authors, heads, perms, a as int),
{
    let ghost original_bound = bound;
    let out = tree.mask(bound);
    proof {
        assert forall|c: u32| tree.contents().contains_key(c) implies
            Clock::callback_bound(original_bound, c, tree.contents()[c]) == policy(c, tree.contents()[c]) by {
            assert(original_bound.ensures((c, tree.contents()[c]), Clock::callback_bound(original_bound, c, tree.contents()[c])));
        }
        assert forall|a: usize| a < g.len() implies below(out.lookup(chains[a as int] as u32), a)
            == visible(g, authors, heads, perms, a as int) by {
            masked_coordinates(tree, &out, policy, chains[a as int] as u32, a);
            mask_matches_policy(g, chains, authors, heads, perms, entries(tree), bounds(tree, policy), frontiers, a);
        }
    }
    out
}

pub fn bounded_policy_runs_on_the_actual_tree() {
    let zero = NodeIdx::new(0);
    let one = NodeIdx::new(1);
    let tree = Clock::from_tips(&vec![one]);
    let ghost g: Graph = seq![ISet::empty(), ISet::empty().insert(0int)];
    let ghost chains = seq![0usize, 0usize];
    let ghost authors = seq![0usize, 0usize];
    let ghost heads = ISet::empty().insert(1int);
    let ghost frontier = ISet::empty().insert(0int);
    let ghost perms = IMap::empty().insert(0usize, Permission::Bounded(frontier));
    let ghost frontiers = IMap::empty().insert(0usize, seq![(0usize, zero)]);
    let ghost policy = |_c: u32, _p: NodeIdx| Some(zero);
    proof {
        tree.sequence_correct();
        assert(tree.sequence()[0].0 == 0u32);
        assert(entries(&tree) =~= seq![(0usize, one)]);
        assert(bounds(&tree, policy) =~= seq![Some(zero)]);
        assert forall|a: int, d: int| 0 <= a <= d < 2 implies reaches(g, a, d) by {
            unfold_reaches(g, a, 0);
            unfold_reaches(g, a, 1);
        }
        assert(chain_invariant(g, chains, authors));
        assert forall|a: int| history(g, heads).contains(a) == (0 <= a < 2) by {
            unfold_reaches(g, a, 0);
            unfold_reaches(g, a, 1);
            assert(heads.contains(1int));
        }
        assert forall|a: int| history(g, frontier.intersect(history(g, heads))).contains(a) == (a == 0) by {
            unfold_reaches(g, a, 0);
            assert(frontier.intersect(history(g, heads)).contains(0int));
        }
        assert(history(g, heads).contains(0int) && history(g, heads).contains(1int));
        assert forall|a: int| history(g, heads).contains(a) implies 0 <= a < chains.len()
            && exists|i: int| 0 <= i < entries(&tree).len() && entries(&tree)[i].0 == chains[a] && a <= entries(&tree)[i].1@ by {
            assert(entries(&tree)[0].0 == chains[a] && a <= entries(&tree)[0].1@);
        }
        assert(summarizes(entries(&tree), chains, history(g, heads)));
        let selected_frontier = history(g, frontier.intersect(history(g, heads)));
        assert(selected_frontier.contains(0int));
        assert forall|a: int| selected_frontier.contains(a) implies 0 <= a < chains.len()
            && exists|i: int| 0 <= i < frontiers[0usize].len() && frontiers[0usize][i].0 == chains[a] && a <= frontiers[0usize][i].1@ by {
            assert(frontiers[0usize][0].0 == chains[a] && a <= frontiers[0usize][0].1@);
        }
        assert(summarizes(frontiers[0usize], chains, selected_frontier));
        assert forall|p: usize| below(Some(zero), p) == member(frontiers[0usize], 0usize, p) by {
            if p == 0 { assert(frontiers[0usize][0].0 == 0usize && p <= frontiers[0usize][0].1@); }
        }
        assert(compiled_bounds(g, chains, authors, heads, perms, entries(&tree), bounds(&tree, policy), frontiers));
    }
    let bound = |_c: u32, _p: NodeIdx| -> (r: Option<NodeIdx>) ensures r == Some(zero), { Some(zero) };
    let out = mask_for_policy(&tree, bound, Ghost(policy), Ghost(g), Ghost(chains), Ghost(authors), Ghost(heads), Ghost(perms), Ghost(frontiers));
    assert(visible(g, authors, heads, perms, 0));
    assert(!visible(g, authors, heads, perms, 1));
    assert(below(out.lookup(0u32), 0usize));
    assert(!below(out.lookup(0u32), 1usize));
    let ancestor = out.contains(0, zero);
    let later = out.contains(0, one);
    assert(ancestor && !later);
    assert(visible(g, authors, heads, perms, 0));
    assert(!visible(g, authors, heads, perms, 1));
}

} // verus!
