//! Graph-local ancestry clocks. A down-set intersects each chain in a prefix.
//! Positions are topological NodeIdx values, not consecutive per-chain counters.

use std::collections::BTreeMap;

use crate::NodeIdx;

/// Sorted, sparse chain coordinates. Absent chains contain no commits (including
/// NodeIdx(0)); no sentinel position is used. Coordinates are meaningful only
/// within one graph's labeling (including the shared prefix of a clone).
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
pub(crate) struct ChainTime{
    pub(crate) chain: usize,
    pub(crate) position: NodeIdx,
}
