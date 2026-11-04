//! Concrete witnesses for the actual shared tree's representation and lookup.
use vstd::prelude::*;
use std::sync::Arc;
use crate::tree::{Clock, ClockBuilder, Entry};
use crate::clock::NodeIdx;

verus! {

fn leaf(chain: u32, tip: NodeIdx) -> (out: Clock)
    ensures out.ordered(), out.contents() == Map::<u32, NodeIdx>::empty().insert(chain, tip),
        out.metadata(), out.balanced(), out.size() == 1, out.depth() == 1,
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

// All rotation paths have satisfiable premises, with distinct tips so key/value
// association matters as well as key membership. The general balance contract
// covers arbitrary subtrees; these witnesses check concrete nonempty use.
pub fn single_right_rotation() {
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    let c = NodeIdx::new(30);
    let child = Clock::node(1, b, leaf(0, a), Clock::default());
    let snapshot = child.clone();
    let ghost old_contents = child.contents();
    let tree = Clock::balance(2, c, child, Clock::default());
    assert(tree.ordered() && tree.metadata() && tree.balanced());
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(1, b).insert(2, c));
    let size = tree.len();
    let first = tree.position(0);
    let middle = tree.position(1);
    let last = tree.position(2);
    assert(size == 3 && first == Some(a) && middle == Some(b) && last == Some(c));
    assert(snapshot.contents() == old_contents);
}

pub fn double_left_right_rotation() {
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    let c = NodeIdx::new(30);
    let child = Clock::node(0, a, Clock::default(), leaf(1, b));
    let snapshot = child.clone();
    let ghost old_contents = child.contents();
    let tree = Clock::balance(2, c, child, Clock::default());
    assert(tree.ordered() && tree.metadata() && tree.balanced());
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(1, b).insert(2, c));
    let size = tree.len();
    let first = tree.position(0);
    let middle = tree.position(1);
    let last = tree.position(2);
    assert(size == 3 && first == Some(a) && middle == Some(b) && last == Some(c));
    assert(snapshot.contents() == old_contents);
}

pub fn single_left_rotation() {
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    let c = NodeIdx::new(30);
    let child = Clock::node(1, b, Clock::default(), leaf(2, c));
    let snapshot = child.clone();
    let ghost old_contents = child.contents();
    let tree = Clock::balance(0, a, Clock::default(), child);
    assert(tree.ordered() && tree.metadata() && tree.balanced());
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(1, b).insert(2, c));
    let size = tree.len();
    let first = tree.position(0);
    let middle = tree.position(1);
    let last = tree.position(2);
    assert(size == 3 && first == Some(a) && middle == Some(b) && last == Some(c));
    assert(snapshot.contents() == old_contents);
}

pub fn double_right_left_rotation() {
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    let c = NodeIdx::new(30);
    let child = Clock::node(2, c, leaf(1, b), Clock::default());
    let snapshot = child.clone();
    let ghost old_contents = child.contents();
    let tree = Clock::balance(0, a, Clock::default(), child);
    assert(tree.ordered() && tree.metadata() && tree.balanced());
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(1, b).insert(2, c));
    let size = tree.len();
    let first = tree.position(0);
    let middle = tree.position(1);
    let last = tree.position(2);
    assert(size == 3 && first == Some(a) && middle == Some(b) && last == Some(c));
    assert(snapshot.contents() == old_contents);
}

pub fn no_rotation_and_equal_height_grandchildren() {
    let tip = NodeIdx::new(0);
    let tree = Clock::balance(1, tip, leaf(0, tip), leaf(2, tip));
    assert(tree.balanced() && tree.metadata() && tree.size() == 3);
    // Equal grandchild heights select a single rotation, not a double rotation.
    let left_heavy = Clock::balance(3, tip, tree.clone(), Clock::default());
    let right_heavy = Clock::balance(0, tip, Clock::default(),
        Clock::node(2, tip, leaf(1, tip), leaf(3, tip)));
    assert(left_heavy.balanced() && right_heavy.balanced());
    assert(left_heavy.size() == 4 && right_heavy.size() == 4);
    assert(left_heavy.contents() =~= right_heavy.contents());
    assert(tree.balanced() && tree.size() == 3);
}

