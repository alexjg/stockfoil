//! A structural sedimentree summary of publishable history.
//!
//! This is not an inventory of encoded blobs or a proof of fragment-local author
//! coverage. Encoding, blob metadata, sealing, and reconciliation are out of scope.

use std::collections::{BTreeMap, BTreeSet};

use im::OrdSet;

use crate::{CommitHash, CommitMeta};

/// Inclusive head and boundary commits of a canonical fragment.
/// A walk stops at ancestors at least as deep as the head, or at roots.
/// Roots below the head's depth are included without becoming boundaries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FragmentRange {
    pub head: CommitHash,
    pub boundary: BTreeSet<CommitHash>,
}

/// Truncated interior checkpoint identifier. Never used as an internal index key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Checkpoint(pub [u8; 12]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseCommit {
    pub commit: CommitHash,
    pub parents: BTreeSet<CommitHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub range: FragmentRange,
    /// Nonzero-depth interior commits, excluding the head and boundaries.
    pub checkpoints: BTreeSet<Checkpoint>,
}

/// A deterministic, reduced structural cover for one graph.
///
/// Includes publishable delivered history, independently of write permissions
/// and viewpoints. Uses the graph's existing publication rules, not the stronger
/// proof requirements that independently verifiable blobs will eventually need.
/// Entries are sorted by full commit hash / fragment range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedimentreeSummary {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

/// Borrows maintained metadata indexes, never commit payloads.
/// Creating a view and reading its summary do not decompose the graph again.
pub struct SedimentreeView<'graph> {
    pub(crate) index: &'graph SedimentreeIndex,
}

impl SedimentreeView<'_> {
    /// Copy the current cover's metadata, with work proportional to summary size.
    /// No payload access, graph traversal, encoding, or IO is performed.
    pub fn summary(&self) -> SedimentreeSummary {
        SedimentreeSummary {
            loose_commits: self.index.loose.values().cloned().collect(),
            fragments: self
                .index
                .cover
                .values()
                .map(|f| f.metadata.clone())
                .collect(),
        }
    }
}

fn depth(hash: CommitHash) -> usize {
    hash.as_bytes()
        .iter()
        .take_while(|byte| **byte == 0)
        .count()
}

// Persistent sets of local topological node indices. Extending a chain shares
// the old set's tree, rather than copying all its members at every commit.
type Members = OrdSet<usize>;

// OrdSet::union reinserts shared members, copying their tree paths unnecessarily.
// Diff skips shared subtrees. For a small operand, use membership checks instead
// so adding a singleton doesn't enumerate the larger set's diff removals.
fn union(left: Members, right: Members) -> Members {
    let (base, other) = if left.len() >= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut result = base.clone();
    if other.len() < base.len() / 2 {
        for &member in &other {
            if !result.contains(&member) {
                result.insert(member);
            }
        }
    } else {
        for change in base.diff(&other) {
            if let im::ordset::DiffItem::Add(&member) = change {
                result.insert(member);
            }
        }
    }
    result
}

/// What this node contributes to a future fragment at each depth, starting at 1.
/// The last entry has no boundaries: it also serves every greater depth. Thus a
/// history with no checkpoints needs one entry, not 32 identical entries.
#[derive(Clone)]
struct Prefixes(Vec<Members>);

impl Prefixes {
    fn at(&self, depth: usize) -> &Members {
        &self.0[(depth - 1).min(self.0.len() - 1)]
    }

    fn extend(node: usize, depth: usize, parents: &[&Self]) -> (Self, Option<Members>) {
        // A prefix's largest index is its tip. Newest-first lets us skip an
        // ancestor parent once a descendant's prefix has already included it.
        let mut parents = parents.to_vec();
        parents.sort_unstable_by(|a, b| b.at(1).get_max().cmp(&a.at(1).get_max()));
        let levels = parents
            .iter()
            .map(|p| p.0.len())
            .max()
            .unwrap_or(1)
            .max(depth + 1);
        let singleton = Members::unit(node);
        let mut fragment = None;
        let prefixes = (1..=levels)
            .map(|level| {
                if level < depth {
                    return singleton.clone();
                }
                let mut members = Members::new();
                for parent in &parents {
                    let prefix = parent.at(level);
                    // Prefixes are closed under this level's boundary walk:
                    // including the tip includes its entire prefix, even when
                    // the tip is itself a boundary (then its prefix is {tip}).
                    if !members.contains(prefix.get_max().unwrap()) {
                        members = union(members, prefix.clone());
                    }
                }
                members.insert(node);
                if level == depth {
                    // Close the fragment BEFORE resetting this level's prefix.
                    fragment = Some(members);
                    singleton.clone()
                } else {
                    members
                }
            })
            .collect();
        (Self(prefixes), fragment)
    }
}

