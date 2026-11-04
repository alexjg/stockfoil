//! Chain-prefix clocks. Each chain holds only one author's commits, in topological
//! (node index) order. A clock maps chains to the latest included position; it
//! includes every chain member at or before that position.
//!
//! Clocks are persistent AVL maps: copies share structure, an update copies one
//! root-to-leaf path, and a union copies paths only for the smaller side. Cached
//! clocks of merges therefore cost about `O(log n)` new entries each, not a full
//! copy of every chain.
//!
//! Ancestry clocks describe exact down-sets. Permission-masked clocks use the same
//! representation but need not be parent-closed.
use super::NodeIdx;
use crate::verified::clock as verified;
#[cfg(test)]
use std::sync::Arc;

pub(crate) use crate::verified::tree::{ChainId, Clock};

impl Clock {
    /// Cap each coordinate at its bound, or drop it for `None`.
    pub fn mask(&self, mut bound: impl FnMut(ChainId, NodeIdx) -> Option<NodeIdx>) -> Self {
        let entries: Vec<_> = self
            .entries()
            .filter_map(|(chain, tip)| {
                verified::cap(tip, bound(chain, tip)).map(|limit| (chain, limit))
            })
            .collect();
        Self::from_sorted(&entries)
    }

    /// Visit every tree entry, identified by address, for memory accounting.
    #[cfg(test)]
    pub fn for_each_entry(&self, visit: &mut impl FnMut(*const ()) -> bool) {
        if let Some(e) = &self.0
            && visit(Arc::as_ptr(e) as *const ())
        {
            e.left.for_each_entry(visit);
            e.right.for_each_entry(visit);
        }
    }
}

impl PartialEq for Clock {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.entries().eq(other.entries())
    }
}

impl Eq for Clock {}

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.entries()).finish()
    }
}

/// Accumulates a union of clocks and single coordinates.
#[derive(Default)]
pub(crate) struct ClockBuilder {
    clock: Clock,
}

impl ClockBuilder {
    pub fn include(&mut self, chain: ChainId, position: NodeIdx) {
        self.clock.include(chain, position);
    }

    pub fn merge(&mut self, clock: &Clock) {
        self.clock = self.clock.union(clock);
    }

    pub fn finish(self) -> Clock {
        self.clock
    }
}

/// Every node in `chain_nodes` that one clock includes and the other does not,
/// chain by chain. With `before = None` this enumerates all of `after`.
pub(crate) fn difference(
    chain_nodes: &[Vec<NodeIdx>],
    after: &Clock,
    before: &Clock,
    mut visit: impl FnMut(NodeIdx),
) {
    for (chain, tip) in after.entries() {
        let nodes = &chain_nodes[chain as usize];
        let (start, end) = verified::difference_range(nodes, tip, before.position(chain));
        for &idx in &nodes[start..end] {
            visit(idx);
        }
    }
}

/// Nodes included by exactly one of two clocks.
pub(crate) fn symmetric_difference(
    chain_nodes: &[Vec<NodeIdx>],
    a: &Clock,
    b: &Clock,
    mut visit: impl FnMut(NodeIdx),
) {
    difference(chain_nodes, a, b, &mut visit);
    difference(chain_nodes, b, a, &mut visit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn lookup_extremes_and_cloned_snapshots() {
        let mut clock = Clock::default();
        assert_eq!(clock.position(0), None);
        clock.include(0, NodeIdx::new(0));
        clock.include(5, NodeIdx::new(7));
        clock.include(u32::MAX, NodeIdx::new(u32::MAX));
        let snapshot = clock.clone();
        assert!(Arc::ptr_eq(
            clock.0.as_ref().unwrap(),
            snapshot.0.as_ref().unwrap()
        ));
        clock.include(0, NodeIdx::new(9));
        assert_eq!(snapshot.position(0), Some(NodeIdx::new(0)));
        assert_eq!(clock.position(0), Some(NodeIdx::new(9)));
        assert_eq!(snapshot.position(5), Some(NodeIdx::new(7)));
        assert_eq!(snapshot.position(u32::MAX), Some(NodeIdx::new(u32::MAX)));
        assert_eq!(snapshot.position(4), None);
        assert_eq!(snapshot.position(6), None);
        assert!(snapshot.contains(0, NodeIdx::new(0)));
        assert!(!snapshot.contains(0, NodeIdx::new(1)));
    }

    #[test]
    fn interval_difference_matches_a_linear_filter() {
        let positions = [0, 1, 3, 9, u32::MAX];
        for size in 0..=positions.len() {
            let nodes: Vec<_> = positions[..size]
                .iter()
                .copied()
                .map(NodeIdx::new)
                .collect();
            for tip in positions {
                for prior in [None, Some(0), Some(2), Some(9), Some(u32::MAX)] {
                    let (start, end) = verified::difference_range(
                        &nodes,
                        NodeIdx::new(tip),
                        prior.map(NodeIdx::new),
                    );
                    let expected: Vec<_> = nodes
                        .iter()
                        .copied()
                        .filter(|node| {
                            node.index() <= tip as usize
                                && prior.is_none_or(|p| node.index() > p as usize)
                        })
                        .collect();
                    assert_eq!(&nodes[start..end], expected);
                }
            }
        }
    }

    fn check(c: &Clock) -> u8 {
        let Some(e) = &c.0 else { return 0 };
        let (a, b) = (check(&e.left), check(&e.right));
        assert!(a.abs_diff(b) <= 1);
        assert_eq!(e.height, 1 + a.max(b));
        assert_eq!(e.len as usize, 1 + e.left.len() + e.right.len());
        e.height
    }

    #[test]
    fn persistent_clocks_match_an_ordered_map() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        let mut clocks = vec![(Clock::default(), BTreeMap::new())];
        for _ in 0..4000 {
            let (mut clock, mut model) = clocks[next(clocks.len() as u64) as usize].clone();
            match next(3) {
                0 | 1 => {
                    let (chain, tip) = (next(200) as ChainId, NodeIdx::new(next(10_000) as u32));
                    clock.include(chain, tip);
                    let e = model.entry(chain).or_insert(tip);
                    *e = (*e).max(tip);
                }
                _ => {
                    let (other, other_model) = &clocks[next(clocks.len() as u64) as usize];
                    clock = clock.union(other);
                    for (&k, &v) in other_model {
                        let e = model.entry(k).or_insert(v);
                        *e = (*e).max(v);
                    }
                }
            }
            check(&clock);
            assert_eq!(
                clock.entries().collect::<Vec<_>>(),
                model.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
            );
            assert_eq!(clock.len(), model.len());
            for chain in 0..200 {
                assert_eq!(clock.position(chain), model.get(&chain).copied());
            }
            clocks.push((clock, model));
        }
        // Earlier versions are unchanged by later updates.
        for (clock, model) in &clocks {
            assert_eq!(clock.entries().collect::<BTreeMap<_, _>>(), *model);
        }
    }

    #[test]
    fn from_sorted_is_balanced() {
        for n in 0..300u32 {
            let entries: Vec<_> = (0..n).map(|i| (i * 3, NodeIdx::new(i))).collect();
            let clock = Clock::from_sorted(&entries);
            check(&clock);
            assert_eq!(clock.entries().collect::<Vec<_>>(), entries);
        }
    }
}
