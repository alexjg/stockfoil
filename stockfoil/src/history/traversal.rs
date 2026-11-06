//! Canonical lazy enumeration of unions of chain-clock differences.
//! Shared by materialization changes and ancestry queries.
use super::clock::{self, ChainId, Clock};
use super::{History, NodeIdx};
use crate::CommitHash;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;

#[derive(Debug)]
pub(super) struct Traversal {
    pending: BTreeMap<NodeIdx, PendingChain>,
    ready: BTreeSet<(CommitHash, NodeIdx)>,
    blocked: HashMap<NodeIdx, Vec<NodeIdx>>,
    remaining: usize,
}

impl Traversal {
    /// Select the union of `after - before` for each pair. Initialization
    /// retains intervals, never a list of all selected commits.
    pub(super) fn new(history: &History, differences: &[(&Clock, &Clock)]) -> Self {
        let mut intervals: BTreeMap<ChainId, Vec<Range<usize>>> = BTreeMap::new();
        let mut select = |chain, range| intervals.entry(chain).or_default().push(range);
        for &(after, before) in differences {
            clock::difference_intervals(&history.chain_nodes, after, before, &mut select);
        }

        let mut pending = BTreeMap::new();
        let mut remaining = 0;
        for (chain, mut ranges) in intervals {
            ranges.sort_unstable_by_key(|range| range.start);
            let mut merged: VecDeque<Range<usize>> = VecDeque::new();
            for range in ranges {
                if let Some(last) = merged.back_mut()
                    && range.start <= last.end
                {
                    last.end = last.end.max(range.end);
                } else {
                    merged.push_back(range);
                }
            }
            remaining += merged.iter().map(|range| range.len()).sum::<usize>();
            let chain = PendingChain {
                chain,
                intervals: merged,
            };
            pending.insert(chain.first(history), chain);
        }

        let mut traversal = Self {
            pending,
            ready: BTreeSet::new(),
            blocked: HashMap::new(),
            remaining,
        };
        let frontiers: Vec<_> = traversal.pending.keys().copied().collect();
        for idx in frontiers {
            traversal.schedule(history, idx);
        }
        traversal
    }

    #[cfg(test)]
    pub(super) fn retained_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.pending.len(),
            self.pending.values().map(|c| c.intervals.len()).sum(),
            self.ready.len(),
            self.blocked.values().map(Vec::len).sum(),
        )
    }
}

/// The selected portions of one chain, including its next record. Positions
/// are captured now, so appending to the history cannot extend this traversal.
#[derive(Debug)]
struct PendingChain {
    chain: ChainId,
    intervals: VecDeque<Range<usize>>,
}

impl PendingChain {
    fn first(&self, history: &History) -> NodeIdx {
        history.chain_nodes[self.chain as usize][self.intervals[0].start]
    }

    /// Last selected ancestor at or before `tip`, which may lie well beyond
    /// the frontier. Waiting for this one avoids waking on every intervening
    /// commit in a long chain.
    fn last_before(&self, history: &History, tip: NodeIdx) -> NodeIdx {
        let nodes = &history.chain_nodes[self.chain as usize];
        let end = nodes.partition_point(|&idx| idx <= tip);
        let range = self
            .intervals
            .iter()
            .rev()
            .find(|range| range.start < end)
            .expect("the frontier is an ancestor");
        nodes[range.end.min(end) - 1]
    }
}

impl Traversal {
    /// Records remaining in this traversal, without expanding its intervals.
    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }

    /// Only the first pending member of each chain can be ready: all later
    /// members descend from it. A candidate is ready iff none of the other
    /// frontiers is its ancestor. Node indices rule out future ancestors
    /// cheaply, but never determine which ready record is emitted first.
    fn schedule(&mut self, history: &History, idx: NodeIdx) {
        let mut earlier = self.pending.range(..idx).peekable();
        if earlier.peek().is_some() {
            let ancestry = history.clock(idx);
            for (&frontier, chain) in earlier {
                if history.contains(ancestry, frontier) {
                    let tip = ancestry.position(chain.chain).unwrap();
                    let dependency = chain.last_before(history, tip);
                    self.blocked.entry(dependency).or_default().push(idx);
                    return;
                }
            }
        }
        self.ready.insert((history.node(idx).hash, idx));
    }

    pub(super) fn next(&mut self, history: &History) -> Option<NodeIdx> {
        let (_, idx) = self.ready.pop_first()?;
        let mut chain = self.pending.remove(&idx).expect("ready frontier");
        chain.intervals[0].start += 1;
        if chain.intervals[0].is_empty() {
            chain.intervals.pop_front();
        }
        if !chain.intervals.is_empty() {
            let next = chain.first(history);
            self.pending.insert(next, chain);
            self.schedule(history, next);
        }
        if let Some(waiters) = self.blocked.remove(&idx) {
            for waiter in waiters {
                self.schedule(history, waiter);
            }
        }
        self.remaining -= 1;
        Some(idx)
    }
}
