//! Construct the lazy traversal selection from actual clock differences.
#[cfg(verus_keep_ghost)]
use super::{chain_intervals, pending};
use super::{
    chain_intervals::Intervals,
    clock::{self, NodeIdx},
    pending::PendingChain,
    schedule_queues::Queues,
    topology::Topology,
    traversal_step,
    tree::{ChainId, Clock},
};
use std::{collections::BTreeMap, ops::Range};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
use vstd::std_specs::iter::IteratorSpec;

verus! {

pub(crate) type Ranges = BTreeMap<ChainId, Vec<Range<usize>>>;

pub(crate) open spec fn valid(tables: Seq<Vec<NodeIdx>>, ranges: Map<ChainId, Vec<Range<usize>>>) -> bool {
    forall|c: ChainId| ranges.contains_key(c) ==> c < tables.len() && ranges[c].len() > 0
        && forall|i: int| #![trigger ranges[c][i]] 0 <= i < ranges[c].len() ==> 0 <= ranges[c][i].start < ranges[c][i].end <= tables[c as int].len()
}

pub(crate) open spec fn selected(ranges: Map<ChainId, Vec<Range<usize>>>, c: ChainId, p: int) -> bool {
    ranges.contains_key(c) && super::chain_intervals::selected(ranges[c]@, p)
}

pub(crate) open spec fn inputs(tables: Seq<Vec<NodeIdx>>, differences: Seq<(&Clock, &Clock)>) -> bool {
    &&& forall|c: int| 0 <= c < tables.len() ==> clock::ordered(tables[c]@)
    &&& forall|i: int| 0 <= i < differences.len() ==> differences[i].0.ordered() && differences[i].1.ordered()
        && forall|c: ChainId| differences[i].0.contents().contains_key(c) ==> c < tables.len()
}

pub(crate) open spec fn delta(tables: Seq<Vec<NodeIdx>>, after: &Clock, before: &Clock, c: ChainId, p: int) -> bool {
    c < tables.len() && 0 <= p < tables[c as int].len() && after.contents().contains_key(c)
        && super::difference::selected(tables[c as int][p], after.contents()[c], before.lookup(c))
}

pub(crate) open spec fn union(tables: Seq<Vec<NodeIdx>>, differences: Seq<(&Clock, &Clock)>, c: ChainId, p: int) -> bool {
    exists|i: int| 0 <= i < differences.len() && delta(tables, differences[i].0, differences[i].1, c, p)
}

spec fn prefix(tables: Seq<Vec<NodeIdx>>, entries: Seq<(ChainId, NodeIdx)>, before: &Clock, n: nat, c: ChainId, p: int) -> bool {
    c < tables.len() && 0 <= p < tables[c as int].len() && exists|i: int| 0 <= i < n && entries[i].0 == c
        && super::difference::selected(tables[c as int][p], entries[i].1, before.lookup(c))
}

fn append(ranges: &mut Ranges, chain: ChainId, start: usize, end: usize)
    requires start < end,
    ensures final(ranges)@.dom() == old(ranges)@.dom().insert(chain),
        final(ranges)@[chain]@ == (if old(ranges)@.contains_key(chain) { old(ranges)@[chain]@ } else { Seq::empty() }).push(Range { start, end }),
        forall|c: ChainId| c != chain && old(ranges)@.contains_key(c) ==> final(ranges)@[c] == old(ranges)@[c],
{
    match ranges.get_mut(&chain) {
        Some(row) => row.push(Range { start, end }),
        None => {
            let mut row = Vec::new();
            row.push(Range { start, end });
            ranges.insert(chain, row);
        },
    }
}

fn collect_one(tables: &[Vec<NodeIdx>], after: &Clock, before: &Clock, ranges: &mut Ranges)
    requires valid(tables@, old(ranges)@), after.ordered(), before.ordered(),
        forall|c: ChainId| after.contents().contains_key(c) ==> c < tables.len(),
        forall|c: int| 0 <= c < tables.len() ==> clock::ordered(tables[c]@),
    ensures valid(tables@, final(ranges)@),
        forall|c: ChainId, p: int| selected(final(ranges)@, c, p) == (selected(old(ranges)@, c, p) || delta(tables@, after, before, c, p)),
{
    proof { after.sequence_correct(); }
    let ghost initial = ranges@;
    let ghost mut done: nat = 0;
    let mut iter = after.entries();
    loop
        invariant valid(tables@, ranges@), after.ordered(), before.ordered(), done <= after.sequence().len(),
            iter.remaining() == after.sequence().skip(done as int),
            forall|c: ChainId| after.contents().contains_key(c) ==> c < tables.len(),
            forall|c: int| 0 <= c < tables.len() ==> clock::ordered(tables[c]@),
            forall|c: ChainId, p: int| #![trigger selected(ranges@, c, p)] selected(ranges@, c, p) == (selected(initial, c, p) || prefix(tables@, after.sequence(), before, done, c, p)),
        ensures valid(tables@, ranges@),
            forall|c: ChainId, p: int| #![trigger selected(ranges@, c, p)] selected(ranges@, c, p) == (selected(initial, c, p) || delta(tables@, after, before, c, p)),
        decreases iter.remaining().len(),
    {
        match iter.next() {
            None => {
                proof {
                    after.sequence_correct();
                    assert forall|c: ChainId, p: int| #![trigger prefix(tables@, after.sequence(), before, done, c, p)] prefix(tables@, after.sequence(), before, done, c, p) == delta(tables@, after, before, c, p) by {
                        if after.contents().contains_key(c) {
                            let i = choose|i: int| 0 <= i < after.sequence().len() && after.sequence()[i].0 == c;
                            assert(after.sequence()[i].1 == after.contents()[c]);
                        }
                        if prefix(tables@, after.sequence(), before, done, c, p) {
                            let i = choose|i: int| 0 <= i < done && after.sequence()[i].0 == c
                                && super::difference::selected(tables[c as int][p], after.sequence()[i].1, before.lookup(c));
                            assert(after.contents().contains_key(c));
                        }
                    }
                    assert forall|c: ChainId, p: int| #![trigger selected(ranges@, c, p)] selected(ranges@, c, p) ==
                        (selected(initial, c, p) || delta(tables@, after, before, c, p)) by {
                        assert(prefix(tables@, after.sequence(), before, done, c, p) == delta(tables@, after, before, c, p));
                        assert(selected(ranges@, c, p) == (selected(initial, c, p) || prefix(tables@, after.sequence(), before, done, c, p)));
                    }
                }
                break;
            },
            Some((chain, tip)) => {
                proof { after.sequence_correct(); assert(after.sequence()[done as int] == (chain, tip)); }
                let (start, end) = clock::difference_range(&tables[chain as usize], tip, before.position(chain));
                let ghost prior = ranges@;
                if start < end { append(ranges, chain, start, end); }
                proof {
                    assert forall|c: ChainId, p: int| #![trigger selected(ranges@, c, p)] selected(ranges@, c, p) == (selected(prior, c, p) || c == chain && start <= p < end) by {
                        if c == chain && start < end {
                            let old_row = if prior.contains_key(chain) { prior[chain]@ } else { Seq::empty() };
                            super::chain_intervals::selected_push(old_row, Range { start, end }, p);
                        }
                    }
                    assert forall|c: ChainId, p: int| #![trigger prefix(tables@, after.sequence(), before, done + 1, c, p)] prefix(tables@, after.sequence(), before, done + 1, c, p) ==
                        (prefix(tables@, after.sequence(), before, done, c, p) || c == chain && start <= p < end) by {
                        if prefix(tables@, after.sequence(), before, done + 1, c, p) {
                            let i = choose|i: int| 0 <= i < done + 1 && after.sequence()[i].0 == c
                                && super::difference::selected(tables[c as int][p], after.sequence()[i].1, before.lookup(c));
                            if i < done { assert(prefix(tables@, after.sequence(), before, done, c, p)); }
                        }
                        if prefix(tables@, after.sequence(), before, done, c, p) {
                            let i = choose|i: int| 0 <= i < done && after.sequence()[i].0 == c
                                && super::difference::selected(tables[c as int][p], after.sequence()[i].1, before.lookup(c));
                            assert(i < done + 1);
                        }
                        if c == chain && start <= p < end {
                            assert(super::difference::selected(tables[c as int][p], after.sequence()[done as int].1, before.lookup(c)));
                        }
                    }
                    assert forall|c: ChainId, p: int| #![trigger selected(ranges@, c, p)] selected(ranges@, c, p) ==
                        (selected(initial, c, p) || prefix(tables@, after.sequence(), before, done + 1, c, p)) by {
                        assert(selected(prior, c, p) == (selected(initial, c, p) || prefix(tables@, after.sequence(), before, done, c, p)));
                    }
                    done = done + 1;
                }
            },
        }
    }
}

