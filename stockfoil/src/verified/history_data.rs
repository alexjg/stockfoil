//! Production history data types; no admission or cryptographic invariant is assumed.
use super::{author::Author, commit_hash::CommitHash};
use std::collections::BTreeSet;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Application logical payload ID. Stockfoil only stores and returns it.
pub type PayloadId = u64;

/// A sedimentree fragment: the head, plus its ancestry down to (and excluding)
/// the boundary commits and all their ancestors.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub struct FragmentRange {
    pub head: CommitHash,
    pub boundary: BTreeSet<CommitHash>,
}

/// A standalone commit or a canonical fragment.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub enum ItemId {
    Commit(CommitHash),
    Fragment(FragmentRange),
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, Copy, PartialEq, Eq))]
pub struct Signature(pub(crate) [u8; 64]);

/// Signature evidence retained for export: `author` signed `commit`.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, Copy, PartialEq, Eq))]
pub(crate) struct SignatureEvidence {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub(crate) enum Key {
    Commit(CommitHash),
    /// A fragment's boundary is fixed by its head.
    Fragment(CommitHash),
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Certificate {
    pub id: ItemId,
    pub evidence: Vec<SignatureEvidence>,
}

} // verus!
