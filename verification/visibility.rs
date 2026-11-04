//! Permission policy is stated using parent reachability, independently of clocks.
use vstd::prelude::*;
use crate::coordinates::{Entry, member, permitted};
use crate::clock::below;
use crate::graph::*;

verus! {

pub enum Permission {
    Denied,
    Unbounded,
    Bounded(ISet<int>),
}

pub open spec fn permission(perms: IMap<usize, Permission>, author: usize) -> Permission {
    if perms.dom().contains(author) { perms[author] } else { Permission::Denied }
}

pub open spec fn allowed(g: Graph, h: ISet<int>, perm: Permission, a: int) -> bool {
    match perm {
        Permission::Denied => false,
        Permission::Unbounded => true,
        Permission::Bounded(frontier) => exists|f: int|
            frontier.contains(f) && h.contains(f) && reaches(g, a, f),
    }
}

pub open spec fn visible(
    g: Graph, authors: Seq<usize>, heads: ISet<int>, perms: IMap<usize, Permission>, a: int,
) -> bool {
    0 <= a < authors.len() && history(g, heads).contains(a)
        && allowed(g, history(g, heads), permission(perms, authors[a]), a)
}

pub proof fn bounded_policy(g: Graph, h: ISet<int>, frontier: ISet<int>, a: int)
    ensures allowed(g, h, Permission::Bounded(frontier), a)
        == history(g, frontier.intersect(h)).contains(a),
{
    if allowed(g, h, Permission::Bounded(frontier), a) {
        let f = choose|f: int| frontier.contains(f) && h.contains(f) && reaches(g, a, f);
        assert(frontier.intersect(h).contains(f));
    }
    if history(g, frontier.intersect(h)).contains(a) {
        let f = choose|f: int| frontier.intersect(h).contains(f) && reaches(g, a, f);
        assert(frontier.contains(f) && h.contains(f));
    }
}

// Captures the three branches of participation(). For a bounded author, the
// bound is the lookup result in the union of *available frontier heads*.
// The wrapper must still establish these premises from its maps and iterators.
pub open spec fn compiled_bounds(
    g: Graph, chains: Seq<usize>, authors: Seq<usize>, heads: ISet<int>,
    perms: IMap<usize, Permission>, entries: Seq<Entry>, bounds: Seq<Option<crate::clock::NodeIdx>>,
    frontiers: IMap<usize, Seq<Entry>>,
) -> bool {
    &&& bounds.len() == entries.len()
    &&& forall|i: int| #![trigger bounds[i]] 0 <= i < entries.len() ==> {
        let (chain, tip) = entries[i];
        let author = authors[tip@ as int];
        match permission(perms, author) {
            Permission::Denied => bounds[i] == None,
            Permission::Unbounded => bounds[i] == Some(tip),
            Permission::Bounded(frontier) => frontiers.dom().contains(author)
                && summarizes(frontiers[author], chains,
                    history(g, frontier.intersect(history(g, heads))))
                && forall|p: usize| below(bounds[i], p) == member(frontiers[author], chain, p),
        }
    }
}

proof fn entry_permission(
    g: Graph, chains: Seq<usize>, authors: Seq<usize>, heads: ISet<int>,
    perms: IMap<usize, Permission>, entries: Seq<Entry>, bounds: Seq<Option<crate::clock::NodeIdx>>,
    frontiers: IMap<usize, Seq<Entry>>, i: int, a: usize,
)
    requires
        chain_invariant(g, chains, authors), a < g.len(),
        summarizes(entries, chains, history(g, heads)),
        compiled_bounds(g, chains, authors, heads, perms, entries, bounds, frontiers),
        0 <= i < entries.len(), entries[i].0 == chains[a as int], a <= entries[i].1@,
    ensures below(bounds[i], a)
        == allowed(g, history(g, heads), permission(perms, authors[a as int]), a as int),
{
    let tip = entries[i].1@;
    let author = authors[tip as int];
    assert(0 <= a <= tip < g.len());
    assert(chains[tip as int] == chains[a as int]);
    assert(reaches(g, a as int, tip as int));
    assert(author == authors[a as int]);
    assert(bounds[i] == bounds[i]);
    if let Permission::Bounded(frontier) = permission(perms, author) {
        clock_matches_history(g, chains, authors, frontier.intersect(history(g, heads)),
            frontiers[author], a);
        bounded_policy(g, history(g, heads), frontier, a as int);
    }
}

pub proof fn mask_matches_policy(
    g: Graph, chains: Seq<usize>, authors: Seq<usize>, heads: ISet<int>,
    perms: IMap<usize, Permission>, entries: Seq<Entry>, bounds: Seq<Option<crate::clock::NodeIdx>>,
    frontiers: IMap<usize, Seq<Entry>>, a: usize,
)
    requires
        chain_invariant(g, chains, authors), a < g.len(),
        summarizes(entries, chains, history(g, heads)),
        compiled_bounds(g, chains, authors, heads, perms, entries, bounds, frontiers),
    ensures permitted(entries, bounds, chains[a as int], a)
        == visible(g, authors, heads, perms, a as int),
{
    clock_matches_history(g, chains, authors, heads, entries, a);
    if permitted(entries, bounds, chains[a as int], a) {
        let i = choose|i: int| 0 <= i < entries.len() && entries[i].0 == chains[a as int]
            && a <= entries[i].1@ && below(bounds[i], a);
        assert(member(entries, chains[a as int], a));
        entry_permission(g, chains, authors, heads, perms, entries, bounds, frontiers, i, a);
    }
    if visible(g, authors, heads, perms, a as int) {
        let i = choose|i: int| 0 <= i < entries.len()
            && entries[i].0 == chains[a as int] && a <= entries[i].1@;
        entry_permission(g, chains, authors, heads, perms, entries, bounds, frontiers, i, a);
        assert(permitted(entries, bounds, chains[a as int], a));
    }
}

// No available frontier head means no grant, even if rejected heads have
// ancestors inside the delivered history.
pub proof fn unavailable_frontier_grants_nothing(
    g: Graph, h: ISet<int>, frontier: ISet<int>, a: int,
)
    requires forall|f: int| frontier.contains(f) ==> !h.contains(f),
    ensures !allowed(g, h, Permission::Bounded(frontier), a),
{}

} // verus!