pub(crate) fn collect(tables: &[Vec<NodeIdx>], differences: &[(&Clock, &Clock)]) -> (out: Ranges)
    requires inputs(tables@, differences@),
    ensures valid(tables@, out@), forall|c: ChainId, p: int| selected(out@, c, p) == union(tables@, differences@, c, p),
{
    let mut ranges = BTreeMap::new();
    let mut i = 0;
    while i < differences.len()
        invariant i <= differences.len(), inputs(tables@, differences@), valid(tables@, ranges@),
            forall|c: ChainId, p: int| selected(ranges@, c, p) == union(tables@, differences@.take(i as int), c, p),
        decreases differences.len() - i,
    {
        let ghost prior = ranges@;
        collect_one(tables, differences[i].0, differences[i].1, &mut ranges);
        proof {
            assert forall|c: ChainId, p: int| union(tables@, differences@.take(i + 1), c, p) ==
                (union(tables@, differences@.take(i as int), c, p) || delta(tables@, differences[i as int].0, differences[i as int].1, c, p)) by {
                if union(tables@, differences@.take(i + 1), c, p) {
                    let j = choose|j: int| 0 <= j < i + 1 && delta(tables@, differences[j].0, differences[j].1, c, p);
                    if j < i {
                        assert(differences@.take(i as int)[j] == differences[j]);
                        assert(delta(tables@, differences@.take(i as int)[j].0, differences@.take(i as int)[j].1, c, p));
                        assert(union(tables@, differences@.take(i as int), c, p));
                    }
                }
                if union(tables@, differences@.take(i as int), c, p) {
                    let j = choose|j: int| 0 <= j < i && delta(tables@, differences[j].0, differences[j].1, c, p);
                    assert(differences@.take(i + 1)[j] == differences[j]);
                    assert(delta(tables@, differences@.take(i + 1)[j].0, differences@.take(i + 1)[j].1, c, p));
                }
                if delta(tables@, differences[i as int].0, differences[i as int].1, c, p) {
                    assert(delta(tables@, differences@.take(i + 1)[i as int].0, differences@.take(i + 1)[i as int].1, c, p));
                }
            }
            assert forall|c: ChainId, p: int| selected(ranges@, c, p) == union(tables@, differences@.take(i + 1), c, p) by {
                assert(selected(prior, c, p) == union(tables@, differences@.take(i as int), c, p));
                assert(selected(ranges@, c, p) == (selected(prior, c, p) || delta(tables@, differences[i as int].0, differences[i as int].1, c, p)));
                assert(union(tables@, differences@.take(i + 1), c, p) == (union(tables@, differences@.take(i as int), c, p) || delta(tables@, differences[i as int].0, differences[i as int].1, c, p)));
            }
        }
        i += 1;
    }
    assert(differences@.take(i as int) == differences@);
    ranges
}


