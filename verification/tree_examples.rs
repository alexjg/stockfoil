//! Concrete witnesses for the actual shared tree's representation and lookup.
use vstd::prelude::*;
use std::sync::Arc;
use crate::tree::{Clock, Entry};
use crate::clock::NodeIdx;

verus! {

fn leaf(chain: u32, tip: NodeIdx) -> (out: Clock)
    ensures out.ordered(), out.contents() == Map::<u32, NodeIdx>::empty().insert(chain, tip),
{
    let left = Clock::default();
    let right = Clock::default();
    let out = Clock(Some(Arc::new(Entry { chain, tip, height: 1, len: 1, left, right })));
    proof { out.unfold(); }
    out
}

pub fn lookup_on_a_nonempty_shared_tree() {
    let empty = Clock::default();
    let missing = empty.position(0);
    assert(missing is None);
    let zero = NodeIdx::new(0);
    let middle = NodeIdx::new(7);
    let last = NodeIdx::new(u32::MAX);
    let left = leaf(0, zero);
    let right = leaf(u32::MAX, last);
    let tree = Clock(Some(Arc::new(Entry {
        chain: 5, tip: middle, height: 2, len: 3, left, right,
    })));
    proof { tree.unfold(); }
    assert(tree.ordered());
    let snapshot = tree.clone();
    assert(snapshot == tree);
    let first = tree.position(0);
    let center = tree.position(5);
    let end = snapshot.position(u32::MAX);
    let gap = tree.position(4);
    let right_gap = tree.position(6);
    assert(first == Some(zero));
    assert(center == Some(middle));
    assert(end == Some(last));
    assert(gap is None && right_gap is None);
    let included = snapshot.contains(0, zero);
    let excluded = tree.contains(0, NodeIdx::new(1));
    assert(included && !excluded);
}

// The ordering precondition is substantive: a left key above its parent is
// rejected even if the height and size metadata look reasonable.
pub fn malformed_tree_does_not_satisfy_the_invariant() {
    let tip = NodeIdx::new(0);
    let left = leaf(8, tip);
    let right = Clock::default();
    let malformed = Clock(Some(Arc::new(Entry {
        chain: 5, tip, height: 2, len: 2, left, right,
    })));
    proof { malformed.unfold(); }
    assert(!malformed.ordered()) by {
        assert(malformed.0.unwrap().left.contents().contains_key(8u32));
        if malformed.ordered() { assert(8u32 < 5u32); }
    }
}

} // verus!
