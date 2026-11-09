//! Connect the production interval search to parent-edge reachability. The AVL
//! lookups and chain-node table must establish the explicit boundary contracts.
use vstd::prelude::*;
use crate::clock::{self, NodeIdx, below};
use crate::coordinates::{Entry, member};
use crate::graph::*;

verus! {

pub fn history_difference_range(
    nodes: &[NodeIdx], after: NodeIdx, before: Option<NodeIdx>,
    Ghost(g): Ghost<Graph>, Ghost(chains): Ghost<Seq<usize>>,
    Ghost(authors): Ghost<Seq<usize>>, Ghost(chain): Ghost<usize>,
    Ghost(after_heads): Ghost<ISet<int>>, Ghost(before_heads): Ghost<ISet<int>>,
    Ghost(after_entries): Ghost<Seq<Entry>>, Ghost(before_entries): Ghost<Seq<Entry>>,
) -> (range: (usize, usize))
    requires
        chain_invariant(g, chains, authors), clock::ordered(nodes@),
        summarizes(after_entries, chains, history(g, after_heads)),
        summarizes(before_entries, chains, history(g, before_heads)),
        forall|i: int| 0 <= i < nodes.len() ==>
            nodes[i]@ < g.len() && chains[nodes[i]@ as int] == chain,
        // These are the contracts needed from AVL lookup, not assumed axioms.
        forall|p: usize| below(Some(after), p) == member(after_entries, chain, p),
        forall|p: usize| below(before, p) == member(before_entries, chain, p),
    ensures
        range.0 <= range.1 <= nodes.len(),
        forall|i: int| 0 <= i < nodes.len() ==> (
            range.0 <= i < range.1 <==>
                history(g, after_heads).contains(nodes[i]@ as int)
                && !history(g, before_heads).contains(nodes[i]@ as int)),
{
    let range = clock::difference_range(nodes, after, before);
    proof {
        assert forall|i: int| 0 <= i < nodes.len() implies (
            range.0 <= i < range.1 <==>
                history(g, after_heads).contains(nodes[i]@ as int)
                && !history(g, before_heads).contains(nodes[i]@ as int)) by {
            let p = nodes[i]@ as usize;
            clock_matches_history(g, chains, authors, after_heads, after_entries, p);
            clock_matches_history(g, chains, authors, before_heads, before_entries, p);
        }
    }
    range
}

// Set-level contract of fragment_members: inclusive head history minus all
// boundary histories. Boundary discovery and admission checks remain unproved.
pub proof fn fragment_membership(g: Graph, head: int, boundary: ISet<int>, a: int)
    ensures
        history(g, ISet::empty().insert(head)).difference(history(g, boundary)).contains(a)
        == (reaches(g, a, head)
            && !(exists|b: int| boundary.contains(b) && reaches(g, a, b))),
{
    let heads = ISet::empty().insert(head);
    if reaches(g, a, head) {
        assert(heads.contains(head));
    }
    if history(g, heads).contains(a) {
        let h = choose|h: int| heads.contains(h) && reaches(g, a, h);
        assert(h == head);
    }
}

} // verus!