/// Range positions map injectively to stored nodes, so interval weight is record count.
proof fn chain_cardinality<P>(t: &Topology<P>, chain: &PendingChain)
    requires t.valid(), chain.valid(t),
    ensures chain_intervals::positions(chain.intervals.view()).map(|p: int| t.tables_view()[chain.chain as int][p]).len()
            == chain_intervals::weight(chain.intervals.view()),
        forall|n: NodeIdx| chain_intervals::positions(chain.intervals.view()).map(|p: int| t.tables_view()[chain.chain as int][p]).contains(n)
            == chain.selected(t, n),
{
    t.traversal_facts();
    chain_intervals::cardinality(chain.intervals.view());
    assert forall|p: int| chain_intervals::positions(chain.intervals.view()).contains(p) == chain.intervals.contains(p) by {
        chain_intervals::positions_membership(chain.intervals.view(), p);
    }
    let positions = chain_intervals::positions(chain.intervals.view());
    let row = t.tables_view()[chain.chain as int]@;
    let f = |p: int| row[p];
    assert forall|p: int, q: int| positions.contains(p) && positions.contains(q) && p != q implies f(p) != f(q) by {
        chain_intervals::positions_membership(chain.intervals.view(), p);
        chain_intervals::positions_membership(chain.intervals.view(), q);
        assert(chain.intervals.contains(p));
        assert(chain.intervals.contains(q));
        assert(chain.intervals.bounded(t.tables_view()[chain.chain as int].len()));
        assert(0 <= p < t.tables_view()[chain.chain as int].len());
        assert(0 <= q < t.tables_view()[chain.chain as int].len());
        assert(t.tables_view()[chain.chain as int].len() == row.len());
        assert(0 <= p < row.len() && 0 <= q < row.len());
        if p < q { assert(row[p]@ < row[q]@); }
        else { assert(row[q]@ < row[p]@); }
    }
    assert(positions.injective_on(f));
    vstd::set_lib::lemma_map_size(positions, positions.map(f), f);
    assert forall|n: NodeIdx| positions.map(f).contains(n) == chain.selected(t, n) by {
        if positions.map(f).contains(n) {
            let p = choose|p: int| positions.contains(p) && f(p) == n;
            assert(chain.intervals.contains(p));
        }
        if chain.selected(t, n) {
            let p = choose|p: int| chain.intervals.contains(p) && row[p] == n;
            assert(positions.contains(p));
            assert(positions.map(f).contains(f(p)));
        }
    }
}

