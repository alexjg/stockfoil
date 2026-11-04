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
use std::cmp::Ordering;
use std::sync::Arc;

pub(crate) type ChainId = u32;

#[derive(Clone, Default)]
pub(crate) struct Clock(Option<Arc<Entry>>);

struct Entry {
    chain: ChainId,
    tip: NodeIdx,
    height: u8,
    len: u32,
    left: Clock,
    right: Clock,
}

impl Clock {
    fn height(&self) -> u8 {
        self.0.as_ref().map_or(0, |e| e.height)
    }

    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |e| e.len as usize)
    }

    fn node(chain: ChainId, tip: NodeIdx, left: Self, right: Self) -> Self {
        Self(Some(Arc::new(Entry {
            chain,
            tip,
            height: 1 + left.height().max(right.height()),
            len: 1 + left.len() as u32 + right.len() as u32,
            left,
            right,
        })))
    }

    fn balance(chain: ChainId, tip: NodeIdx, mut left: Self, mut right: Self) -> Self {
        if left.height() > right.height() + 1 {
            let l = left.0.as_ref().unwrap();
            if l.right.height() > l.left.height() {
                let r = l.right.0.as_ref().unwrap();
                left = Self::node(
                    r.chain,
                    r.tip,
                    Self::node(l.chain, l.tip, l.left.clone(), r.left.clone()),
                    r.right.clone(),
                );
            }
            let l = left.0.as_ref().unwrap();
            return Self::node(
                l.chain,
                l.tip,
                l.left.clone(),
                Self::node(chain, tip, l.right.clone(), right),
            );
        }
        if right.height() > left.height() + 1 {
            let r = right.0.as_ref().unwrap();
            if r.left.height() > r.right.height() {
                let l = r.left.0.as_ref().unwrap();
                right = Self::node(
                    l.chain,
                    l.tip,
                    l.left.clone(),
                    Self::node(r.chain, r.tip, l.right.clone(), r.right.clone()),
                );
            }
            let r = right.0.as_ref().unwrap();
            return Self::node(
                r.chain,
                r.tip,
                Self::node(chain, tip, left, r.left.clone()),
                r.right.clone(),
            );
        }
        Self::node(chain, tip, left, right)
    }

    pub fn position(&self, chain: ChainId) -> Option<NodeIdx> {
        let mut node = self.0.as_ref();
        while let Some(e) = node {
            match chain.cmp(&e.chain) {
                Ordering::Less => node = e.left.0.as_ref(),
                Ordering::Greater => node = e.right.0.as_ref(),
                Ordering::Equal => return Some(e.tip),
            }
        }
        None
    }

    pub fn contains(&self, chain: ChainId, position: NodeIdx) -> bool {
        self.position(chain).is_some_and(|tip| tip >= position)
    }

    /// Entries in chain order.
    pub fn entries(&self) -> impl Iterator<Item = (ChainId, NodeIdx)> + '_ {
        let mut stack = Vec::new();
        let mut node = self.0.as_deref();
        std::iter::from_fn(move || {
            while let Some(e) = node {
                stack.push(e);
                node = e.left.0.as_deref();
            }
            let e = stack.pop()?;
            node = e.right.0.as_deref();
            Some((e.chain, e.tip))
        })
    }

    /// Raise `chain`'s position to at least `position`.
    pub fn include(&mut self, chain: ChainId, position: NodeIdx) {
        if self.position(chain).is_some_and(|old| old >= position) {
            return;
        }
        *self = self.set(chain, position);
    }

    fn set(&self, chain: ChainId, tip: NodeIdx) -> Self {
        let Some(e) = &self.0 else {
            return Self::node(chain, tip, Self::default(), Self::default());
        };
        match chain.cmp(&e.chain) {
            Ordering::Less => {
                Self::balance(e.chain, e.tip, e.left.set(chain, tip), e.right.clone())
            }
            Ordering::Greater => {
                Self::balance(e.chain, e.tip, e.left.clone(), e.right.set(chain, tip))
            }
            Ordering::Equal => Self::node(chain, tip, e.left.clone(), e.right.clone()),
        }
    }

    /// The pointwise maximum, sharing the larger side's structure.
    pub fn union(&self, other: &Self) -> Self {
        let (mut big, small) = if self.len() >= other.len() {
            (self.clone(), other)
        } else {
            (other.clone(), self)
        };
        for (chain, tip) in small.entries() {
            big.include(chain, tip);
        }
        big
    }

    /// Cap each coordinate at its bound, or drop it for `None`.
    pub fn mask(&self, mut bound: impl FnMut(ChainId, NodeIdx) -> Option<NodeIdx>) -> Self {
        let entries: Vec<_> = self
            .entries()
            .filter_map(|(chain, tip)| bound(chain, tip).map(|limit| (chain, tip.min(limit))))
            .collect();
        Self::from_sorted(&entries)
    }

    /// A clock containing every node, from a table of chain tips.
    pub fn from_tips(tips: &[NodeIdx]) -> Self {
        let entries: Vec<_> = tips
            .iter()
            .enumerate()
            .map(|(chain, &tip)| (chain as ChainId, tip))
            .collect();
        Self::from_sorted(&entries)
    }

    /// A balanced tree from entries sorted by chain, in linear time.
    fn from_sorted(entries: &[(ChainId, NodeIdx)]) -> Self {
        if entries.is_empty() {
            return Self::default();
        }
        let mid = entries.len() / 2;
        let (chain, tip) = entries[mid];
        Self::node(
            chain,
            tip,
            Self::from_sorted(&entries[..mid]),
            Self::from_sorted(&entries[mid + 1..]),
        )
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
        let start = before
            .position(chain)
            .map_or(0, |pos| nodes.partition_point(|&idx| idx <= pos));
        let end = nodes.partition_point(|&idx| idx <= tip);
        for &idx in nodes.get(start..end).unwrap_or(&[]) {
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
                    let (chain, tip) = (next(200) as ChainId, NodeIdx(next(10_000) as u32));
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
            let entries: Vec<_> = (0..n).map(|i| (i * 3, NodeIdx(i))).collect();
            let clock = Clock::from_sorted(&entries);
            check(&clock);
            assert_eq!(clock.entries().collect::<Vec<_>>(), entries);
        }
    }
}
