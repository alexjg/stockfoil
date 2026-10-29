//! Graph-local chain-prefix clocks. Each chain contains only one author's commits.
//! Ancestry is parent-closed; permission masks preserve chain prefixes but need not
//! preserve parent closure. Positions are topological NodeIdx values, not counters.

use alloc::{collections::BTreeMap, vec::Vec};

use crate::NodeIdx;

/// Sorted, sparse chain coordinates. Absent chains contain no commits (including
/// NodeIdx(0)); no sentinel position is used. Coordinates are meaningful only
/// within one graph's labeling (including the shared prefix of a clone).
/// Cached ancestry clocks describe exact down-sets. Permission-masked clocks use
/// the same representation, but must not be used as ancestry summaries.
#[derive(Debug, Default)]
pub(crate) struct AncestorClock {
    entries: Vec<(usize, NodeIdx)>,
}

impl AncestorClock {
    pub(crate) fn position(&self, chain: usize) -> Option<NodeIdx> {
        self.entries
            .binary_search_by_key(&chain, |&(chain, _)| chain)
            .ok()
            .map(|i| self.entries[i].1)
    }

    pub(crate) fn contains(&self, chain: usize, position: NodeIdx) -> bool {
        self.position(chain).is_some_and(|tip| tip >= position)
    }

    pub(crate) fn contains_time(&self, time: ChainTime) -> bool {
        self.position(time.chain).is_some_and(|tip| tip >= time.position)
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (usize, NodeIdx)> + '_ {
        self.entries.iter().copied()
    }

    pub(crate) fn latest(&self) -> Option<NodeIdx> {
        self.entries.iter().map(|&(_, position)| position).max()
    }

    /// Cap each coordinate at its permission bound, or remove it for `None`.
    /// Returning the existing tip leaves a chain unbounded within this history.
    /// The result cannot extend this clock, but need not remain parent-closed.
    pub(crate) fn mask(
        &self,
        mut bound: impl FnMut(usize, NodeIdx) -> Option<NodeIdx>,
    ) -> Self {
        Self {
            entries: self
                .entries()
                .filter_map(|(chain, tip)| bound(chain, tip).map(|limit| (chain, tip.min(limit))))
                .collect(),
        }
    }

    /// The current working history contains every node, hence every chain tip.
    pub(crate) fn from_tips(tips: &[NodeIdx]) -> Self {
        Self {
            entries: tips.iter().copied().enumerate().collect(),
        }
    }
}

/// Batch unions avoid repeatedly copying vectors as new coordinates arrive.
#[derive(Default)]
pub(crate) struct ClockBuilder {
    entries: BTreeMap<usize, NodeIdx>,
}

impl ClockBuilder {
    pub(crate) fn include(&mut self, chain: usize, position: NodeIdx) {
        self.entries
            .entry(chain)
            .and_modify(|known| *known = (*known).max(position))
            .or_insert(position);
    }

    pub(crate) fn merge(&mut self, clock: &AncestorClock) {
        for (chain, position) in clock.entries() {
            self.include(chain, position);
        }
    }

    pub(crate) fn finish(self) -> AncestorClock {
        AncestorClock {
            entries: self.entries.into_iter().collect(),
        }
    }
}

/// Like a lamport timestamp, but for ancestor clocks
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainTime {
    pub(crate) chain: usize,
    pub(crate) position: NodeIdx,
}