/// The sum cannot overflow: every selected record occupies a distinct stored index.
proof fn selection_bound<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>)
    requires t.valid(), pending::valid(t, p),
    ensures pending::selected_set(t, p).len() <= t.nodes_view().len() <= usize::MAX,
{
    t.traversal_facts();
    pending::selection_facts(t, p);
    t.node_count_bound();
    let selected = pending::selected_set(t, p);
    let f = |n: NodeIdx| n@ as int;
    assert forall|a: NodeIdx, b: NodeIdx| selected.contains(a) && selected.contains(b) && a != b implies f(a) != f(b) by {
        if f(a) == f(b) { NodeIdx::same_index(a, b); }
    }
    assert(selected.injective_on(f));
    vstd::set_lib::lemma_map_size(selected, selected.map(f), f);
    let indices = Set::range(0, t.nodes_view().len() as int);
    vstd::set_lib::lemma_int_range(0, t.nodes_view().len() as int);
    assert forall|i: int| selected.map(f).contains(i) implies indices.contains(i) by {
        let n = choose|n: NodeIdx| selected.contains(n) && f(n) == i;
        let frontier = choose|frontier: NodeIdx| p.contains_key(frontier) && p[frontier].selected(t, n);
        p[frontier].facts(t);
    }
    assert(selected.map(f).subset_of(indices));
    vstd::set_lib::lemma_len_subset(selected.map(f), indices);
}

proof fn selection_insert<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, chain: PendingChain)
    requires !p.contains_key(chain.frontier(t)),
    ensures forall|n: NodeIdx| pending::selected(t, p.insert(chain.frontier(t), chain), n) == (pending::selected(t, p, n) || chain.selected(t, n)),
{
    let next = p.insert(chain.frontier(t), chain);
    assert forall|n: NodeIdx| pending::selected(t, next, n) == (pending::selected(t, p, n) || chain.selected(t, n)) by {
        if pending::selected(t, next, n) {
            let f = choose|f: NodeIdx| next.contains_key(f) && next[f].selected(t, n);
            if f != chain.frontier(t) { assert(p.contains_key(f)); assert(next[f] == p[f]); assert(pending::selected(t, p, n)); }
            else { assert(next[f] == chain); assert(chain.selected(t, n)); }
        }
        if pending::selected(t, p, n) {
            let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(t, n);
            assert(f != chain.frontier(t));
            assert(next.contains_key(f)); assert(next[f] == p[f]); assert(next[f].selected(t, n));
            assert(pending::selected(t, next, n));
        }
        if chain.selected(t, n) {
            assert(next.contains_key(chain.frontier(t)));
            assert(next[chain.frontier(t)].selected(t, n));
            assert(pending::selected(t, next, n));
        }
    }

}

proof fn fresh_chain<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, chain: PendingChain)
    requires t.valid(), pending::valid(t, p), chain.valid(t),
        forall|f: NodeIdx| p.contains_key(f) ==> p[f].chain != chain.chain,
    ensures pending::valid(t, p.insert(chain.frontier(t), chain)), !p.contains_key(chain.frontier(t)),
{
    chain.facts(t);
    if p.contains_key(chain.frontier(t)) { p[chain.frontier(t)].facts(t); }
    assert(pending::valid(t, p.insert(chain.frontier(t), chain)));
}

