//! Runtime head-index normalization and a verification-only eager set model.
//! Production traversal is lazy and canonically ordered by ready commit hashes.
#[cfg(verus_keep_ghost)]
use super::clock;
use super::clock::NodeIdx;
#[cfg(verus_keep_ghost)]
use super::{difference, tree::Clock};
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

pub(crate) open spec fn wanted(
    tables: Seq<Seq<NodeIdx>>, delivered: &Clock, processed: &Clock,
    before: &Clock, after: &Clock, node: NodeIdx,
) -> bool {
    difference::delta_member(tables, delivered, processed, node)
        || difference::delta_member(tables, before, after, node)
        || difference::delta_member(tables, after, before, node)
}

/// Reference set assembly; not the rewritten production Traversal.
#[cfg(verus_keep_ghost)]
pub(crate) fn assemble(
    tables: &[Vec<NodeIdx>], delivered: &Clock, processed: &Clock,
    before: &Clock, after: &Clock,
) -> (pending: Vec<NodeIdx>)
    requires delivered.ordered(), processed.ordered(), before.ordered(), after.ordered(),
        forall|c: int| 0 <= c < tables.len() ==> clock::ordered(tables[c]@),
        forall|c: u32| delivered.contents().contains_key(c) || before.contents().contains_key(c)
            || after.contents().contains_key(c) ==> c < tables.len(),
    ensures strictly_ordered(pending@),
        forall|node: NodeIdx| pending@.contains(node)
            == wanted(difference::tables_view(tables@), delivered, processed, before, after, node),
{
    let mut pending = Vec::new();
    difference::difference(tables, delivered, processed, &mut pending);
    difference::symmetric_difference(tables, before, after, &mut pending);
    proof {
        broadcast use vstd::seq_lib::lemma_seq_concat_contains_all_elements;
        assert forall|node: NodeIdx| pending@.contains(node)
            == wanted(difference::tables_view(tables@), delivered, processed, before, after, node) by {
            difference::difference_membership(difference::tables_view(tables@), delivered, processed, node);
            difference::difference_membership(difference::tables_view(tables@), before, after, node);
            difference::difference_membership(difference::tables_view(tables@), after, before, node);
        }
    }
    normalize(&mut pending);
    pending
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
