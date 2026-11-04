//! Checked table updates establish the candidate model's complete-table layout.
//! History must still connect the ghost chain labels to its actual stored nodes.
use vstd::prelude::*;
use crate::clock::{NodeIdx, below};
use crate::chain_tables::{self, valid, extended};
use crate::difference::tables_view;
use crate::candidate_refinement::{table_layout, members};
use crate::tree::Clock;

verus! {

pub(crate) proof fn extension_preserves_layout(
    tables: Seq<Seq<NodeIdx>>, chains: Seq<u32>, chain: u32, position: NodeIdx,
)
    requires table_layout(tables, chains), chain <= tables.len(), position@ == chains.len(),
    ensures table_layout(extended(tables, chain, position), chains.push(chain)),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    let next = extended(tables, chain, position);
    let labels = chains.push(chain);
    assert forall|c: int, node: NodeIdx| 0 <= c < next.len() && next[c].contains(node) implies
        node@ < labels.len() && labels[node@ as int] == c by {
        if node == position {
            if c != chain { assert(tables[c].contains(node)); }
        } else {
            assert(c < tables.len() && tables[c].contains(node));
            assert(node@ < chains.len() && chains[node@ as int] == c);
        }
    }
    assert forall|node: NodeIdx| node@ < labels.len() implies labels[node@ as int] < next.len()
        && next[labels[node@ as int] as int].contains(node) by {
        if node@ == chains.len() {
            NodeIdx::same_index(node, position);
            assert(next[chain as int].contains(position));
        } else {
            let c = chains[node@ as int];
            assert(c < tables.len() && tables[c as int].contains(node));
            assert(next[c as int].contains(node));
        }
    }
}

pub(crate) fn append_for_layout(
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, chain: u32, position: NodeIdx,
    Ghost(chains): Ghost<Seq<u32>>,
)
    requires valid(old(tips)@, tables_view(old(tables)@)),
        table_layout(tables_view(old(tables)@), chains),
        chain <= old(tips).len(), position@ == chains.len(),
    ensures valid(final(tips)@, tables_view(final(tables)@)),
        final(tips).len() == old(tips).len() + (if chain < old(tips).len() { 0int } else { 1int }),
        table_layout(tables_view(final(tables)@), chains.push(chain)),
{
    let ghost initial = tables_view(tables@);
    proof {
        if chain < tips.len() {
            assert(chain_tables::row_valid(initial[chain as int], tips@[chain as int]));
            assert(initial[chain as int].contains(tips@[chain as int]));
            assert(tips@[chain as int]@ < chains.len());
        }
    }
    chain_tables::append(tips, tables, chain, position);
    proof { extension_preserves_layout(initial, chains, chain, position); }
}

// This discharges the delivered-clock premise of assemble_for_model whenever
// the actual history tables satisfy the layout/last-tip invariant.
pub(crate) fn delivered_from_tables(
    tips: &Vec<NodeIdx>, tables: &Vec<Vec<NodeIdx>>, Ghost(chains): Ghost<Seq<u32>>,
) -> (delivered: Clock)
    requires valid(tips@, tables_view(tables@)), table_layout(tables_view(tables@), chains),
        tips.len() <= u32::MAX, chains.len() <= u32::MAX,
    ensures delivered.valid(),
        forall|a: int| 0 <= a < chains.len() ==> members(&delivered, chains).contains(a),
        forall|c: u32| delivered.contents().contains_key(c) ==> c < tables.len(),
{
    let delivered = Clock::from_tips(tips);
    proof {
        assert forall|a: int| 0 <= a < chains.len() implies members(&delivered, chains).contains(a) by {
            let node = NodeIdx::spec_new(a as u32);
            NodeIdx::spec_new_index(a as u32);
            let c = chains[a];
            let row = tables_view(tables@)[c as int];
            assert(c < tables.len() && row.contains(node));
            assert(chain_tables::row_valid(row, tips@[c as int]));
            let i = choose|i: int| 0 <= i < row.len() && row[i] == node;
            if i < row.len() - 1 { assert(row[i]@ < row.last()@); }
            assert(node@ <= tips@[c as int]@);
            assert(delivered.lookup(c) == Some(tips@[c as int]));
            assert(below(delivered.lookup(c), a as usize));
        }
    }
    delivered
}

pub fn empty_and_interleaved_table_updates() {
    let mut tips = Vec::new();
    let mut tables = Vec::new();
    let zero = NodeIdx::new(0);
    let one = NodeIdx::new(1);
    let two = NodeIdx::new(2);
    let ghost empty = Seq::<u32>::empty();
    let ghost first = seq![0u32];
    let ghost second = seq![0u32, 1u32];
    let ghost third = seq![0u32, 1u32, 0u32];
    assert(valid(tips@, tables_view(tables@)));
    assert(table_layout(tables_view(tables@), empty));
    append_for_layout(&mut tips, &mut tables, 0, zero, Ghost(empty));
    append_for_layout(&mut tips, &mut tables, 1, one, Ghost(first));
    append_for_layout(&mut tips, &mut tables, 0, two, Ghost(second));
    let delivered = delivered_from_tables(&tips, &tables, Ghost(third));
    assert(members(&delivered, seq![0u32, 1u32, 0u32]).contains(0));
    assert(members(&delivered, seq![0u32, 1u32, 0u32]).contains(1));
    assert(members(&delivered, seq![0u32, 1u32, 0u32]).contains(2));
}

} // verus!