proof fn chain_disjoint<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, chain: PendingChain)
    requires t.valid(), pending::valid(t, p), chain.valid(t),
        forall|f: NodeIdx| p.contains_key(f) ==> p[f].chain != chain.chain,
    ensures forall|n: NodeIdx| pending::selected(t, p, n) ==> !chain.selected(t, n),
{
    chain.facts(t);
    assert forall|n: NodeIdx| pending::selected(t, p, n) implies !chain.selected(t, n) by {
        let f = choose|f: NodeIdx| p.contains_key(f) && p[f].selected(t, n);
        p[f].facts(t);
    }
}

proof fn selection_membership<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>)
    requires t.valid(), pending::valid(t, p),
    ensures forall|n: NodeIdx| pending::selected_set(t, p).contains(n) == pending::selected(t, p, n),
{ pending::selection_facts(t, p); }

proof fn inserted_chain<P>(t: &Topology<P>, p: Map<NodeIdx, PendingChain>, chain: PendingChain)
    requires t.valid(), pending::valid(t, p), chain.valid(t),
        forall|f: NodeIdx| p.contains_key(f) ==> p[f].chain != chain.chain,
    ensures pending::valid(t, p.insert(chain.frontier(t), chain)), !p.contains_key(chain.frontier(t)),
        forall|n: NodeIdx| pending::selected(t, p.insert(chain.frontier(t), chain), n) == (pending::selected(t, p, n) || chain.selected(t, n)),
        pending::selected_set(t, p.insert(chain.frontier(t), chain)).len()
            == pending::selected_set(t, p).len() + chain_intervals::weight(chain.intervals.view()),
{
    fresh_chain(t, p, chain);
    let next = p.insert(chain.frontier(t), chain);
    selection_insert(t, p, chain);
    let a = pending::selected_set(t, p);
    let b = chain_intervals::positions(chain.intervals.view()).map(|i: int| t.tables_view()[chain.chain as int][i]);
    assert(pending::selected_set(t, next) == a.union(b) && a.disjoint(b)) by {
        chain_cardinality(t, &chain);
        selection_membership(t, p);
        selection_membership(t, next);
        chain_disjoint(t, p, chain);
        assert(a.disjoint(b));
        assert forall|n: NodeIdx| pending::selected_set(t, next).contains(n) == a.union(b).contains(n) by {
            assert(pending::selected_set(t, next).contains(n) == pending::selected(t, next, n));
            assert(a.contains(n) == pending::selected(t, p, n));
            assert(b.contains(n) == chain.selected(t, n));
            assert(pending::selected(t, next, n) == (pending::selected(t, p, n) || chain.selected(t, n)));
        }
        assert(pending::selected_set(t, next) =~= a.union(b));
    }
    chain_cardinality(t, &chain);
    vstd::set_lib::lemma_set_disjoint_lens(a, b);

}

fn insert_chain<P>(t: &Topology<P>, p: &mut BTreeMap<NodeIdx, PendingChain>, remaining: &mut usize, chain: ChainId, ranges: Vec<Range<usize>>)
    requires pending::valid(t, old(p)@), *old(remaining) == pending::selected_set(t, old(p)@).len(),
        chain < t.tables_view().len(), ranges.len() > 0,
        forall|i: int| #![trigger ranges[i]] 0 <= i < ranges.len() ==> ranges[i].start < ranges[i].end <= t.tables_view()[chain as int].len(),
        forall|f: NodeIdx| old(p)@.contains_key(f) ==> old(p)@[f].chain != chain,
    ensures pending::valid(t, final(p)@), *final(remaining) == pending::selected_set(t, final(p)@).len(),
        forall|n: NodeIdx| pending::selected(t, final(p)@, n) == (pending::selected(t, old(p)@, n)
            || exists|i: int| chain_intervals::selected(ranges@, i) && t.tables_view()[chain as int][i] == n),
        forall|f: NodeIdx| final(p)@.contains_key(f) ==> final(p)@[f].chain == chain || old(p)@.contains_key(f) && final(p)@[f] == old(p)@[f],
{
    proof { t.established(); super::stdlib::node_order_model(); }
    let ghost original = ranges@;
    let merged = Intervals::new(ranges);
    proof {
        merged.facts();
        assert forall|i: int| merged.contains(i) implies 0 <= i < t.tables_view()[chain as int].len() by {
            let r = choose|r: int| 0 <= r < original.len() && #[trigger] original[r].start <= i < original[r].end;
        }
        assert(chain_intervals::selected(original, original[0].start as int));
        assert(merged.contains(original[0].start as int));
        assert(merged.view().len() > 0);
    }
    let count = merged.remaining();
    let chain = PendingChain { chain, intervals: merged };
    proof { inserted_chain(t, p@, chain); }
    proof {
        assert forall|n: NodeIdx| chain.selected(t, n) == (exists|i: int| chain_intervals::selected(original, i) && t.tables_view()[chain.chain as int][i] == n) by {
            if chain.selected(t, n) {
                let i = choose|i: int| chain.intervals.contains(i) && t.tables_view()[chain.chain as int][i] == n;
                assert(chain_intervals::selected(original, i));
            }
            if exists|i: int| chain_intervals::selected(original, i) && t.tables_view()[chain.chain as int][i] == n {
                let i = choose|i: int| chain_intervals::selected(original, i) && t.tables_view()[chain.chain as int][i] == n;
                assert(chain.intervals.contains(i));
            }
        }
    }
    let first = chain.first(t);
    p.insert(first, chain);
    proof { selection_bound(t, p@); }
    *remaining += count;
}

