//! Maximal parent-edge vertices cover a finite, parents-first graph.
use vstd::prelude::*;
use crate::graph::{self, Graph, dag, reaches, history};

verus! {

pub(crate) open spec fn leaf(g: Graph, n: int) -> bool {
    0 <= n < g.len() && forall|d: int| 0 <= d < g.len() ==> !g[d].contains(n)
}

pub(crate) open spec fn heads(g: Graph) -> ISet<int> { ISet::new(|n: int| leaf(g, n)) }
pub(crate) open spec fn all(g: Graph) -> ISet<int> { ISet::new(|n: int| 0 <= n < g.len()) }

pub(crate) proof fn appended_heads(g: Graph, parents: ISet<int>)
    requires dag(g), forall|p: int| parents.contains(p) ==> 0 <= p < g.len(),
    ensures heads(g.push(parents)) == heads(g).difference(parents).insert(g.len() as int),
{
    assert forall|n: int| leaf(g.push(parents), n)
        == (leaf(g, n) && !parents.contains(n) || n == g.len()) by {
        if n == g.len() {
            assert forall|d: int| 0 <= d < g.push(parents).len() implies !g.push(parents)[d].contains(n) by {
                if d < g.len() { assert(!g[d].contains(n)); }
            }
        }
        if leaf(g, n) && !parents.contains(n) {
            assert forall|d: int| 0 <= d < g.push(parents).len() implies !g.push(parents)[d].contains(n) by {};
        }
        if leaf(g.push(parents), n) && n < g.len() {
            assert forall|d: int| 0 <= d < g.len() implies !g[d].contains(n) by {
                assert(g.push(parents)[d] == g[d]);
            }
            assert(!g.push(parents)[g.len() as int].contains(n));
        }
    }
    assert(heads(g.push(parents)) =~= heads(g).difference(parents).insert(g.len() as int));
}

proof fn head_above(g: Graph, a: int)
    requires dag(g), 0 <= a < g.len(),
    ensures exists|h: int| heads(g).contains(h) && reaches(g, a, h),
    decreases g.len() - a,
{
    graph::unfold_reaches(g, a, a);
    if leaf(g, a) {
        assert(heads(g).contains(a));
    } else {
        let d = choose|d: int| 0 <= d < g.len() && g[d].contains(a);
        assert(a < d);
        graph::unfold_reaches(g, a, d);
        assert(reaches(g, a, d));
        head_above(g, d);
        let h = choose|h: int| heads(g).contains(h) && reaches(g, d, h);
        graph::transitive(g, a, d, h);
    }
}

pub(crate) proof fn heads_cover(g: Graph)
    requires dag(g),
    ensures history(g, heads(g)) == all(g),
{
    assert forall|a: int| history(g, heads(g)).contains(a) == all(g).contains(a) by {
        if 0 <= a < g.len() { head_above(g, a); }
        if history(g, heads(g)).contains(a) {
            let h = choose|h: int| heads(g).contains(h) && reaches(g, a, h);
            graph::reaches_order(g, a, h);
        }
    }
    assert(history(g, heads(g)) =~= all(g));
}

} // verus!
