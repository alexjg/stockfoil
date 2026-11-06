//! Concrete witnesses for the actual shared tree's representation and lookup.
use vstd::prelude::*;
use std::sync::Arc;
use crate::tree::{Clock, Entry};
use crate::clock::NodeIdx;

verus! {

fn leaf(chain: u32, tip: NodeIdx) -> (out: Clock)
    ensures out.ordered(), out.contents() == Map::<u32, NodeIdx>::empty().insert(chain, tip),
        out.metadata(), out.size() == 1, out.depth() == 1,
{
    let left = Clock::default();
    let right = Clock::default();
    Clock::node(chain, tip, left, right)
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
    let tree = Clock::node(5, middle, left, right);
    let height = tree.height();
    let len = tree.len();
    assert(height == 2 && len == 3);
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

// Rotations construct temporary unbalanced nodes. Their metadata must remain
// exact regardless of which child is taller; balancing is a separate contract.
pub fn unbalanced_construction_has_exact_metadata() {
    let tip = NodeIdx::new(0);
    let a = leaf(0, tip);
    let b = Clock::node(1, tip, a, Clock::default());
    let left_heavy = Clock::node(2, tip, b, Clock::default());
    let a = leaf(2, tip);
    let b = Clock::node(1, tip, Clock::default(), a);
    let right_heavy = Clock::node(0, tip, Clock::default(), b);
    let lh = left_heavy.height();
    let rh = right_heavy.height();
    let ll = left_heavy.len();
    let rl = right_heavy.len();
    assert(lh == 3 && rh == 3 && ll == 3 && rl == 3);
    let old = left_heavy.clone();
    assert(old.metadata() && old.size() == 3 && old.depth() == 3);
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