fn chains(ranges: &Ranges) -> (out: Vec<ChainId>)
    ensures out@.to_set() == ranges@.dom(), out@.no_duplicates(),
{
    let keys = ranges.keys();
    let ghost input = keys.remaining();
    let mapped = keys.map(|key: &ChainId| -> (out: ChainId) ensures out == *key { *key });
    let out: Vec<ChainId> = mapped.collect();
    proof {
        broadcast use vstd::std_specs::iter::map_postcondition;
        assert(out.len() == input.len());
        assert forall|i: int| 0 <= i < out.len() implies out[i] == *input[i] by {};
        assert(out@ =~= input.unref());
        assert forall|i: int, j: int| 0 <= i < j < out.len() implies out[i] != out[j] by { assert(input[i] != input[j]); }
    }
    out
}

pub(crate) open spec fn raw_node<P>(t: &Topology<P>, ranges: Map<ChainId, Vec<Range<usize>>>, n: NodeIdx) -> bool {
    exists|c: ChainId, i: int| selected(ranges, c, i) && t.tables_view()[c as int][i] == n
}

proof fn keys_step(keys: Seq<ChainId>, i: int)
    requires 0 <= i < keys.len(),
    ensures keys.take(i + 1).to_set() == keys.take(i).to_set().insert(keys[i]),
        keys.take(i + 1).contains(keys[i]),
        forall|c: ChainId| keys.take(i + 1).contains(c) == (keys.take(i).contains(c) || c == keys[i]),
{
    assert(keys.take(i + 1) =~= keys.take(i).push(keys[i]));
    assert forall|c: ChainId| keys.take(i + 1).contains(c) == (keys.take(i).contains(c) || c == keys[i]) by {
        vstd::seq_lib::lemma_seq_contains_after_push(keys.take(i), keys[i], c);
    }
    vstd::seq_lib::lemma_seq_contains_after_push(keys.take(i), keys[i], keys[i]);
    assert(keys.take(i + 1).contains(keys[i]));
    assert forall|c: ChainId| keys.take(i + 1).to_set().contains(c) == keys.take(i).to_set().insert(keys[i]).contains(c) by {
        vstd::seq_lib::lemma_seq_contains_after_push(keys.take(i), keys[i], c);
    }
    assert(keys.take(i + 1).to_set() =~= keys.take(i).to_set().insert(keys[i]));
}

