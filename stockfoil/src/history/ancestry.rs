//! Captured ancestry and participation, plus lazy commit-set differences.
use super::clock::Clock;
use super::traversal::Traversal;
use super::{History, WritePermissions, view};
use crate::{CommitHash, Error};
use std::sync::Arc;

/// The causal past of some heads, inclusive, with participation under the
/// permissions captured when the handle was created.
///
/// Construct with [`History::ancestry`]. Cloning shares the clocks and heads;
/// the handle owns no payloads or copy of the graph. It does not borrow the
/// history, so the graph may keep growing while the handle stays unchanged.
/// Membership needs the originating [`History`] to resolve a hash, followed
/// by a chain-clock lookup. A different history, even if it contains the same
/// commits, is rejected with [`Error::WrongHistory`]. Recreate the handle from
/// its heads and permissions on a rebuilt history.
///
/// [`contains`](Self::contains) is causal membership;
/// [`participates`](Self::participates) additionally applies permissions.
/// Hidden commits remain in the causal past: visible descendants may refer to
/// their contents. Unknown hashes return `false` for both queries.
#[derive(Debug, Clone)]
pub struct Ancestry {
    history: u64,
    heads: Arc<[CommitHash]>,
    reached: Arc<Clock>,
    participating: Arc<Clock>,
}

impl Ancestry {
    pub(super) fn new(
        history: &History,
        heads: &[CommitHash],
        permissions: &WritePermissions,
    ) -> Result<Self, Error> {
        let reached = history.history_clock(heads)?;
        let participating = Arc::new(view::participation_in(history, &reached, permissions));
        let mut heads = heads.to_vec();
        heads.sort_unstable();
        heads.dedup();
        Ok(Self {
            history: history.id,
            heads: heads.into(),
            reached,
            participating,
        })
    }

    /// The supplied heads, sorted and deduplicated. Redundant ancestors are
    /// kept; all heads were checked as delivered when the handle was created.
    pub fn heads(&self) -> &[CommitHash] {
        &self.heads
    }

    /// Whether `hash` is one of the heads or an ancestor of one, regardless of
    /// permissions. `history` must be the one this handle was created from.
    pub fn contains(&self, history: &History, hash: &CommitHash) -> Result<bool, Error> {
        self.check_history(history)?;
        Ok(history
            .by_hash
            .get(hash)
            .is_some_and(|&idx| history.contains(&self.reached, idx)))
    }

    /// Causal membership **and** permission to participate in the captured
    /// view. Later permission changes do not alter this handle. Missing or
    /// denied authors do not participate; a bounded frontier grants only its
    /// ancestors, and only if the frontier itself is in the captured history.
    pub fn participates(&self, history: &History, hash: &CommitHash) -> Result<bool, Error> {
        self.check_history(history)?;
        Ok(history
            .by_hash
            .get(hash)
            .is_some_and(|&idx| history.contains(&self.participating, idx)))
    }

    /// Commits in `self`'s causal past but not `other`'s. Permissions on either
    /// handle are ignored; this is causal set difference, not a visibility
    /// diff. Both handles must originate from `history`.
    ///
    /// The iterator is lazy and exact-sized, and uses the same canonical
    /// ancestor-first, smallest-ready-hash order as materialization planning. Its
    /// working state scales with touched chains, not the number of commits it
    /// will return. The iterator borrows the history, not the handles.
    pub fn difference<'h>(&self, other: &Self, history: &'h History) -> Result<Commits<'h>, Error> {
        self.check_history(history)?;
        other.check_history(history)?;
        Ok(Commits::new(history, &self.reached, &other.reached))
    }

    fn check_history(&self, history: &History) -> Result<(), Error> {
        if self.history != history.id {
            return Err(Error::WrongHistory);
        }
        Ok(())
    }
}

/// A lazy, captured commit-set difference from [`Ancestry::difference`] or
/// [`History::commits_since`]. No commit list is expanded up front.
///
/// Ancestors come first, smallest hash first among ready selected commits.
/// Each commit is returned once, including drafts, regardless of permissions.
#[derive(Debug)]
pub struct Commits<'h> {
    history: &'h History,
    traversal: Traversal,
}

impl<'h> Commits<'h> {
    pub(super) fn new(history: &'h History, after: &Clock, before: &Clock) -> Self {
        Self {
            history,
            traversal: Traversal::new(history, &[(after, before)]),
        }
    }
}

impl Iterator for Commits<'_> {
    type Item = CommitHash;

    fn next(&mut self) -> Option<Self::Item> {
        let idx = self.traversal.next(self.history)?;
        Some(self.history.node(idx).hash)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.traversal.remaining();
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Commits<'_> {}
impl std::iter::FusedIterator for Commits<'_> {}