// These use the actual update entry points: no hand-built invariant is assumed.
pub fn updates_preserve_snapshots_and_take_maximum() {
    let mut tree = Clock::default();
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    tree.include(2, a);
    tree.include(0, a);
    let snapshot = tree.clone();
    tree.include(1, b); // left-right rotation
    tree.include(1, a); // lowering an existing tip is a no-op
    tree.include(2, b);
    assert(tree.valid() && tree.size() == 3);
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(1, b).insert(2, b));
    assert(snapshot.valid() && snapshot.size() == 2);
    assert(snapshot.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, a).insert(2, a));
    proof { tree.height_fits(); }
    assert(tree.depth() < 65);
}

pub fn replacement_and_extreme_keys() {
    let mut tree = Clock::default();
    let zero = NodeIdx::new(0);
    let last = NodeIdx::new(u32::MAX);
    tree.include(0, zero);
    tree.include(u32::MAX, last);
    tree.include(1, last); // right-left rotation
    let replacement = tree.set(u32::MAX, zero);
    assert(replacement.valid() && replacement.size() == 3);
    assert(replacement.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, zero).insert(1, last).insert(u32::MAX, zero));
    assert(tree.contents()[u32::MAX] == last);
}

pub fn iteration_is_ordered_complete_and_stays_exhausted() {
    let mut tree = Clock::default();
    let a = NodeIdx::new(10);
    let b = NodeIdx::new(20);
    let c = NodeIdx::new(30);
    tree.include(2, c);
    tree.include(0, a);
    tree.include(1, b);
    proof { tree.sequence_correct(); }
    let mut iter = tree.entries();
    let first = iter.next();
    let second = iter.next();
    let third = iter.next();
    let end = iter.next();
    let again = iter.next();
    assert(first == Some((0u32, a)) && second == Some((1u32, b)) && third == Some((2u32, c)));
    assert(end is None && again is None);
    let empty = Clock::default();
    proof { empty.sequence_correct(); }
    let mut iter = empty.entries();
    let end = iter.next();
    assert(end is None);
}

pub fn unions_take_maximum_preserve_inputs_and_handle_overlap() {
    let low = NodeIdx::new(10);
    let mid = NodeIdx::new(20);
    let high = NodeIdx::new(30);
    let mut a = Clock::default();
    a.include(0, low);
    a.include(2, high);
    let mut b = Clock::default();
    b.include(1, mid);
    b.include(2, mid);
    assert(a.contents().dom().union(b.contents().dom()) =~= set![0u32, 1u32, 2u32]);
    let out = a.union(&b);
    assert(out.valid() && out.size() == 3);
    assert(out.contents().contains_key(0u32));
    assert(out.contents().contains_key(1u32));
    assert(out.contents().contains_key(2u32));
    assert(a.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, low).insert(2, high));
    assert(b.contents() =~= Map::<u32, NodeIdx>::empty().insert(1, mid).insert(2, mid));
    assert(out.contents()[0u32] == low);
    assert(out.contents()[1u32] == mid);
    assert(out.contents()[2u32]@ >= high@);
    assert(out.contents()[2u32] == high);
    assert(out.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, low).insert(1, mid).insert(2, high));
    assert(b.contents().dom().union(a.contents().dom()) =~= set![0u32, 1u32, 2u32]);
    let swapped = b.union(&a);
    assert(swapped.contents() == out.contents());
    assert(a.contents().dom().union(a.contents().dom()) =~= set![0u32, 2u32]);
    let same = a.union(&a);
    assert(same.contents() == a.contents() && same.size() == a.size());
    let empty = Clock::default();
    assert(empty.contents().dom().union(out.contents().dom()) =~= set![0u32, 1u32, 2u32]);
    assert(out.contents().dom().union(empty.contents().dom()) =~= set![0u32, 1u32, 2u32]);
    let from_empty = empty.union(&out); // larger tree is the second argument
    let with_empty = out.union(&empty);
    assert(from_empty.contents() == out.contents() && with_empty.contents() == out.contents());
    assert(a.contents()[2] == high && b.contents()[2] == mid);
}

