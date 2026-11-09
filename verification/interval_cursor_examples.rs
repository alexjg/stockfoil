//! Consumers of the actual lazy interval cursor, without a ready/blocked scheduler model.
use vstd::prelude::*;
use crate::chain_intervals::{Intervals, selected, positions, weight};
use crate::clock::NodeIdx;
use std::ops::Range;

verus! {

/// Drain an arbitrary valid interval union through the production cursor methods.
/// This is a proof consumer; production traversal remains lazy.
pub(crate) fn drain(ranges: Vec<Range<usize>>) -> (out: Vec<usize>)
    requires forall|i: int| 0 <= i < ranges.len() ==> ranges[i].start < ranges[i].end,
    ensures out@.no_duplicates(),
        forall|p: usize| out@.contains(p) == selected(ranges@, p as int),
        forall|i: int, j: int| 0 <= i < j < out.len() ==> out[i] < out[j],
        out.len() == positions(ranges@).len(),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    let ghost original = ranges@;
    let mut cursor = Intervals::new(ranges);
    proof {
        cursor.facts();
        assert forall|p: int| positions(original).contains(p) == cursor.contains(p) by {
            crate::chain_intervals::positions_membership(original, p);
        }
        assert forall|p: int| positions(original).contains(p) == positions(cursor.view()).contains(p) by {
            crate::chain_intervals::positions_membership(original, p);
            assert(cursor.contains(p) == selected(original, p));
            crate::chain_intervals::positions_membership(cursor.view(), p);
            assert(positions(cursor.view()).contains(p) == selected(cursor.view(), p));
            assert(cursor.contains(p) == selected(cursor.view(), p));
        }
        assert(positions(original) =~= positions(cursor.view()));
    }
    let mut out: Vec<usize> = Vec::new();
    while !cursor.is_empty()
        invariant
            out@.no_duplicates(),
            forall|i: int, j: int| 0 <= i < j < out.len() ==> out[i] < out[j],
            forall|i: int, p: int| 0 <= i < out.len() && cursor.contains(p) ==> out[i] < p,
            forall|p: usize| selected(original, p as int) == (out@.contains(p) || cursor.contains(p as int)),
            out.len() + weight(cursor.view()) == positions(original).len(),
        decreases weight(cursor.view()),
    {
        let position = cursor.first();
        let ghost previous = out@;
        out.push(position);
        cursor = cursor.advance();
        proof {
            cursor.facts();
            assert(out@ == previous.push(position));
            assert forall|p: usize| selected(original, p as int) == (out@.contains(p) || cursor.contains(p as int)) by {
                if previous.contains(p) {
                    let j = choose|j: int| 0 <= j < previous.len() && previous[j] == p;
                    assert(out@[j] == p);
                }
                if position == p { assert(out@[previous.len() as int] == p); }
                if out@.contains(p) {
                    let j = choose|j: int| 0 <= j < out.len() && out[j] == p;
                    if j < previous.len() { assert(previous[j] == p); }
                    else { assert(position == p); }
                }
                assert(out@.contains(p) == (previous.contains(p) || position == p));
            }
        }
    }
    out
}

pub fn gaps_duplicates_and_last_dependency() {
    let ranges = vec![Range { start: 3usize, end: 4usize }, Range { start: 0usize, end: 1usize },
        Range { start: 0usize, end: 2usize }, Range { start: 1usize, end: 2usize }];
    let ghost input = ranges@;
    assert(input[0].start == 3 && input[0].end == 4);
    assert(input[1].start == 0 && input[1].end == 1);
    assert(input[2].start == 0 && input[2].end == 2);
    assert(input[3].start == 1 && input[3].end == 2);
    assert forall|p: int| selected(input, p) == (p == 0 || p == 1 || p == 3) by {
        if p == 0 { assert(input[1].start <= p < input[1].end); }
        if p == 1 { assert(input[3].start <= p < input[3].end); }
        if p == 3 { assert(input[0].start <= p < input[0].end); }
    }
    let mut cursor = Intervals::new(ranges);
    assert(cursor.contains(0) && cursor.contains(1) && !cursor.contains(2) && cursor.contains(3));
    proof {
        cursor.facts();
        assert forall|p: int| positions(cursor.view()).contains(p) == set![0int, 1int, 3int].contains(p) by {
            crate::chain_intervals::positions_membership(cursor.view(), p);
            assert(positions(cursor.view()).contains(p) == selected(cursor.view(), p));
            assert(cursor.contains(p) == selected(cursor.view(), p));
        }
        assert(positions(cursor.view()) =~= set![0int, 1int, 3int]);
    }
    let count = cursor.remaining();
    assert(count == 3);
    let zero = NodeIdx::new(0);
    let two = NodeIdx::new(2);
    let four = NodeIdx::new(4);
    let seven = NodeIdx::new(7);
    let nodes = vec![zero, two, four, seven];
    let gap_tip = NodeIdx::new(6);
    let dependency = cursor.last_node_before(&nodes, gap_tip);
    assert(dependency == two);
    let last = cursor.last_node_before(&nodes, seven);
    assert(last == seven);
    let first = cursor.first();
    assert(first == 0);
    cursor = cursor.advance();
    let next = cursor.first();
    assert(next == 1);
    cursor = cursor.advance();
    let after_gap = cursor.first();
    assert(after_gap == 3);
    cursor = cursor.advance();
    let empty = cursor.is_empty();
    assert(empty);
    let zero_count = cursor.remaining();
    assert(zero_count == 0);
}

/// A capture's position set cannot grow when a chain table is extended.
pub(crate) proof fn append_preserves_selected_nodes(cursor: &Intervals, before: Seq<NodeIdx>, after: Seq<NodeIdx>)
    requires cursor.bounded(before.len() as usize), before.len() <= usize::MAX,
        before.is_prefix_of(after),
    ensures forall|p: int| cursor.contains(p) ==> 0 <= p < after.len() && after[p] == before[p],
{
}

} // verus!
