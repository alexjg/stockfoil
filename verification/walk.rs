//! Semantic invariant for the cached parent walk. The shared ancestry_walk module
//! now refines the actual stack/visited implementation to this set invariant.
use vstd::prelude::*;
use crate::graph::*;

verus! {

pub open spec fn walk_invariant(g: Graph, heads: ISet<int>, covered: ISet<int>, pending: ISet<int>) -> bool {
    &&& dag(g)
    &&& forall|n: int| covered.contains(n) || pending.contains(n)
        ==> 0 <= n < g.len() && history(g, heads).contains(n)
    &&& forall|n: int| heads.contains(n) ==> covered.contains(n) || pending.contains(n)
    &&& forall|n: int, p: int| covered.contains(n) && g[n].contains(p)
        ==> covered.contains(p) || pending.contains(p)
}

pub proof fn start(g: Graph, heads: ISet<int>)
    requires dag(g), forall|h: int| heads.contains(h) ==> 0 <= h < g.len(),
    ensures walk_invariant(g, heads, ISet::empty(), heads),
{
    assert forall|h: int| heads.contains(h) implies history(g, heads).contains(h) by {
        unfold_reaches(g, h, h);
    }
}

pub proof fn expand(g: Graph, heads: ISet<int>, covered: ISet<int>, pending: ISet<int>, n: int)
    requires walk_invariant(g, heads, covered, pending), pending.contains(n),
    ensures walk_invariant(g, heads, covered.insert(n), pending.remove(n).union(g[n])),
{
    assert forall|p: int| g[n].contains(p) implies
        0 <= p < g.len() && history(g, heads).contains(p) by {
        assert(0 <= p < n);
        unfold_reaches(g, p, p);
        unfold_reaches(g, p, n);
        history_closed(g, heads, p, n);
    }
}

pub proof fn revisit(g: Graph, heads: ISet<int>, covered: ISet<int>, pending: ISet<int>, n: int)
    requires walk_invariant(g, heads, covered, pending), covered.contains(n),
    ensures walk_invariant(g, heads, covered, pending.remove(n)),
{}

// A cached full ancestry clock can replace all traversal below n.
// A partially accumulated coordinate cannot satisfy this premise in general.
pub proof fn use_cache(
    g: Graph, heads: ISet<int>, covered: ISet<int>, pending: ISet<int>, n: int, cache: ISet<int>,
)
    requires
        walk_invariant(g, heads, covered, pending), pending.contains(n),
        cache == history(g, ISet::empty().insert(n)),
    ensures walk_invariant(g, heads, covered.union(cache), pending.remove(n)),
{
    unfold_reaches(g, n, n);
    assert(ISet::empty().insert(n).contains(n));
    assert(cache.contains(n));
    assert forall|a: int| cache.contains(a) implies
        0 <= a < g.len() && history(g, heads).contains(a) by {
        let h = choose|h: int| ISet::empty().insert(n).contains(h) && reaches(g, a, h);
        assert(h == n);
        reaches_order(g, a, n);
        history_closed(g, heads, a, n);
    }
    assert forall|a: int, p: int| cache.contains(a) && g[a].contains(p)
        implies cache.contains(p) by {
        unfold_reaches(g, p, p);
        unfold_reaches(g, p, a);
        history_closed(g, ISet::empty().insert(n), p, a);
    }
}

proof fn parent_closed_contains_ancestors(g: Graph, covered: ISet<int>, a: int, d: int)
    requires
        dag(g), covered.contains(d), reaches(g, a, d),
        forall|n: int, p: int| covered.contains(n) && g[n].contains(p) ==> covered.contains(p),
    ensures covered.contains(a),
    decreases d,
{
    unfold_reaches(g, a, d);
    if a != d {
        let p = choose|p: int| 0 <= p < d && g[d].contains(p) && reaches(g, a, p);
        parent_closed_contains_ancestors(g, covered, a, p);
    }
}

pub proof fn finish(g: Graph, heads: ISet<int>, covered: ISet<int>)
    requires walk_invariant(g, heads, covered, ISet::empty()),
    ensures covered =~= history(g, heads),
{
    assert forall|a: int| history(g, heads).contains(a) implies covered.contains(a) by {
        let h = choose|h: int| heads.contains(h) && reaches(g, a, h);
        parent_closed_contains_ancestors(g, covered, a, h);
    }
}

} // verus!
