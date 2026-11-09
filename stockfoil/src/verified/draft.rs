//! The actual public local-draft metadata, without an assumption of publication.
use super::{author::Author, commit_hash::CommitHash};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub struct Draft {
    pub hash: CommitHash,
    pub author: Author,
    pub parents: Vec<CommitHash>,
}
} // verus!
