//! A structural sedimentree summary of publishable history.
//!
//! This is not an inventory of encoded blobs or a proof of fragment-local author
//! coverage. Encoding, blob metadata, sealing, and reconciliation are out of scope.

use std::collections::{BTreeMap, BTreeSet};

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
            fragments: self.index.cover.values().cloned().collect(),
        }
    }
}

fn depth(hash: CommitHash) -> usize {
    hash.as_bytes()
        .iter()
        .take_while(|byte| **byte == 0)
        .count()
}

#[derive(Clone)]
struct IndexedCommit {
    depth: usize,
    parents: BTreeSet<CommitHash>,
}

#[derive(Clone)]
struct IndexedRange {
    // Exact membership permits local minimization without relying on truncated
    // checkpoints. Retain contained ranges even after they leave the cover.
    members: BTreeSet<CommitHash>,
}

/// Append-only canonical ranges plus a maintained, reduced cover. Only causally
/// complete publishable commits enter this index, in parent-before-child order.
#[derive(Clone, Default)]
pub(crate) struct SedimentreeIndex {
    commits: BTreeMap<CommitHash, IndexedCommit>,
    ranges: BTreeMap<FragmentRange, IndexedRange>,
    loose: BTreeMap<CommitHash, LooseCommit>,
    cover: BTreeMap<FragmentRange, Fragment>,
}

impl SedimentreeIndex {
    pub(crate) fn insert(&mut self, meta: &CommitMeta) {
        let head = *meta.hash();
        if self.commits.contains_key(&head) {
            return;
        }
        let head_depth = depth(head);
        debug_assert!(meta.parents().iter().all(|p| self.commits.contains_key(p)));
        self.commits.insert(
            head,
            IndexedCommit {
                depth: head_depth,
                parents: meta.parents().iter().copied().collect(),
            },
        );
        if head_depth == 0 {
            self.loose.insert(
                head,
                LooseCommit {
                    commit: head,
                    parents: self.commits[&head].parents.clone(),
                },
            );
            return;
        }

        let mut members = BTreeSet::new();
        let mut boundary = BTreeSet::new();
        let mut checkpoints = BTreeSet::new();
        let mut stack = vec![head];
        while let Some(hash) = stack.pop() {
            if !members.insert(hash) {
                continue;
            }
            let commit = &self.commits[&hash];
            if hash != head && commit.depth >= head_depth {
                boundary.insert(hash);
                continue;
            }
            if hash != head && commit.depth > 0 {
                checkpoints.insert(Checkpoint(hash.as_bytes()[..12].try_into().unwrap()));
            }
            stack.extend(commit.parents.iter().copied());
        }

        // A new head cannot be inside an older range: insertion is causal.
        // Drop only whole ranges contained by this one at equal or lower depth.
        // Adjacent ranges can overlap at their inclusive boundaries and still
        // both be necessary. This is redundancy removal, not minimum set cover.
        self.cover.retain(|range, _| {
            self.commits[&range.head].depth > head_depth
                || !self.ranges[range].members.is_subset(&members)
        });
        self.loose.retain(|hash, _| !members.contains(hash));
        let range = FragmentRange { head, boundary };
        self.cover.insert(
            range.clone(),
            Fragment {
                range: range.clone(),
                checkpoints,
            },
        );
        self.ranges.insert(range, IndexedRange { members });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
