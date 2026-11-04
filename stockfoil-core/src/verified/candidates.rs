//! Shared candidate normalization; only the two stdlib calls are trusted.
use super::clock::NodeIdx;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn strictly_ordered(nodes: Seq<NodeIdx>) -> bool {
    forall|i: int, j: int| 0 <= i < j < nodes.len() ==> nodes[i]@ < nodes[j]@
}

pub(crate) fn normalize(nodes: &mut Vec<NodeIdx>)
    ensures
        strictly_ordered(final(nodes)@),
        forall|node: NodeIdx| final(nodes)@.contains(node) == old(nodes)@.contains(node),
{
    super::stdlib::sort_nodes(nodes);
    super::stdlib::dedup_nodes(nodes);
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_matches_numeric_set() {
        let mut state = 19u32;
        for len in 0..128 {
            let mut values = vec![u32::MAX, 0, u32::MAX, 0];
            for _ in 0..len {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                values.push(state % 17);
            }
            let expected: std::collections::BTreeSet<u32> = values.iter().copied().collect();
            let mut nodes: Vec<_> = values.into_iter().map(NodeIdx::new).collect();
            normalize(&mut nodes);
            assert_eq!(
                nodes.iter().map(|n| n.index() as u32).collect::<Vec<_>>(),
                expected.into_iter().collect::<Vec<_>>()
            );
        }
        let mut empty = Vec::new();
        normalize(&mut empty);
        assert!(empty.is_empty());
    }
}
