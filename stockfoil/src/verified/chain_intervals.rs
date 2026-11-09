//! Lazy per-chain selections: normalized intervals, exact progress and dependencies.
use super::clock::{self, NodeIdx};
use std::collections::VecDeque;
use std::ops::Range;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn selected(ranges: Seq<Range<usize>>, position: int) -> bool {
    exists|i: int| 0 <= i < ranges.len() && #[trigger] ranges[i].start <= position < ranges[i].end
}

pub(crate) open spec fn canonical(ranges: Seq<Range<usize>>) -> bool {
    &&& forall|i: int| 0 <= i < ranges.len() ==> ranges[i].start < ranges[i].end
    &&& forall|i: int, j: int| 0 <= i < j < ranges.len() ==> ranges[i].end < ranges[j].start
}

pub(crate) open spec fn weight(ranges: Seq<Range<usize>>) -> int
    decreases ranges.len(),
{
    if ranges.len() == 0 { 0 } else {
        weight(ranges.drop_last()) + ranges.last().end - ranges.last().start
    }
}

/// Finite set of selected positions, independently of the arithmetic count.
pub(crate) open spec fn positions(ranges: Seq<Range<usize>>) -> Set<int>
    decreases ranges.len(),
{
    if ranges.len() == 0 { Set::empty() } else {
        positions(ranges.drop_last()).union(Set::range(ranges.last().start as int, ranges.last().end as int))
    }
}

pub(crate) proof fn positions_membership(ranges: Seq<Range<usize>>, p: int)
    ensures positions(ranges).contains(p) == selected(ranges, p),
    decreases ranges.len(),
{
    broadcast use vstd::set_lib::range_set_properties;
    if ranges.len() > 0 {
        positions_membership(ranges.drop_last(), p);
        assert(ranges =~= ranges.drop_last().push(ranges.last()));
        selected_push(ranges.drop_last(), ranges.last(), p);
    }
}

pub(crate) proof fn cardinality(ranges: Seq<Range<usize>>)
    requires canonical(ranges),
    ensures positions(ranges).len() == weight(ranges),
    decreases ranges.len(),
{
    if ranges.len() > 0 {
        let prior = ranges.drop_last();
        let last = ranges.last();
        cardinality(prior);
        vstd::set_lib::lemma_int_range(last.start as int, last.end as int);
        let tail = Set::range(last.start as int, last.end as int);
        assert forall|p: int| positions(prior).contains(p) implies !tail.contains(p) by {
            positions_membership(prior, p);
            let i = choose|i: int| 0 <= i < prior.len() && #[trigger] prior[i].start <= p < prior[i].end;
            assert(ranges[i].end < last.start);
        }
        assert(positions(prior).disjoint(tail));
        vstd::set_lib::lemma_set_disjoint_lens(positions(prior), tail);
    }
}

proof fn weight_bound(ranges: Seq<Range<usize>>)
    requires canonical(ranges),
    ensures 0 <= weight(ranges),
        ranges.len() == 0 ==> weight(ranges) == 0,
        ranges.len() > 0 ==> 0 < weight(ranges) <= ranges.last().end - ranges[0].start,
    decreases ranges.len(),
{
    if ranges.len() > 0 {
        weight_bound(ranges.drop_last());
        if ranges.len() > 1 { assert(ranges[ranges.len() - 2].end < ranges.last().start); }
    }
}

pub(crate) proof fn selected_push(ranges: Seq<Range<usize>>, range: Range<usize>, p: int)
    ensures selected(ranges.push(range), p) == (selected(ranges, p) || range.start <= p < range.end),
{
    if selected(ranges.push(range), p) {
        let i = choose|i: int| 0 <= i < ranges.push(range).len() && #[trigger] ranges.push(range)[i].start <= p < ranges.push(range)[i].end;
        if i < ranges.len() { assert(ranges[i].start <= p < ranges[i].end); }
    }
    if selected(ranges, p) {
        let i = choose|i: int| 0 <= i < ranges.len() && #[trigger] ranges[i].start <= p < ranges[i].end;
        assert(ranges.push(range)[i].start <= p < ranges.push(range)[i].end);
    }
    if range.start <= p < range.end { assert(ranges.push(range)[ranges.len() as int].start <= p < ranges.push(range)[ranges.len() as int].end); }
}

