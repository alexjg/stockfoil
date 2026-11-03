//! Independent graph semantics: only parent edges define reachability.
use vstd::prelude::*;
use crate::clock::{Entry, member, sorted};

verus! {

pub type Graph = Seq<ISet<int>>;

pub open spec fn dag(g: Graph) -> bool {
    forall|d: int, p: int| 0 <= d < g.len() && g[d].contains(p) ==> 0 <= p < d
}

// Enumerate possible parent IDs in the specification only. This avoids recursive
// calls under a quantifier; unfold_reaches proves the ordinary parent-path rule.
pub open spec fn reach_prefix(g: Graph, a: int, d: int, limit: int) -> bool
    decreases d, limit,
{
    0 <= d < g.len() && 0 <= limit <= d && (
        if limit == 0 { a == d } else {
            reach_prefix(g, a, d, limit - 1)
            || (g[d].contains(limit - 1)
                && reach_prefix(g, a, limit - 1, limit - 1))
        }
    )
}

pub open spec fn reaches(g: Graph, a: int, d: int) -> bool {
    reach_prefix(g, a, d, d)
}

proof fn unfold_prefix(g: Graph, a: int, d: int, limit: int)
    requires 0 <= limit <= d < g.len(),
    ensures reach_prefix(g, a, d, limit) == (a == d || exists|p: int|
        0 <= p < limit && g[d].contains(p) && reaches(g, a, p)),
    decreases limit,
{
    reveal_with_fuel(reach_prefix, 2);
    if limit > 0 {
        unfold_prefix(g, a, d, limit - 1);
        assert((exists|p: int| 0 <= p < limit && g[d].contains(p) && reaches(g, a, p))
            == ((exists|p: int| 0 <= p < limit - 1 && g[d].contains(p) && reaches(g, a, p))
                || (g[d].contains(limit - 1) && reaches(g, a, limit - 1)))) by {
            if exists|p: int| 0 <= p < limit && g[d].contains(p) && reaches(g, a, p) {
                let p = choose|p: int| 0 <= p < limit && g[d].contains(p) && reaches(g, a, p);
                if p < limit - 1 {
                    assert(exists|q: int| 0 <= q < limit - 1 && g[d].contains(q) && reaches(g, a, q));
                }
            }
            if exists|p: int| 0 <= p < limit - 1 && g[d].contains(p) && reaches(g, a, p) {
                let p = choose|p: int| 0 <= p < limit - 1 && g[d].contains(p) && reaches(g, a, p);
                assert(exists|q: int| 0 <= q < limit && g[d].contains(q) && reaches(g, a, q));
            }
        }
    }
}

pub proof fn unfold_reaches(g: Graph, a: int, d: int)
    requires 0 <= d < g.len(),
    ensures reaches(g, a, d) == (a == d || exists|p: int|
        0 <= p < d && g[d].contains(p) && reaches(g, a, p)),
{
    unfold_prefix(g, a, d, d);
}

pub open spec fn history(g: Graph, heads: ISet<int>) -> ISet<int> {
    ISet::new(|a: int| exists|h: int| heads.contains(h) && reaches(g, a, h))
}

pub proof fn reaches_order(g: Graph, a: int, d: int)
    requires reaches(g, a, d),
    ensures 0 <= a <= d < g.len(),
    decreases d,
{
    unfold_reaches(g, a, d);
    if a != d {
        let p = choose|p: int| 0 <= p < d && g[d as int].contains(p) && #[trigger] reaches(g, a, p);
        reaches_order(g, a, p);
    }
}

pub proof fn transitive(g: Graph, a: int, b: int, d: int)
    requires reaches(g, a, b), reaches(g, b, d),
    ensures reaches(g, a, d),
    decreases d,
{
    unfold_reaches(g, b, d);
    unfold_reaches(g, a, d);
    if b != d {
        let p = choose|p: int| 0 <= p < d && g[d as int].contains(p) && #[trigger] reaches(g, b, p);
        transitive(g, a, b, p);
        assert(g[d as int].contains(p) && reaches(g, a, p));
    }
}

pub proof fn history_closed(g: Graph, heads: ISet<int>, a: int, d: int)
    requires history(g, heads).contains(d), reaches(g, a, d),
    ensures history(g, heads).contains(a),
{
    let h = choose|h: int| heads.contains(h) && reaches(g, d, h);
    transitive(g, a, d, h);
}

// This is the semantic reason existing ancestry caches survive graph append.
pub proof fn append_preserves_reachability(g: Graph, parents: ISet<int>, a: int, d: int)
    requires 0 <= d < g.len(),
    ensures reaches(g.push(parents), a, d) == reaches(g, a, d),
    decreases d,
{
    unfold_reaches(g, a, d);
    unfold_reaches(g.push(parents), a, d);
    assert forall|p: int| 0 <= p < d implies
        reaches(g.push(parents), a, p) == reaches(g, a, p) by {
        append_preserves_reachability(g, parents, a, p);
    }
    if reaches(g, a, d) && a != d {
        let p = choose|p: int| 0 <= p < d && g[d].contains(p) && reaches(g, a, p);
        assert(reaches(g.push(parents), a, p));
        assert(g.push(parents)[d].contains(p));
    }
    if reaches(g.push(parents), a, d) && a != d {
        let next = g.push(parents);
        let p = choose|p: int| 0 <= p < d && next[d].contains(p) && reaches(next, a, p);
        assert(reaches(g, a, p));
        assert(g[d].contains(p));
    }
}

// Parent expansion used by an uncached ancestry walk. Set semantics intentionally
// ignore repeated parent entries, which do not change reachability.
pub proof fn expand_head(g: Graph, head: int)
    requires dag(g), 0 <= head < g.len(),
    ensures history(g, ISet::empty().insert(head))
        =~= ISet::empty().insert(head).union(history(g, g[head as int])),
{
    assert forall|a: int| history(g, ISet::empty().insert(head)).contains(a)
        == ISet::empty().insert(head).union(history(g, g[head as int])).contains(a) by {
        unfold_reaches(g, a, head);
        if reaches(g, a, head) && a != head {
            let p = choose|p: int| 0 <= p < head && g[head].contains(p) && reaches(g, a, p);
            assert(history(g, g[head]).contains(a));
        }
        if history(g, g[head as int]).contains(a) {
            let p = choose|p: int| g[head as int].contains(p) && reaches(g, a, p);
            assert(p < head);
            assert(reaches(g, a, head));
        }
        let singleton = ISet::empty().insert(head);
        if history(g, singleton).contains(a) {
            let h = choose|h: int| singleton.contains(h) && reaches(g, a, h);
            assert(h == head);
        }
        if reaches(g, a, head) {
            assert(singleton.contains(head));
            assert(history(g, singleton).contains(a));
        }
    }
}

pub open spec fn chain_invariant(g: Graph, chains: Seq<usize>, authors: Seq<usize>) -> bool {
    &&& dag(g)
    &&& chains.len() == g.len()
    &&& authors.len() == g.len()
    &&& forall|a: int, d: int| 0 <= a <= d < g.len() && chains[a] == chains[d]
        ==> reaches(g, a as int, d as int) && authors[a] == authors[d]
}

// The boundary needed from ClockBuilder: every coordinate is an actual history
// member, and it is at least as large as every history member on its chain.
// This does NOT assume that lookup is exact; that is proved below.
pub open spec fn summarizes(entries: Seq<Entry>, chains: Seq<usize>, h: ISet<int>) -> bool {
    &&& sorted(entries)
    &&& forall|i: int| #![trigger entries[i]] 0 <= i < entries.len() ==> {
        let (c, node) = entries[i];
        let tip = node@;
        tip < chains.len() && chains[tip as int] == c && h.contains(tip as int)
    }
    &&& forall|a: int| h.contains(a) ==> 0 <= a < chains.len()
        && exists|i: int| 0 <= i < entries.len()
            && entries[i].0 == chains[a as int] && a <= entries[i].1@
}

