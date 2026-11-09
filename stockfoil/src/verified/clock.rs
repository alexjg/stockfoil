//! Shared coordinate and interval operations for the persistent ancestry clock.
//! AVL construction and the caller's sorted chain-node invariant are separate
//! proof obligations; these contracts do not assume that every tree is valid.
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

pub use super::node_key::NodeIdx;

verus! {

#[cfg(verus_keep_ghost)]
impl View for NodeIdx {
    type V = u32;
    closed spec fn view(&self) -> u32 { self.0 }
}

impl NodeIdx {
    pub(crate) fn new(index: u32) -> (out: Self)
        ensures out@ == index,
        no_unwind
    { Self(index) }

    pub(crate) fn index(self) -> (out: usize)
        ensures out == self@,
        no_unwind
    { self.0 as usize }

    pub proof fn same_index(a: Self, b: Self)
        requires a@ == b@,
        ensures a == b,
    {}

    pub closed spec fn spec_new(index: u32) -> Self { Self(index) }

    pub proof fn spec_new_index(index: u32)
        ensures Self::spec_new(index)@ == index,
    {}
}

pub open spec fn below(tip: Option<NodeIdx>, p: usize) -> bool {
    match tip { None => false, Some(tip) => p <= tip@ }
}

pub(crate) fn contains(tip: Option<NodeIdx>, p: NodeIdx) -> (out: bool)
    ensures out == below(tip, p@ as usize),
{
    match tip { None => false, Some(tip) => p.index() <= tip.index() }
}

pub(crate) fn cap(tip: NodeIdx, bound: Option<NodeIdx>) -> (out: Option<NodeIdx>)
    ensures
        out == match bound {
            None => None,
            Some(limit) => Some(if tip@ <= limit@ { tip } else { limit }),
        },
        forall|p: usize| below(out, p) == (p <= tip@ && below(bound, p)),
{
    match bound {
        None => None,
        Some(limit) => Some(if tip.index() <= limit.index() { tip } else { limit }),
    }
}

// A nondecreasing slice suffices for partitioning; uniqueness in graph output
// additionally depends on each chain table being strictly increasing.
pub open spec fn ordered(nodes: Seq<NodeIdx>) -> bool {
    forall|i: int, j: int| 0 <= i < j < nodes.len() ==> nodes[i]@ <= nodes[j]@
}

pub(crate) fn upper_bound(nodes: &[NodeIdx], tip: NodeIdx) -> (out: usize)
    requires ordered(nodes@),
    ensures
        out <= nodes.len(),
        forall|i: int| 0 <= i < nodes.len() ==> (i < out <==> nodes[i]@ <= tip@),
{
    let mut lo = 0;
    let mut hi = nodes.len();
    while lo < hi
        invariant
            ordered(nodes@), 0 <= lo <= hi <= nodes.len(),
            forall|i: int| 0 <= i < lo ==> nodes[i]@ <= tip@,
            forall|i: int| hi <= i < nodes.len() ==> nodes[i]@ > tip@,
        decreases hi - lo,
    {
        let mid = lo + (hi - lo) / 2;
        if nodes[mid].index() <= tip.index() {
            assert forall|i: int| 0 <= i <= mid implies nodes[i]@ <= tip@ by {
                if i < mid { assert(nodes[i]@ <= nodes[mid as int]@); }
            }
            lo = mid + 1;
        } else {
            assert forall|i: int| mid <= i < nodes.len() implies nodes[i]@ > tip@ by {
                if mid < i { assert(nodes[mid as int]@ <= nodes[i]@); }
            }
            hi = mid;
        }
    }
    lo
}

/// Exact slice for an inclusive prefix minus another inclusive prefix. A bound
/// above the new tip produces an empty, valid range rather than a reversed one.
pub(crate) fn difference_range(
    nodes: &[NodeIdx], after: NodeIdx, before: Option<NodeIdx>,
) -> (out: (usize, usize))
    requires ordered(nodes@),
    ensures
        out.0 <= out.1 <= nodes.len(),
        forall|i: int| 0 <= i < nodes.len() ==> (
            out.0 <= i < out.1 <==> nodes[i]@ <= after@ && !below(before, nodes[i]@ as usize)),
{
    let start = match before { None => 0, Some(tip) => upper_bound(nodes, tip) };
    let end = upper_bound(nodes, after);
    (if start <= end { start } else { end }, end)
}

pub proof fn absent_is_not_zero()
    ensures !below(None, 0), below(Some(NodeIdx::spec_new(0)), 0),
        !below(Some(NodeIdx::spec_new(0)), 1),
{ NodeIdx::spec_new_index(0); }

} // verus!
