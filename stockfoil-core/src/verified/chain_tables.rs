//! Production updates to the ordered chain-node tables and their current tips.
use super::clock::NodeIdx;
#[cfg(verus_keep_ghost)]
use super::difference::tables_view;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn row_ordered(row: Seq<NodeIdx>) -> bool {
    forall|i: int, j: int| 0 <= i < j < row.len() ==> row[i]@ < row[j]@
}

pub(crate) open spec fn row_valid(row: Seq<NodeIdx>, tip: NodeIdx) -> bool {
    row.len() > 0 && row_ordered(row) && row.last() == tip
}

pub(crate) open spec fn valid(tips: Seq<NodeIdx>, tables: Seq<Seq<NodeIdx>>) -> bool {
    &&& tips.len() == tables.len()
    &&& forall|c: int| 0 <= c < tables.len() ==> #[trigger] row_valid(tables[c], tips[c])
}

pub(crate) open spec fn extended(tables: Seq<Seq<NodeIdx>>, chain: u32, position: NodeIdx) -> Seq<Seq<NodeIdx>> {
    if chain < tables.len() { tables.update(chain as int, tables[chain as int].push(position)) }
    else { tables.push(seq![position]) }
}

proof fn row_append(row: Seq<NodeIdx>, position: NodeIdx)
    requires row_ordered(row), row.len() > 0, row.last()@ < position@,
    ensures row_ordered(row.push(position)),
{
    assert forall|i: int, j: int| 0 <= i < j < row.push(position).len() implies
        row.push(position)[i]@ < row.push(position)[j]@ by {
        if j == row.len() {
            if i < row.len() - 1 { assert(row[i]@ < row.last()@); }
        }
    }
}

proof fn extension_valid(tips: Seq<NodeIdx>, tables: Seq<Seq<NodeIdx>>, chain: u32, position: NodeIdx)
    requires valid(tips, tables), chain <= tips.len(),
        chain < tips.len() ==> tips[chain as int]@ < position@,
    ensures valid(if chain < tips.len() { tips.update(chain as int, position) } else { tips.push(position) },
        extended(tables, chain, position)),
{
    let next = extended(tables, chain, position);
    let next_tips = if chain < tips.len() { tips.update(chain as int, position) } else { tips.push(position) };
    assert(next.len() == next_tips.len());
    assert forall|c: int| 0 <= c < next.len() implies row_valid(next[c], next_tips[c]) by {
        if c == chain {
            if chain < tips.len() {
                assert(row_valid(tables[c], tips[c]));
                row_append(tables[c], position);
                assert(next[c] == tables[c].push(position));
                assert(next[c].last() == position);
            } else {
                assert(next[c] =~= seq![position]);
                assert(row_ordered(next[c]));
            }
            assert(next_tips[c] == position);
        } else {
            assert(c < tables.len());
            assert(next[c] == tables[c]);
            assert(next_tips[c] == tips[c]);
            assert(row_valid(tables[c], tips[c]));
        }
    }
    assert(valid(next_tips, next));
}

/// Append to a selected existing chain, or allocate the next contiguous chain.
/// Selection of a same-author ancestral tip is a separate History obligation.
pub(crate) fn append(
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, chain: u32, position: NodeIdx,
)
    requires valid(old(tips)@, tables_view(old(tables)@)), chain <= old(tips).len(),
        chain < old(tips).len() ==> old(tips)@[chain as int]@ < position@,
    ensures valid(final(tips)@, tables_view(final(tables)@)),
        final(tips)@ == if chain < old(tips).len() { old(tips)@.update(chain as int, position) }
            else { old(tips)@.push(position) },
        tables_view(final(tables)@) == extended(tables_view(old(tables)@), chain, position),
{
    let ghost initial = tables_view(tables@);
    proof { extension_valid(tips@, initial, chain, position); }
    if (chain as usize) < tips.len() {
        tips[chain as usize] = position;
        tables[chain as usize].push(position);
        assert(tables_view(tables@) =~= extended(initial, chain, position));
    } else {
        assert(chain == initial.len());
        tips.push(position);
        let row = vec![position];
        assert(row@ =~= seq![position]);
        tables.push(row);
        assert(tables_view(tables@) =~= extended(initial, chain, position));
    }
}

} // verus!
