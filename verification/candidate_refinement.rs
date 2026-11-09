//! Conditional bridge from reference eager candidate assembly to the batch model.
//! Concrete History construction must establish the complete table layout.
use vstd::prelude::*;
use crate::clock::{NodeIdx, below};
use crate::tree::Clock;
use crate::{candidates, difference, materialization};
pub(crate) use crate::chain_tables::table_layout;

verus! {

pub(crate) open spec fn members(clock: &Clock, chains: Seq<u32>) -> ISet<int> {
    ISet::new(|a: int| 0 <= a < chains.len() && below(clock.lookup(chains[a]), a as usize))
}

pub(crate) open spec fn indices(pending: Seq<NodeIdx>) -> Seq<usize> {
    pending.map(|i: int, node: NodeIdx| node@ as usize)
}

pub(crate) proof fn delta_matches_members(
    tables: Seq<Seq<NodeIdx>>, chains: Seq<u32>, after: &Clock, before: &Clock, node: NodeIdx,
)
    requires table_layout(tables, chains),
    ensures difference::delta_member(tables, after, before, node)
        == (members(after, chains).contains(node@ as int) && !members(before, chains).contains(node@ as int)),
{
    if difference::delta_member(tables, after, before, node) {
        let c = choose|c: u32| c < tables.len() && tables[c as int].contains(node)
            && after.contents().contains_key(c) && difference::selected(node, after.contents()[c], before.lookup(c));
        assert(node@ < chains.len() && chains[node@ as int] == c);
    }
    if members(after, chains).contains(node@ as int) && !members(before, chains).contains(node@ as int) {
        let c = chains[node@ as int];
        assert(tables[c as int].contains(node));
        assert(after.contents().contains_key(c));
        assert(difference::selected(node, after.contents()[c], before.lookup(c)));
    }
}

pub(crate) proof fn indices_membership(pending: Seq<NodeIdx>, node: NodeIdx)
    ensures indices(pending).contains(node@ as usize) == pending.contains(node),
{
    if pending.contains(node) {
        let i = choose|i: int| 0 <= i < pending.len() && pending[i] == node;
        assert(indices(pending)[i] == node@);
    }
    if indices(pending).contains(node@ as usize) {
        let i = choose|i: int| 0 <= i < indices(pending).len() && indices(pending)[i] == node@;
        NodeIdx::same_index(pending[i], node);
        assert(pending.contains(node));
    }
}

pub(crate) proof fn assembly_refines_pending_contract(
    tables: Seq<Seq<NodeIdx>>, chains: Seq<u32>, pending: Seq<NodeIdx>,
    delivered: &Clock, processed: &Clock, before: &Clock, after: &Clock,
)
    requires table_layout(tables, chains), chains.len() <= u32::MAX,
        forall|a: int| 0 <= a < chains.len() ==> members(delivered, chains).contains(a),
        candidates::strictly_ordered(pending),
        forall|node: NodeIdx| pending.contains(node)
            == candidates::wanted(tables, delivered, processed, before, after, node),
    ensures materialization::pending_contract(indices(pending), chains.len(),
        members(processed, chains), members(before, chains), members(after, chains)),
{
    assert forall|node: NodeIdx| pending.contains(node) == (
        node@ < chains.len() && (!members(processed, chains).contains(node@ as int)
            || members(before, chains).contains(node@ as int) != members(after, chains).contains(node@ as int))) by {
        delta_matches_members(tables, chains, delivered, processed, node);
        delta_matches_members(tables, chains, before, after, node);
        delta_matches_members(tables, chains, after, before, node);
    }
    assert forall|i: int| 0 <= i < pending.len() implies indices(pending)[i] < chains.len() by {
        assert(pending.contains(pending[i]));
    }
    assert forall|a: int| indices(pending).contains(a as usize) && 0 <= a <= usize::MAX
        <==> 0 <= a < chains.len() && (!members(processed, chains).contains(a)
            || members(before, chains).contains(a) != members(after, chains).contains(a)) by {
        if 0 <= a <= u32::MAX {
            let node = NodeIdx::spec_new(a as u32);
            NodeIdx::spec_new_index(a as u32);
            indices_membership(pending, node);
        } else if indices(pending).contains(a as usize) && 0 <= a <= usize::MAX {
            let i = choose|i: int| 0 <= i < indices(pending).len() && indices(pending)[i] == a;
            assert(indices(pending)[i] <= u32::MAX);
        }
    }
}

// Execute the reference eager assembler and discharge the existing batch
// model's input contract. No library contract states this application result.
pub(crate) fn assemble_for_model(
    tables: &[Vec<NodeIdx>], delivered: &Clock, processed: &Clock, before: &Clock, after: &Clock,
    Ghost(chains): Ghost<Seq<u32>>,
) -> (pending: Vec<NodeIdx>)
    requires delivered.ordered(), processed.ordered(), before.ordered(), after.ordered(),
        forall|c: int| 0 <= c < tables.len() ==> crate::clock::ordered(tables[c]@),
        forall|c: u32| delivered.contents().contains_key(c) || before.contents().contains_key(c)
            || after.contents().contains_key(c) ==> c < tables.len(),
        table_layout(difference::tables_view(tables@), chains), chains.len() <= u32::MAX,
        forall|a: int| 0 <= a < chains.len() ==> members(delivered, chains).contains(a),
    ensures materialization::pending_contract(indices(pending@), chains.len(),
        members(processed, chains), members(before, chains), members(after, chains)),
{
    let pending = candidates::assemble(tables, delivered, processed, before, after);
    proof {
        assembly_refines_pending_contract(difference::tables_view(tables@), chains, pending@,
            delivered, processed, before, after);
    }
    pending
}

} // verus!
