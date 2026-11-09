//! Reference model of the former eager candidate planning and batch emission.
//! The current lazy canonical Traversal and public Materialization protocol are not proved here.
//! Reference eager candidate assembly connects to this model under explicit table
//! and clock premises. History identity and cursor publication remain wrapper
//! obligations. The batch boundary and classifier are shared code.
use vstd::prelude::*;
use crate::events::{self, Vis};
use crate::graph::*;
use crate::authorization::*;

verus! {

pub open spec fn candidates(
    delivered: ISet<int>, processed: ISet<int>, before: ISet<int>, after: ISet<int>,
) -> ISet<int> {
    delivered.difference(processed)
        .union(before.difference(after)).union(after.difference(before))
}

pub proof fn candidates_exact(
    delivered: ISet<int>, processed: ISet<int>, before: ISet<int>, after: ISet<int>, a: int,
)
    requires before.subset_of(delivered), after.subset_of(delivered),
    ensures candidates(delivered, processed, before, after).contains(a)
        == (delivered.contains(a)
            && (!processed.contains(a) || before.contains(a) != after.contains(a))),
{}

pub open spec fn pending_contract(
    pending: Seq<usize>, count: nat, processed: ISet<int>, before: ISet<int>, after: ISet<int>,
) -> bool {
    &&& forall|j: int| 0 <= j < pending.len() ==> pending[j] < count
    &&& forall|j: int, k: int| 0 <= j < k < pending.len() ==> pending[j] < pending[k]
    &&& forall|a: int| pending.contains(a as usize) && 0 <= a <= usize::MAX
        <==> 0 <= a < count && (!processed.contains(a) || before.contains(a) != after.contains(a))
}

// Independent policy expressed with parent edges; no chain IDs appear here.
pub proof fn planning_matches_policy(
    g: Graph, authors: Seq<usize>, processed_heads: ISet<int>,
    before_heads: ISet<int>, before_perms: IMap<usize, Permission>,
    after_heads: ISet<int>, after_perms: IMap<usize, Permission>, a: int,
)
    requires authors.len() == g.len(),
    ensures candidates(
        ISet::new(|i: int| 0 <= i < g.len()), history(g, processed_heads),
        ISet::new(|i: int| visible(g, authors, before_heads, before_perms, i)),
        ISet::new(|i: int| visible(g, authors, after_heads, after_perms, i)),
    ).contains(a) == (0 <= a < g.len() && (
        !history(g, processed_heads).contains(a)
        || visible(g, authors, before_heads, before_perms, a)
            != visible(g, authors, after_heads, after_perms, a))),
{
    candidates_exact(
        ISet::new(|i: int| 0 <= i < g.len()), history(g, processed_heads),
        ISet::new(|i: int| visible(g, authors, before_heads, before_perms, i)),
        ISet::new(|i: int| visible(g, authors, after_heads, after_perms, i)), a,
    );
}

pub struct Event {
    pub node: usize,
    pub newly_delivered: bool,
    pub visibility: Vis,
}

// An abstract batch emitter: payload/hash reads are replaced by boolean vectors.
// There is no proof here that the concrete History wrapper supplies these inputs.
pub fn emit_batch(
    pending: &Vec<usize>, next: usize, max: usize,
    processed: &Vec<bool>, before: &Vec<bool>, after: &Vec<bool>,
) -> (result: (usize, Vec<Event>))
    requires
        next <= pending.len(), processed.len() == before.len(), before.len() == after.len(),
        forall|j: int| 0 <= j < pending.len() ==> pending[j] < processed.len(),
    ensures
        next <= result.0 <= pending.len(),
        result.0 - next == result.1.len(), result.1.len() <= max,
        max > 0 && next < pending.len() ==> next < result.0,
        forall|j: int| #![trigger result.1[j]] 0 <= j < result.1.len() ==> {
            let e = result.1[j];
            &&& e.node == pending[next + j]
            &&& e.newly_delivered == !processed[e.node as int]
            &&& events::before(e.visibility) == before[e.node as int]
            &&& events::after(e.visibility) == after[e.node as int]
        },
{
    let end = events::batch_end(next, pending.len(), max);
    let mut i = next;
    let mut out: Vec<Event> = Vec::new();
    while i < end
        invariant
            next <= i <= end <= pending.len(), end - next <= max,
            max > 0 && next < pending.len() ==> next < end,
            processed.len() == before.len(), before.len() == after.len(),
            forall|j: int| 0 <= j < pending.len() ==> pending[j] < processed.len(),
            out.len() == i - next,
            forall|j: int| #![trigger out[j]] 0 <= j < out.len() ==> {
                let e = out[j];
                &&& e.node == pending[next + j]
                &&& e.newly_delivered == !processed[e.node as int]
                &&& events::before(e.visibility) == before[e.node as int]
                &&& events::after(e.visibility) == after[e.node as int]
            },
        decreases end - i,
    {
        let node = pending[i];
        out.push(Event {
            node, newly_delivered: !processed[node],
            visibility: events::classify(before[node], after[node]),
        });
        i += 1;
    }
    (end, out)
}

// Consecutive batches partition the captured pending sequence exactly. Thus a
// positive budget decreases the number remaining; zero budget makes no progress.
pub proof fn adjacent_batches(pending: Seq<usize>, start: int, mid: int, end: int)
    requires 0 <= start <= mid <= end <= pending.len(),
    ensures pending.subrange(start, mid) + pending.subrange(mid, end)
        =~= pending.subrange(start, end),
{}

pub proof fn pending_is_topological(
    g: Graph, pending: Seq<usize>, processed: ISet<int>, before: ISet<int>, after: ISet<int>,
    j: int, k: int,
)
    requires
        pending_contract(pending, g.len(), processed, before, after),
        0 <= j < pending.len(), 0 <= k < pending.len(),
    ensures
        pending[j] == pending[k] ==> j == k,
        pending[j] != pending[k] && reaches(g, pending[j] as int, pending[k] as int) ==> j < k,
{
    if reaches(g, pending[j] as int, pending[k] as int) {
        reaches_order(g, pending[j] as int, pending[k] as int);
    }
}

pub proof fn unchanged_cursor_emits_nothing(delivered: ISet<int>, visible: ISet<int>)
    ensures candidates(delivered, delivered, visible, visible) =~= ISet::empty(),
{}

pub proof fn new_invisible_commit_is_still_emitted()
    ensures candidates(ISet::empty().insert(0int), ISet::empty(), ISet::empty(), ISet::empty()).contains(0),
{}

} // verus!
