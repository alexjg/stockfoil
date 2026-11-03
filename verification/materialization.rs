//! Verified scan model for visit_changes. The callback/graph wrapper is not yet
//! verified; this executable model records events in a vector for specification.
use vstd::prelude::*;
use crate::events::{self, Vis};
use crate::graph::*;
use crate::visibility::*;

verus! {

pub struct Event {
    pub node: usize,
    pub newly_delivered: bool,
    pub visibility: Vis,
}

pub open spec fn emitted(out: Seq<Event>, node: int) -> bool {
    exists|j: int| 0 <= j < out.len() && out[j].node == node
}

pub open spec fn needed(processed: Seq<bool>, before: Seq<bool>, after: Seq<bool>, i: int) -> bool {
    !processed[i] || before[i] != after[i]
}

pub open spec fn events_for_prefix(
    events: Seq<Event>, processed: Seq<bool>, before: Seq<bool>, after: Seq<bool>, n: int,
) -> bool {
    &&& forall|j: int| #![trigger events[j]] 0 <= j < events.len() ==> {
        let event = events[j];
        &&& event.node < n
        &&& needed(processed, before, after, event.node as int)
        &&& event.newly_delivered == !processed[event.node as int]
        &&& events::before(event.visibility) == before[event.node as int]
        &&& events::after(event.visibility) == after[event.node as int]
    }
    &&& forall|i: int| 0 <= i < n && needed(processed, before, after, i)
        ==> exists|j: int| 0 <= j < events.len() && events[j].node == i
    &&& forall|j: int, k: int| 0 <= j < k < events.len() ==> events[j].node < events[k].node
}

pub fn scan(processed: &Vec<bool>, before: &Vec<bool>, after: &Vec<bool>) -> (out: Vec<Event>)
    requires processed.len() == before.len(), before.len() == after.len(),
    ensures events_for_prefix(out@, processed@, before@, after@, processed.len() as int),
{
    let mut out: Vec<Event> = Vec::new();
    let mut i: usize = 0;
    while i < processed.len()
        invariant
            processed.len() == before.len(), before.len() == after.len(),
            i <= processed.len(), out.len() <= i,
            events_for_prefix(out@, processed@, before@, after@, i as int),
        decreases processed.len() - i,
    {
        let new = !processed[i];
        let visibility = events::classify(before[i], after[i]);
        let ghost previous = out@;
        if events::should_emit(new, visibility) {
            out.push(Event { node: i, newly_delivered: new, visibility });
        }
        proof {
            assert forall|node: int| 0 <= node <= i && needed(processed@, before@, after@, node)
                implies exists|j: int| 0 <= j < out.len() && out[j].node == node by {
                if node < i {
                    let j = choose|j: int| 0 <= j < previous.len() && previous[j].node == node;
                    assert(out[j] == previous[j]);
                } else {
                    assert(out[previous.len() as int].node == node);
                }
            }
        }
        i += 1;
    }
    out
}

// Connect the scan's boolean inputs to the graph/permission specification.
pub open spec fn scan_inputs(
    g: Graph, authors: Seq<usize>, processed_heads: ISet<int>,
    before_heads: ISet<int>, before_perms: IMap<usize, Permission>,
    after_heads: ISet<int>, after_perms: IMap<usize, Permission>,
    processed: Seq<bool>, before: Seq<bool>, after: Seq<bool>,
) -> bool {
    &&& authors.len() == g.len()
    &&& processed.len() == g.len() && before.len() == g.len() && after.len() == g.len()
    &&& forall|i: int| 0 <= i < g.len() ==> (
        processed[i] == history(g, processed_heads).contains(i)
        && before[i] == visible(g, authors, before_heads, before_perms, i)
        && after[i] == visible(g, authors, after_heads, after_perms, i))
}

pub proof fn materialization_exact(
    g: Graph, authors: Seq<usize>, processed_heads: ISet<int>,
    before_heads: ISet<int>, before_perms: IMap<usize, Permission>,
    after_heads: ISet<int>, after_perms: IMap<usize, Permission>,
    processed: Seq<bool>, before: Seq<bool>, after: Seq<bool>, out: Seq<Event>,
)
    requires
        dag(g),
        scan_inputs(g, authors, processed_heads, before_heads, before_perms,
            after_heads, after_perms, processed, before, after),
        events_for_prefix(out, processed, before, after, g.len() as int),
    ensures
        // Completeness and soundness, including new commits invisible in both views.
        forall|i: int| 0 <= i < g.len() ==>
            (emitted(out, i)
                == (!history(g, processed_heads).contains(i)
                    || visible(g, authors, before_heads, before_perms, i)
                        != visible(g, authors, after_heads, after_perms, i))),
        // Uniqueness.
        forall|j: int, k: int| 0 <= j < out.len() && 0 <= k < out.len()
            && out[j].node == out[k].node ==> j == k,
        // Ancestors precede descendants among emitted events, even across gaps.
        forall|j: int, k: int| 0 <= j < out.len() && 0 <= k < out.len()
            && out[j].node != out[k].node
            && reaches(g, out[j].node as int, out[k].node as int) ==> j < k,
{
    assert forall|i: int| 0 <= i < g.len() implies
        (emitted(out, i)
            == (!history(g, processed_heads).contains(i)
                || visible(g, authors, before_heads, before_perms, i)
                    != visible(g, authors, after_heads, after_perms, i))) by {
        assert(processed[i] == history(g, processed_heads).contains(i));
        assert(before[i] == visible(g, authors, before_heads, before_perms, i));
        assert(after[i] == visible(g, authors, after_heads, after_perms, i));
        if exists|j: int| 0 <= j < out.len() && out[j].node == i {
            let j = choose|j: int| 0 <= j < out.len() && out[j].node == i;
            assert(needed(processed, before, after, out[j].node as int));
        }
        if needed(processed, before, after, i) {
            assert(exists|j: int| 0 <= j < out.len() && out[j].node == i);
        }
    }
    assert forall|j: int, k: int| 0 <= j < out.len() && 0 <= k < out.len()
        && out[j].node != out[k].node
        && reaches(g, out[j].node as int, out[k].node as int) implies j < k by {
        reaches_order(g, out[j].node as int, out[k].node as int);
    }
}

pub proof fn unchanged_cursor_emits_nothing(
    processed: Seq<bool>, before: Seq<bool>, after: Seq<bool>, out: Seq<Event>,
)
    requires
        processed.len() == before.len(), before == after,
        forall|i: int| 0 <= i < processed.len() ==> processed[i],
        events_for_prefix(out, processed, before, after, processed.len() as int),
    ensures out.len() == 0,
{
    if out.len() > 0 {
        assert(needed(processed, before, after, out[0].node as int));
    }
}

} // verus!