pub proof fn clock_matches_history(
    g: Graph, chains: Seq<usize>, authors: Seq<usize>, heads: ISet<int>, entries: Seq<Entry>,
    a: usize,
)
    requires
        chain_invariant(g, chains, authors), a < g.len(),
        summarizes(entries, chains, history(g, heads)),
    ensures member(entries, chains[a as int], a) == history(g, heads).contains(a as int),
{
    if member(entries, chains[a as int], a) {
        let i = choose|i: int| 0 <= i < entries.len()
            && entries[i].0 == chains[a as int] && a <= entries[i].1@;
        let tip = entries[i].1@;
        assert(chains[a as int] == chains[tip as int]);
        assert(reaches(g, a as int, tip as int));
        history_closed(g, heads, a as int, tip as int);
    }
}

// Compose the representation theorem with the exact executable lookup used by
// Stockfoil. Ghost inputs and the graph theorem erase in an ordinary Rust build.
pub fn contains_history(
    entries: &Vec<Entry>, chain: usize, a: usize,
    Ghost(g): Ghost<Graph>, Ghost(chains): Ghost<Seq<usize>>,
    Ghost(authors): Ghost<Seq<usize>>, Ghost(heads): Ghost<ISet<int>>,
) -> (result: bool)
    requires
        chain_invariant(g, chains, authors), a < g.len(), chain == chains[a as int],
        summarizes(entries@, chains, history(g, heads)),
    ensures result == history(g, heads).contains(a as int),
{
    proof { clock_matches_history(g, chains, authors, heads, entries@, a); }
    crate::clock::contains(entries, chain, crate::clock::NodeIdx::new(a))
}

// Reusing a same-author maximal chain tip in the parents' history preserves the
// invariant, whether that tip is a direct parent or an indirect ancestor.
pub proof fn extend_chain(
    g: Graph, chains: Seq<usize>, authors: Seq<usize>, parents: ISet<int>,
    chain: usize, author: usize, reuse: Option<int>,
)
    requires
        chain_invariant(g, chains, authors),
        forall|p: int| parents.contains(p) ==> 0 <= p < g.len(),
        match reuse {
            None => forall|i: int| 0 <= i < chains.len() ==> chains[i] != chain,
            Some(tip) => 0 <= tip < g.len() && chains[tip as int] == chain
                && authors[tip as int] == author && history(g, parents).contains(tip)
                && forall|i: int| 0 <= i < chains.len() && chains[i] == chain ==> i <= tip,
        },
    ensures chain_invariant(g.push(parents), chains.push(chain), authors.push(author)),
{
    let next = g.push(parents);
    let labels = chains.push(chain);
    let owners = authors.push(author);
    assert forall|a: int, d: int| 0 <= a <= d < next.len() && labels[a] == labels[d]
        implies reaches(next, a as int, d as int) && owners[a] == owners[d] by {
        unfold_reaches(next, a, d);
        if d < g.len() {
            append_preserves_reachability(g, parents, a as int, d as int);
        } else if a < d {
            let tip = reuse.unwrap();
            assert(a <= tip);
            assert(reaches(g, a as int, tip));
            history_closed(g, parents, a as int, tip);
            let p = choose|p: int| parents.contains(p) && reaches(g, a as int, p);
            append_preserves_reachability(g, parents, a as int, p);
            assert(reaches(next, a as int, p));
        }
    }
}

} // verus!