pub fn sorted_reconstruction_keeps_sparse_keys_and_tips() {
    let low = NodeIdx::new(0);
    let high = NodeIdx::new(u32::MAX);
    let entries = vec![(0u32, low), (5u32, high), (7u32, low), (u32::MAX, high)];
    let tree = Clock::from_sorted(&entries);
    assert(tree.valid() && tree.size() == 4 && tree.sequence() == entries@);
    proof { tree.sequence_correct(); }
    let first = tree.position(0);
    let last = tree.position(u32::MAX);
    let gap = tree.position(6);
    assert(first == Some(low) && last == Some(high) && gap is None);
    let empty: Vec<(u32, NodeIdx)> = Vec::new();
    let empty_tree = Clock::from_sorted(&empty);
    assert(empty_tree.valid() && empty_tree.size() == 0);
}

pub fn tip_tables_preserve_zero_and_chain_indices() {
    let zero = NodeIdx::new(0);
    let middle = NodeIdx::new(10);
    let last = NodeIdx::new(u32::MAX);
    let tips = vec![last, zero, middle];
    let tree = Clock::from_tips(&tips);
    assert(tree.valid() && tree.size() == 3);
    assert(tree.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, last).insert(1, zero).insert(2, middle));
    let empty: Vec<NodeIdx> = Vec::new();
    let tree = Clock::from_tips(&empty);
    assert(tree.valid() && tree.size() == 0 && tree.contents() =~= Map::<u32, NodeIdx>::empty());
}

pub fn masks_deny_cap_and_preserve_zero_without_expansion() {
    let zero = NodeIdx::new(0);
    let low = NodeIdx::new(10);
    let high = NodeIdx::new(30);
    let tips = vec![low, high, low];
    let tree = Clock::from_tips(&tips);
    let bound = |chain: u32, _tip: NodeIdx| -> (limit: Option<NodeIdx>)
        ensures limit == (if chain == 0 { Some(zero) } else if chain == 1 { None } else { Some(high) }),
    {
        if chain == 0 { Some(zero) } else if chain == 1 { None } else { Some(high) }
    };
    let out = tree.mask(bound);
    assert(out.valid() && out.size() <= tree.size());
    assert(out.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, zero).insert(2, low));
    let deny = |_chain: u32, _tip: NodeIdx| -> (limit: Option<NodeIdx>)
        ensures limit is None,
    { None };
    let denied = tree.mask(deny);
    assert(denied.contents() =~= Map::<u32, NodeIdx>::empty());
    let allow = |_chain: u32, tip: NodeIdx| -> (limit: Option<NodeIdx>)
        ensures limit == Some(tip),
    { Some(tip) };
    let unchanged = tree.mask(allow);
    assert(unchanged.contents() == tree.contents());
    assert(tree.contents()[0u32] == low && tree.contents()[1u32] == high);
}

pub fn builders_accumulate_maxima_without_losing_cached_inputs() {
    let low = NodeIdx::new(10);
    let high = NodeIdx::new(30);
    let cached = Clock::from_tips(&vec![high, low]);
    let mut builder = ClockBuilder::default();
    builder.include(0, low);
    assert(builder.contents().dom().union(cached.contents().dom()) =~= set![0u32, 1u32]);
    builder.merge(&cached);
    assert(builder.contents().contains_key(0u32));
    assert(cached.contents().contains_key(0u32));
    assert(cached.contents()[0u32] == high);
    assert(Clock::joins(builder.contents(), Map::<u32, NodeIdx>::empty().insert(0, low), cached.contents()));
    assert(builder.contents()[0u32]@ >= cached.contents()[0u32]@);
    assert(builder.contents()[0u32]@ >= high@);
    assert(builder.contents()[0u32] == high);
    builder.include(1, high);
    builder.include(2, low);
    let result = builder.finish();
    assert(result.contents().contains_key(0u32));
    assert(result.contents()[0u32]@ >= high@);
    assert(result.valid() && result.size() == 3);
    assert(result.contents() =~= Map::<u32, NodeIdx>::empty().insert(0, high).insert(1, high).insert(2, low));
    assert(cached.contents()[1u32] == low);
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