fn prepare<P>(t: &Topology<P>, ranges: Ranges) -> (out: (BTreeMap<NodeIdx, PendingChain>, usize))
    requires valid(t.tables_view(), ranges@),
    ensures pending::valid(t, out.0@), out.1 == pending::selected_set(t, out.0@).len(),
        forall|n: NodeIdx| pending::selected(t, out.0@, n) == raw_node(t, ranges@, n),
{
    proof { t.established(); }
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    let ghost original = ranges@;
    let mut ranges = ranges;
    let keys = chains(&ranges);
    let mut pending = BTreeMap::new();
    let mut remaining = 0usize;
    let mut i = 0;
    assert(pending::selected_set(t, pending@) =~= Set::<NodeIdx>::empty());
    while i < keys.len()
        invariant t.valid(), valid(t.tables_view(), original), valid(t.tables_view(), ranges@),
            keys@.to_set() == original.dom(), keys@.no_duplicates(), i <= keys.len(),
            ranges@ == original.remove_keys(keys@.take(i as int).to_set()),
            pending::valid(t, pending@), remaining == pending::selected_set(t, pending@).len(),
            forall|f: NodeIdx| pending@.contains_key(f) ==> keys@.take(i as int).contains(pending@[f].chain),
            forall|n: NodeIdx| pending::selected(t, pending@, n) == (exists|c: ChainId, p: int|
                keys@.take(i as int).contains(c) && selected(original, c, p) && t.tables_view()[c as int][p] == n),
        decreases keys.len() - i,
    {
        let c = keys[i];
        proof {
            assert(!keys@.take(i as int).contains(c));
            assert(original.dom().contains(c));
        }
        let row = ranges.remove(&c).unwrap();
        let ghost prior = pending@;
        insert_chain(t, &mut pending, &mut remaining, c, row);
        proof {
            keys_step(keys@, i as int);
            assert(keys@[i as int] == c);
            assert(keys@.take(i + 1).to_set() =~= keys@.take(i as int).to_set().insert(c));
            assert(ranges@ =~= original.remove_keys(keys@.take(i + 1).to_set()));
            assert forall|n: NodeIdx| pending::selected(t, pending@, n) == (exists|ch: ChainId, p: int|
                keys@.take(i + 1).contains(ch) && selected(original, ch, p) && t.tables_view()[ch as int][p] == n) by {
                if pending::selected(t, prior, n) {
                    let (ch, p) = choose|ch: ChainId, p: int| keys@.take(i as int).contains(ch)
                        && selected(original, ch, p) && t.tables_view()[ch as int][p] == n;
                    assert(keys@.take(i + 1).contains(ch));
                }
                if exists|p: int| chain_intervals::selected(row@, p) && t.tables_view()[c as int][p] == n {
                    let p = choose|p: int| chain_intervals::selected(row@, p) && t.tables_view()[c as int][p] == n;
                    assert(selected(original, c, p));
                    assert(keys@.take(i + 1).contains(c));
                }
                if exists|ch: ChainId, p: int| keys@.take(i + 1).contains(ch) && selected(original, ch, p) && t.tables_view()[ch as int][p] == n {
                    let (ch, p) = choose|ch: ChainId, p: int| keys@.take(i + 1).contains(ch)
                        && selected(original, ch, p) && t.tables_view()[ch as int][p] == n;
                    if ch == c { assert(chain_intervals::selected(row@, p)); }
                    else { assert(keys@.take(i as int).contains(ch)); assert(pending::selected(t, prior, n)); }
                }
            }
        }
        i += 1;
    }
    assert(keys@.take(i as int) == keys@);
    (pending, remaining)
}

pub(crate) open spec fn requested<P>(t: &Topology<P>, differences: Seq<(&Clock, &Clock)>, n: NodeIdx) -> bool {
    exists|c: ChainId, p: int| union(t.tables_view(), differences, c, p) && t.tables_view()[c as int][p] == n
}

/// Relate interval positions to the existing record-level clock-difference contract.
pub(crate) proof fn requested_difference<P>(t: &Topology<P>, differences: Seq<(&Clock, &Clock)>, n: NodeIdx)
    ensures requested(t, differences, n) == (exists|i: int| 0 <= i < differences.len()
        && super::difference::delta_member(super::difference::tables_view(t.tables_view()), differences[i].0, differences[i].1, n)),
{
    let tables = super::difference::tables_view(t.tables_view());
    if requested(t, differences, n) {
        let (c, p) = choose|c: ChainId, p: int| union(t.tables_view(), differences, c, p) && t.tables_view()[c as int][p] == n;
        let i = choose|i: int| 0 <= i < differences.len() && delta(t.tables_view(), differences[i].0, differences[i].1, c, p);
        assert(tables[c as int].contains(n));
        assert(super::difference::delta_member(tables, differences[i].0, differences[i].1, n));
    }
    if exists|i: int| 0 <= i < differences.len() && super::difference::delta_member(tables, differences[i].0, differences[i].1, n) {
        let i = choose|i: int| 0 <= i < differences.len() && super::difference::delta_member(tables, differences[i].0, differences[i].1, n);
        let after = differences[i].0;
        let before = differences[i].1;
        let c = choose|c: ChainId| c < tables.len() && tables[c as int].contains(n)
            && after.contents().contains_key(c) && super::difference::selected(n, after.contents()[c], before.lookup(c));
        let p = choose|p: int| 0 <= p < tables[c as int].len() && tables[c as int][p] == n;
        assert(delta(t.tables_view(), after, before, c, p));
        assert(union(t.tables_view(), differences, c, p));
        assert(requested(t, differences, n));
    }
}

