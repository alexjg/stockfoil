//! Audited standard-library contracts. See verification/TRUSTED_STDLIB.md.
use super::clock::{self, NodeIdx};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

// TRUST SORT-1: std slice sorting and NodeIdx's derived u32 ordering.
#[verifier::external_body]
pub(crate) fn sort_nodes(nodes: &mut Vec<NodeIdx>)
    ensures
        clock::ordered(final(nodes)@),
        final(nodes)@.to_multiset() == old(nodes)@.to_multiset(),
        forall|node: NodeIdx| final(nodes)@.contains(node) == old(nodes)@.contains(node),
{
    nodes.sort_unstable();
}

// TRUST DEDUP-1: std Vec deduplication and NodeIdx's derived u32 equality.
#[verifier::external_body]
pub(crate) fn dedup_nodes(nodes: &mut Vec<NodeIdx>)
    requires clock::ordered(old(nodes)@),
    ensures
        final(nodes).len() <= old(nodes).len(),
        forall|i: int, j: int| 0 <= i < j < final(nodes).len()
            ==> final(nodes)@[i]@ < final(nodes)@[j]@,
        forall|node: NodeIdx| final(nodes)@.contains(node) == old(nodes)@.contains(node),
{
    nodes.dedup();
}

} // verus!
