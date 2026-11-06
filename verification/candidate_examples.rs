//! Nonempty calls of the production assembly, including overlapping reasons.
use vstd::prelude::*;
use crate::clock::NodeIdx;
use crate::tree::Clock;
use crate::candidates;

verus! {

pub fn overlapping_delivery_and_visibility_candidates() {
    let zero = NodeIdx::new(0);
    let one = NodeIdx::new(1);
    let two = NodeIdx::new(2);
    let three = NodeIdx::new(3);
    let tables = vec![vec![zero, two], vec![one, three]];
    let delivered = Clock::from_tips(&vec![two, three]);
    let processed = Clock::from_tips(&vec![zero, one]);
    let before = Clock::default();
    let after = Clock::from_tips(&vec![two, one]);
    proof {
        delivered.sequence_correct(); processed.sequence_correct(); after.sequence_correct();
        assert(delivered.sequence() =~= seq![(0u32, two), (1u32, three)]);
        assert(processed.sequence() =~= seq![(0u32, zero), (1u32, one)]);
        assert(after.sequence() =~= seq![(0u32, two), (1u32, one)]);
        assert(after.lookup(0) == Some(two));
        assert(after.lookup(1) == Some(one));
    }
    let pending = candidates::assemble(&tables, &delivered, &processed, &before, &after);
    assert(crate::difference::delta_member(crate::difference::tables_view(tables@), &after, &before, zero)) by {
        assert(tables@[0]@.contains(zero));
    }
    assert(crate::difference::delta_member(crate::difference::tables_view(tables@), &after, &before, one)) by {
        assert(tables@[1]@.contains(one));
    }
    assert(pending@.contains(zero)); // Existing node becoming visible.
    assert(pending@.contains(one));
    assert(pending@.contains(two)); // Both new delivery and becoming visible.
    assert(pending@.contains(three)); // New delivery even though invisible.
    assert(candidates::strictly_ordered(pending@));
    assert forall|i: int, j: int| 0 <= i < j < pending.len() implies pending@[i] != pending@[j] by {};
    let ghost chains = seq![0u32, 1u32, 0u32, 1u32];
    proof {
        assert forall|node: NodeIdx| node@ < chains.len() implies chains[node@ as int] < tables.len()
            && crate::difference::tables_view(tables@)[chains[node@ as int] as int].contains(node) by {
            if node@ == 0 { NodeIdx::same_index(node, zero); }
            else if node@ == 1 { NodeIdx::same_index(node, one); }
            else if node@ == 2 { NodeIdx::same_index(node, two); }
            else { NodeIdx::same_index(node, three); }
        }
        assert(crate::candidate_refinement::table_layout(crate::difference::tables_view(tables@), chains));
        assert forall|a: int| 0 <= a < chains.len() implies
            crate::candidate_refinement::members(&delivered, chains).contains(a) by {};
    }
    let modeled = crate::candidate_refinement::assemble_for_model(
        &tables, &delivered, &processed, &before, &after, Ghost(chains));
    assert(crate::materialization::pending_contract(crate::candidate_refinement::indices(modeled@), 4,
        crate::candidate_refinement::members(&processed, chains),
        crate::candidate_refinement::members(&before, chains),
        crate::candidate_refinement::members(&after, chains)));
    let unchanged = candidates::assemble(&tables, &delivered, &delivered, &after, &after);
    assert(unchanged.len() == 0) by {
        if unchanged.len() > 0 { assert(unchanged@.contains(unchanged@[0])); }
    }
}

} // verus!
