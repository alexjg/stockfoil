//! Mathematical view of persistent AVL clock entries. `tree_refinement` connects
//! this view to actual tree lookup and masking; history callers remain to prove.
use vstd::prelude::*;
pub(crate) use crate::clock::NodeIdx;
use crate::clock::below;
verus! {
pub type Entry = (usize, NodeIdx);

pub open spec fn sorted(entries: Seq<Entry>) -> bool {
    forall|i: int, j: int| 0 <= i < j < entries.len() ==>
        entries[i].0 < entries[j].0
}

pub open spec fn member(entries: Seq<Entry>, chain: usize, position: usize) -> bool {
    exists|i: int| 0 <= i < entries.len()
        && entries[i].0 == chain && position <= entries[i].1@
}

pub open spec fn permitted(
    entries: Seq<Entry>, bounds: Seq<Option<NodeIdx>>, chain: usize, p: usize,
) -> bool {
    exists|i: int| 0 <= i < entries.len() && entries[i].0 == chain
        && p <= entries[i].1@ && below(bounds[i], p)
}

} // verus!