/// Full production construction, with an exact union-of-differences selection.
pub(crate) fn new<P>(t: &Topology<P>, differences: &[(&Clock, &Clock)]) -> (out: (BTreeMap<NodeIdx, PendingChain>, Queues, usize))
    requires inputs(t.tables_view(), differences@),
    ensures traversal_step::valid(t, out.0@, &out.1, out.2),
        forall|n: NodeIdx| pending::selected(t, out.0@, n) == requested(t, differences@, n),
{
    let ranges = collect(t.chain_nodes(), differences);
    let ghost raw = ranges@;
    let (pending, remaining) = prepare(t, ranges);
    proof {
        assert forall|n: NodeIdx| raw_node(t, raw, n) == requested(t, differences@, n) by {
            if raw_node(t, raw, n) {
                let (c, p) = choose|c: ChainId, p: int| selected(raw, c, p) && t.tables_view()[c as int][p] == n;
                assert(union(t.tables_view(), differences@, c, p));
                assert(requested(t, differences@, n));
            }
            if exists|c: ChainId, p: int| union(t.tables_view(), differences@, c, p) && t.tables_view()[c as int][p] == n {
                let (c, p) = choose|c: ChainId, p: int| union(t.tables_view(), differences@, c, p) && t.tables_view()[c as int][p] == n;
                assert(selected(raw, c, p));
                assert(raw_node(t, raw, n));
            }
        }
    }
    let queues = traversal_step::initialize(t, &pending, remaining);
    assert forall|n: NodeIdx| pending::selected(t, pending@, n) == requested(t, differences@, n) by {
        assert(pending::selected(t, pending@, n) == raw_node(t, raw, n));
    }
    (pending, queues, remaining)
}

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{author::Author, commit_hash::CommitHash};

    #[test]
    fn clock_union_keeps_holes_deduplicates_overlaps_and_counts_records() {
        let mut t = Topology::new();
        let mut nodes = Vec::new();
        let hashes: Vec<_> = (10..16).map(|b| CommitHash([b; 32])).collect();
        for i in 0..6 {
            let parents = if i == 0 { &[][..] } else { &hashes[i - 1..i] };
            nodes.push(
                t.insert(hashes[i], Author([1; 32]), parents, (), 0)
                    .unwrap(),
            );
        }
        let independent = t
            .insert(CommitHash([0; 32]), Author([2; 32]), &[], (), 0)
            .unwrap();
        let chain = t.nodes()[nodes[0].index()].chain;
        let at = |i| Clock::from_sorted(&[(chain, nodes[i])]);
        let empty = Clock::default();
        let (one, two, three, four, five) = (at(1), at(2), at(3), at(4), at(5));
        let other = Clock::from_sorted(&[(t.nodes()[independent.index()].chain, independent)]);
        let differences = [
            (&five, &three),
            (&two, &empty),
            (&five, &four),
            (&one, &four),
            (&other, &empty),
        ];
        let (mut p, mut q, mut remaining) = new(&t, &differences);
        assert_eq!(remaining, 6);
        assert_eq!(p.len(), 2);
        assert_eq!(p[&nodes[0]].intervals.interval_count(), 2);
        for (i, expected) in [
            independent,
            nodes[0],
            nodes[1],
            nodes[2],
            nodes[4],
            nodes[5],
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                traversal_step::next(&t, &mut p, &mut q, &mut remaining),
                Some(expected)
            );
            assert_eq!(remaining, 5 - i);
        }
        assert_eq!(
            traversal_step::next(&t, &mut p, &mut q, &mut remaining),
            None
        );
        assert!(p.is_empty());
        for differences in [&[][..], &[(&five, &five)][..], &[(&one, &four)][..]] {
            let (mut p, mut q, mut remaining) = new(&t, differences);
            assert_eq!(remaining, 0);
            assert_eq!(
                traversal_step::next(&t, &mut p, &mut q, &mut remaining),
                None
            );
        }
    }
}
