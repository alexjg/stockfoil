//! Nonempty witnesses for the production difference enumeration.
use vstd::prelude::*;
use crate::clock::NodeIdx;
use crate::tree::Clock;
use crate::difference::*;

verus! {

pub fn differences_preserve_prefix_and_enumerate_both_directions() {
    let zero = NodeIdx::new(0);
    let one = NodeIdx::new(1);
    let two = NodeIdx::new(2);
    let three = NodeIdx::new(3);
    let five = NodeIdx::new(5);
    let six = NodeIdx::new(6);
    let marker = NodeIdx::new(u32::MAX);
    let tables = vec![vec![zero, two, five], vec![one, three, six]];
    let after = Clock::from_tips(&vec![five, three]);
    let before = Clock::from_tips(&vec![zero, six]);
    proof {
        after.sequence_correct(); before.sequence_correct();
        assert(after.sequence() =~= seq![(0u32, five), (1u32, three)]);
        assert(before.sequence() =~= seq![(0u32, zero), (1u32, six)]);
        reveal_with_fuel(selection, 4);
        reveal_with_fuel(difference_prefix, 3);
    }
    let mut out = vec![marker];
    difference(&tables, &after, &before, &mut out);
    assert(out@ =~= seq![marker, two, five]);
    let mut symmetric = Vec::new();
    symmetric_difference(&tables, &after, &before, &mut symmetric);
    assert(symmetric@ =~= seq![two, five, six]);
    let empty = Clock::default();
    let mut all = Vec::new();
    difference(&tables, &after, &empty, &mut all);
    assert(all@ =~= seq![zero, two, five, one, three]);
    let mut none = vec![marker];
    difference(&tables, &after, &after, &mut none);
    assert(none@ =~= seq![marker]);
}

} // verus!