fn merge_next(merged: &mut VecDeque<Range<usize>>, start: usize, end: usize)
    requires canonical(old(merged)@), start < end,
        forall|i: int| 0 <= i < old(merged).len() ==> old(merged)@[i].start <= start,
    ensures canonical(final(merged)@),
        forall|p: int| selected(final(merged)@, p) == (selected(old(merged)@, p) || start <= p < end),
        forall|i: int| 0 <= i < final(merged).len() ==> final(merged)@[i].start <= start,
{
    let ghost prior = merged@;
    let n = merged.len();
    if n > 0 && start <= merged[n - 1].end {
        let last_start = merged[n - 1].start;
        let last_end = merged[n - 1].end;
        let new_end = if last_end < end { end } else { last_end };
        merged[n - 1] = Range { start: last_start, end: new_end };
        assert forall|p: int| selected(merged@, p) == (selected(prior, p) || start <= p < end) by {
            assert(prior =~= prior.drop_last().push(prior.last()));
            assert(merged@ =~= prior.drop_last().push(Range { start: last_start, end: new_end }));
            selected_push(prior.drop_last(), prior.last(), p);
            selected_push(prior.drop_last(), Range { start: last_start, end: new_end }, p);
        }
    } else {
        merged.push_back(Range { start, end });
        assert forall|p: int| selected(merged@, p) == (selected(prior, p) || start <= p < end) by {
            selected_push(prior, Range { start, end }, p);
        }
    }
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Intervals {
    ranges: VecDeque<Range<usize>>,
}

impl Intervals {
    pub(crate) closed spec fn view(&self) -> Seq<Range<usize>> { self.ranges@ }
    pub(crate) open spec fn contains(&self, p: int) -> bool { selected(self.view(), p) }
    pub(crate) open spec fn bounded(&self, end: usize) -> bool {
        forall|p: int| self.contains(p) ==> 0 <= p < end
    }
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { canonical(self.view()) }

    pub(crate) proof fn facts(tracked &self)
        ensures canonical(self.view()),
            positions(self.view()).len() == weight(self.view()),
            forall|p: int| positions(self.view()).contains(p) == self.contains(p),
    {
        use_type_invariant(self);
        cardinality(self.view());
        assert forall|p: int| positions(self.view()).contains(p) == self.contains(p) by {
            positions_membership(self.view(), p);
        }
    }

    pub(crate) fn new(mut ranges: Vec<Range<usize>>) -> (out: Self)
        requires forall|i: int| 0 <= i < ranges.len() ==> ranges[i].start < ranges[i].end,
        ensures forall|p: int| out.contains(p) == selected(ranges@, p),
    {
        let ghost input = ranges@;
        super::stdlib::sort_ranges(&mut ranges);
        proof {
            assert forall|p: int| selected(ranges@, p) == selected(input, p) by {
                if selected(ranges@, p) {
                    let i = choose|i: int| 0 <= i < ranges.len() && #[trigger] ranges[i].start <= p < ranges[i].end;
                    assert(ranges@.to_set().contains(ranges@[i]));
                    assert(input.contains(ranges@[i]));
                    let j = choose|j: int| 0 <= j < input.len() && input[j] == ranges@[i];
                }
                if selected(input, p) {
                    let i = choose|i: int| 0 <= i < input.len() && #[trigger] input[i].start <= p < input[i].end;
                    assert(input.to_set().contains(input[i]));
                    assert(ranges@.contains(input[i]));
                    let j = choose|j: int| 0 <= j < ranges.len() && ranges@[j] == input[i];
                }
            }
        }
        proof {
            assert forall|i: int| 0 <= i < ranges.len() implies ranges[i].start < ranges[i].end by {
                assert(ranges@.to_set().contains(ranges@[i]));
                assert(input.contains(ranges@[i]));
                let j = choose|j: int| 0 <= j < input.len() && input[j] == ranges@[i];
            }
        }
        let mut merged = VecDeque::new();
        let mut i = 0;
        while i < ranges.len()
            invariant i <= ranges.len(), canonical(merged@),
                forall|j: int| 0 <= j < ranges.len() ==> ranges[j].start < ranges[j].end,
                forall|j: int, k: int| 0 <= j < k < ranges.len() ==> ranges[j].start <= ranges[k].start,
                forall|p: int| selected(merged@, p) == selected(ranges@.take(i as int), p),
                forall|j: int, k: int| 0 <= j < merged.len() && i <= k < ranges.len() ==> merged@[j].start <= ranges[k].start,
            decreases ranges.len() - i,
        {
            merge_next(&mut merged, ranges[i].start, ranges[i].end);
            proof {
                assert forall|p: int| selected(merged@, p) == selected(ranges@.take(i + 1), p) by {
                    assert(ranges@.take(i + 1) =~= ranges@.take(i as int).push(ranges@[i as int]));
                    selected_push(ranges@.take(i as int), ranges@[i as int], p);
                }
            }
            i += 1;
        }
        assert(ranges@.take(ranges.len() as int) == ranges@);
        Self { ranges: merged }
    }

    pub(crate) fn is_empty(&self) -> (out: bool)
        ensures out == (self.view().len() == 0), out == (forall|p: int| !self.contains(p)),
    {
        proof {
            use_type_invariant(self);
            if self.view().len() > 0 { assert(self.contains(self.view()[0].start as int)); }
        }
        self.ranges.len() == 0
    }

    #[cfg(test)]
    pub(crate) fn interval_count(&self) -> usize { self.ranges.len() }

    pub(crate) fn first(&self) -> (out: usize)
        requires self.view().len() > 0,
        ensures self.contains(out as int), forall|p: int| self.contains(p) ==> out <= p,
            out == self.view()[0].start,
    {
        proof {
            use_type_invariant(self);
            assert forall|p: int| self.contains(p) implies self.view()[0].start <= p by {
                let i = choose|i: int| 0 <= i < self.view().len() && #[trigger] self.view()[i].start <= p < self.view()[i].end;
                if i > 0 { assert(self.view()[0].end < self.view()[i].start); }
            }
        }
        self.ranges[0].start
    }

    pub(crate) fn remaining(&self) -> (out: usize)
        ensures out == weight(self.view()), out == positions(self.view()).len(),
    {
        proof { self.facts(); weight_bound(self.view()); }
        let mut total = 0usize;
        let mut i = 0;
        while i < self.ranges.len()
            invariant i <= self.ranges.len(), canonical(self.view()),
                total == weight(self.view().take(i as int)),
            decreases self.ranges.len() - i,
        {
            proof {
                assert(self.view().take(i + 1).drop_last() =~= self.view().take(i as int));
                assert(self.view().take(i + 1).last() == self.view()[i as int]);
                weight_bound(self.view().take(i + 1));
            }
            total += self.ranges[i].end - self.ranges[i].start;
            i += 1;
        }
        assert(self.view().take(self.view().len() as int) == self.view());
        total
    }

    pub(crate) fn advance(self) -> (out: Self)
        requires self.view().len() > 0,
        ensures forall|p: int| out.contains(p) == (self.contains(p) && p != self.view()[0].start),
            weight(out.view()) == weight(self.view()) - 1,
    {
        proof { use_type_invariant(&self); }
        let ghost prior = self.view();
        let mut ranges = self.ranges;
        let start = ranges[0].start;
        let end = ranges[0].end;
        if start + 1 == end {
            ranges.pop_front();
        } else {
            ranges[0] = Range { start: start + 1, end };
        }
        let out = Self { ranges };
        proof {
            assert forall|p: int| out.contains(p) == (selected(prior, p) && p != start) by {
                if out.contains(p) {
                    let i = choose|i: int| 0 <= i < out.view().len() && #[trigger] out.view()[i].start <= p < out.view()[i].end;
                    if start + 1 == end { assert(prior[i + 1].start <= p < prior[i + 1].end); }
                    else { assert(prior[i].start <= p < prior[i].end); }
                }
                if selected(prior, p) && p != start {
                    let i = choose|i: int| 0 <= i < prior.len() && #[trigger] prior[i].start <= p < prior[i].end;
                    if start + 1 == end {
                        assert(i > 0);
                        assert(out.view()[i - 1].start <= p < out.view()[i - 1].end);
                    } else { assert(out.view()[i].start <= p < out.view()[i].end); }
                }
            }
            weight_front(prior);
            weight_front(out.view());
            if start + 1 != end { assert(out.view().skip(1) =~= prior.skip(1)); }

        }
        out
    }

    /// Largest selected position strictly below the partition point `end`.
    pub(crate) fn last_before(&self, end: usize) -> (out: usize)
        requires self.view().len() > 0, self.view()[0].start < end,
        ensures self.contains(out as int), out < end,
            forall|p: int| self.contains(p) && p < end ==> p <= out,
    {
        proof { use_type_invariant(self); }
        let mut i = self.ranges.len();
        while i > 0
            invariant 0 < i <= self.ranges.len(), canonical(self.view()),
                self.view()[0].start < end,
                forall|j: int| i <= j < self.ranges.len() ==> self.view()[j].start >= end,
            decreases i,
        {
            i -= 1;
            let start = self.ranges[i].start;
            let stop = self.ranges[i].end;
            if start < end {
                let limit = if stop < end { stop } else { end };
                let out = limit - 1;
                assert(self.contains(out as int));
                assert forall|p: int| self.contains(p) && p < end implies p <= out by {
                    let j = choose|j: int| 0 <= j < self.view().len() && #[trigger] self.view()[j].start <= p < self.view()[j].end;
                    if j < i { assert(self.view()[j].end < self.view()[i as int].start); }
                }
                return out;
            }
        }
        unreachable!()
    }

    /// Same dependency query over the real sorted chain table.
    pub(crate) fn last_node_before(&self, nodes: &[NodeIdx], tip: NodeIdx) -> (out: NodeIdx)
        requires self.view().len() > 0, self.bounded(nodes.len()), clock::ordered(nodes@),
            nodes[self.view()[0].start as int]@ <= tip@,
        ensures exists|i: int| self.contains(i) && nodes[i] == out,
            out@ <= tip@,
            forall|i: int| self.contains(i) && nodes[i]@ <= tip@ ==> nodes[i]@ <= out@,
    {
        let _first = self.first();
        let end = clock::upper_bound(nodes, tip);
        let position = self.last_before(end);
        let out = nodes[position];
        assert forall|i: int| self.contains(i) && nodes[i]@ <= tip@ implies nodes[i]@ <= out@ by {
            assert(i < end && i <= position);
            if i < position { assert(nodes[i]@ <= nodes[position as int]@); }
        }
        out
    }
}

proof fn weight_front(ranges: Seq<Range<usize>>)
    ensures ranges.len() > 0 ==> weight(ranges) == ranges[0].end - ranges[0].start + weight(ranges.skip(1)),
    decreases ranges.len(),
{
    reveal_with_fuel(weight, 2);
    if ranges.len() > 1 {
        weight_front(ranges.drop_last());
        assert(ranges.skip(1).drop_last() =~= ranges.drop_last().skip(1));
        assert(ranges.skip(1).last() == ranges.last());
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn interval_union_and_dependencies_match_exhaustive_sets() {
        let ranges: Vec<_> = (0..5)
            .flat_map(|start| (start + 1..=5).map(move |end| start..end))
            .collect();
        let nodes: Vec<_> = [0, 2, 5, 9, 12].into_iter().map(NodeIdx::new).collect();
        let mut extended = nodes.clone();
        extended.extend([NodeIdx::new(15), NodeIdx::new(20)]);
        for a in &ranges {
            for b in &ranges {
                for c in &ranges {
                    let input = vec![a.clone(), b.clone(), c.clone()];
                    let mut expected: BTreeSet<usize> = input.iter().cloned().flatten().collect();
                    let mut cursor = Intervals::new(input);
                    assert_eq!(cursor.remaining(), expected.len());
                    while let Some(first) = expected.first().copied() {
                        assert!(!cursor.is_empty());
                        assert_eq!(cursor.first(), first);
                        for tip in nodes[first].index()..=22 {
                            let dependency = expected
                                .iter()
                                .copied()
                                .filter(|&i| nodes[i].index() <= tip)
                                .last()
                                .unwrap();
                            assert_eq!(
                                cursor.last_node_before(&nodes, NodeIdx::new(tip as u32)),
                                nodes[dependency]
                            );
                            assert_eq!(
                                cursor.last_node_before(&extended, NodeIdx::new(tip as u32)),
                                nodes[dependency]
                            );
                        }
                        cursor = cursor.advance();
                        expected.remove(&first);
                        assert_eq!(cursor.remaining(), expected.len());
                    }
                    assert!(cursor.is_empty());
                }
            }
        }
    }

    #[test]
    fn empty_and_maximum_position_intervals() {
        let empty = Intervals::new(Vec::new());
        assert!(empty.is_empty());
        assert_eq!(empty.remaining(), 0);
        let cursor = Intervals::new(vec![0..usize::MAX, 1..usize::MAX, 0..1]);
        assert_eq!(cursor.interval_count(), 1);
        assert_eq!(cursor.remaining(), usize::MAX);
        assert_eq!(cursor.last_before(usize::MAX), usize::MAX - 1);
        let cursor = cursor.advance();
        assert_eq!(cursor.first(), 1);
        assert_eq!(cursor.remaining(), usize::MAX - 1);
        let mut tail = Intervals::new(vec![usize::MAX - 2..usize::MAX]);
        assert_eq!(tail.remaining(), 2);
        tail = tail.advance();
        assert_eq!(tail.first(), usize::MAX - 1);
        tail = tail.advance();
        assert!(tail.is_empty());
        assert_eq!(tail.remaining(), 0);
    }
}