#[derive(Clone)]
struct IndexedNode {
    depth: usize,
    prefixes: Prefixes,
}

#[derive(Clone)]
struct CoveredFragment {
    metadata: Fragment,
    members: Members,
}

/// Derived coverage only: no copy of the graph's parent links or commit metadata.
/// Entries use the graph's node indices, with holes for withheld local history.
/// Old prefixes remain available for forks from any previously published node.
#[derive(Clone, Default)]
pub(crate) struct SedimentreeIndex {
    nodes: Vec<Option<IndexedNode>>,
    loose: BTreeMap<CommitHash, LooseCommit>,
    // A fully delivered head has exactly one canonical range, so sorting by head
    // gives the same summary order as sorting by (head, boundary).
    cover: BTreeMap<CommitHash, CoveredFragment>,
}

impl SedimentreeIndex {
    pub(crate) fn insert<'a>(
        &mut self,
        node: usize,
        meta: &CommitMeta,
        parents: impl Iterator<Item = usize>,
        metadata: impl Fn(usize) -> &'a CommitMeta,
    ) {
        if self.nodes.get(node).is_some_and(Option::is_some) {
            return;
        }
        let head = *meta.hash();
        let head_depth = depth(head);
        let parents: Vec<_> = parents
            .map(|p| {
                &self.nodes[p]
                    .as_ref()
                    .expect("parent must be publishable")
                    .prefixes
            })
            .collect();
        let (prefixes, members) = Prefixes::extend(node, head_depth, &parents);
        if self.nodes.len() <= node {
            self.nodes.resize_with(node + 1, || None);
        }
        self.nodes[node] = Some(IndexedNode {
            depth: head_depth,
            prefixes,
        });

        let Some(members) = members else {
            self.loose.insert(
                head,
                LooseCommit {
                    commit: head,
                    parents: meta.parents().iter().copied().collect(),
                },
            );
            return;
        };

        let mut boundary = BTreeSet::new();
        let mut checkpoints = BTreeSet::new();
        for &member in &members {
            if member == node {
                continue;
            }
            let hash = *metadata(member).hash();
            let depth = self.nodes[member].as_ref().unwrap().depth;
            if depth >= head_depth {
                boundary.insert(hash);
            } else if depth > 0 {
                checkpoints.insert(Checkpoint(hash.as_bytes()[..12].try_into().unwrap()));
            }
            self.loose.remove(&hash);

            // Only ranges whose heads are included can be redundant. A shallower
            // head's whole range is included; an equal-depth boundary needs an
            // exact subset check (another merge path may include its ancestry).
            if self.cover.get(&hash).is_some_and(|old| {
                depth < head_depth || (depth == head_depth && old.members.is_subset(&members))
            }) {
                self.cover.remove(&hash);
            }
        }
        self.cover.insert(
            head,
            CoveredFragment {
                metadata: Fragment {
                    range: FragmentRange { head, boundary },
                    checkpoints,
                },
                members,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A deliberately slow oracle for the pure set recurrence. Synthetic depths
    // let us exercise depth 32 too; integration tests mine actual commit hashes.
    fn walk(
        nodes: &[(usize, Vec<usize>)],
        head: usize,
        level: usize,
        close: bool,
    ) -> BTreeSet<usize> {
        let mut members = BTreeSet::new();
        let mut stack = vec![head];
        while let Some(node) = stack.pop() {
            if !members.insert(node) {
                continue;
            }
            if nodes[node].0 >= level && !(close && node == head) {
                continue;
            }
            stack.extend(&nodes[node].1);
        }
        members
    }

    #[test]
    fn prefixes_match_boundary_walks_on_random_dags() {
        use rand::{RngExt, SeedableRng};

        let mut rng = rand::rngs::StdRng::seed_from_u64(17);
        for _ in 0..16 {
            let mut nodes = Vec::new();
            let mut prefixes: Vec<Prefixes> = Vec::new();
            for node in 0..64 {
                let depth = [0, 0, 0, 1, 2, 4, 32][rng.random_range(0..7)];
                let parents: Vec<_> = if node == 0 {
                    vec![]
                } else {
                    (0..rng.random_range(0..=3))
                        .map(|_| rng.random_range(0..node))
                        .collect()
                };
                let parent_prefixes: Vec<_> = parents.iter().map(|&p| &prefixes[p]).collect();
                let (next, fragment) = Prefixes::extend(node, depth, &parent_prefixes);
                nodes.push((depth, parents));
                for level in 1..=33 {
                    assert_eq!(
                        next.at(level).iter().copied().collect::<BTreeSet<_>>(),
                        walk(&nodes, node, level, false),
                        "node {node}, depth {level}"
                    );
                }
                assert_eq!(
                    fragment.map(|s| s.iter().copied().collect::<BTreeSet<_>>()),
                    (depth > 0).then(|| walk(&nodes, node, depth, true))
                );
                prefixes.push(next);
            }
        }
    }

    #[test]
    fn chain_prefixes_share_snapshots_and_keep_old_forks_available() {
        let (root, _) = Prefixes::extend(0, 0, &[]);
        let mut chain = vec![root];
        for node in 1..2048 {
            let (next, fragment) = Prefixes::extend(node, 0, &[&chain[node - 1]]);
            assert!(fragment.is_none());
            assert_eq!(next.0.len(), 1); // All depths share the unbounded prefix.
            assert_eq!(next.at(32).len(), node + 1);
            chain.push(next);
        }
        let (closed, fragment) = Prefixes::extend(2048, 4, &[&chain[2047]]);
        assert_eq!(fragment.unwrap().len(), 2049);
        for level in 1..=4 {
            assert!(closed.at(1).ptr_eq(closed.at(level)));
        }
        assert_eq!(closed.at(5).len(), 2049);
        let (_, branch) = Prefixes::extend(2049, 1, &[&chain[1]]);
        assert_eq!(branch.unwrap(), Members::from(vec![0, 1, 2049]));
        assert_eq!(chain[1].at(1), &Members::from(vec![0, 1]));
    }

    #[test]
    fn repeated_diamonds_do_not_reinsert_shared_history() {
        let mut states = vec![Prefixes::extend(0, 0, &[]).0];
        for _ in 0..512 {
            let base = states.len() - 1;
            states.push(Prefixes::extend(base + 1, 0, &[&states[base]]).0);
            states.push(Prefixes::extend(base + 2, 0, &[&states[base]]).0);
            states.push(Prefixes::extend(base + 3, 0, &[&states[base + 1], &states[base + 2]]).0);
            let merged = states.last().unwrap().at(1);
            assert_eq!(merged.len(), states.len());
            // Union with an already-included parent must preserve the tree's
            // identity, not merely its values. Ordinary OrdSet::union fails this.
            assert!(union(merged.clone(), states[base].at(1).clone()).ptr_eq(merged));
            assert!(union(merged.clone(), merged.clone()).ptr_eq(merged));
        }
    }

    #[test]
    fn redundant_ancestor_parents_do_not_expand_the_prefix_in_either_order() {
        let mut states = vec![Prefixes::extend(0, 0, &[]).0];
        for node in 1..1536 {
            states.push(Prefixes::extend(node, 0, &[&states[node - 1]]).0);
        }
        for node in 1536..2048 {
            let older = &states[511];
            let tip = &states[node - 1];
            let (forward, _) = Prefixes::extend(node, 0, &[older, tip]);
            let (reverse, _) = Prefixes::extend(node, 0, &[tip, older]);
            assert_eq!(forward.at(1), reverse.at(1));
            assert_eq!(forward.at(1).len(), node + 1);
            states.push(forward);
        }
    }

    #[test]
    fn wide_merges_union_small_disjoint_prefixes() {
        let roots: Vec<_> = (0..2048)
            .map(|node| Prefixes::extend(node, 0, &[]).0)
            .collect();
        let parents: Vec<_> = roots.iter().collect();
        let (merged, fragment) = Prefixes::extend(2048, 1, &parents);
        assert_eq!(fragment.unwrap(), (0usize..=2048).collect::<Members>());
        assert_eq!(merged.at(1), &Members::unit(2048));
        assert_eq!(merged.at(2).len(), 2049);
        // The small-operand path must preserve sharing for a no-op, too.
        assert!(union(merged.at(2).clone(), roots[0].at(1).clone()).ptr_eq(merged.at(2)));
        assert_eq!(roots[0].at(1), &Members::unit(0));
    }

    #[test]
    fn depth_counts_zero_bytes_not_hex_digits_or_bits() {
        for (prefix, expected) in [
            (vec![0x01], 0),
            (vec![0x0f], 0),
            (vec![0x00, 0x12], 1),
            (vec![0x00, 0x00, 0x12], 2),
            (vec![0x00; 32], 32),
        ] {
            let mut bytes = [0xff; 32];
            bytes[..prefix.len()].copy_from_slice(&prefix);
            assert_eq!(depth(CommitHash::from(bytes)), expected);
        }
    }
}
