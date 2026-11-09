//! The production public error representation.
use super::{author::Author, commit_hash::CommitHash};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Stockfoil's history, protocol and codec failures. In-memory application
/// failures retain their own types in [`DriveError`](crate::DriveError).
/// Variants describe the first violated rule; an operation that returns one
/// is terminal unless documented otherwise.
#[derive(Debug)]
#[cfg_attr(not(verus_keep_ghost), derive(Clone, PartialEq, Eq))]
#[non_exhaustive]
pub enum Error {
    /// The bytes do not follow the wire format.
    Malformed(&'static str),
    /// A configured structural or resource limit was exceeded.
    Limit(&'static str),
    /// The item names a different application format than the one expected.
    WrongFormat,
    /// Metadata, reconstructed identities, or decoded contents are inconsistent.
    InvalidItem(&'static str),
    InvalidPublicKey(Author),
    /// A signature does not verify. `member` is the untrusted member ordinal.
    InvalidSignature {
        author: Author,
        member: usize,
    },
    /// Some member by `author` is not covered by that author's signatures.
    MissingSignature {
        author: Author,
        member: usize,
    },
    /// Bytes read back from storage are not the payload of this commit.
    CorruptPayload(CommitHash),
    /// The fragment does not sit at its canonical place in admitted history.
    InvalidPlacement,
    /// An admitted dependency is missing. Only blocking conveniences report this;
    /// explicit operations wait instead.
    MissingDependency(CommitHash),
    UnknownCommit(CommitHash),
    /// A local parent has not been delivered.
    MissingParent(CommitHash),
    /// A signature was offered for a commit that is not an unadmitted local draft.
    NotDraft(CommitHash),
    /// An operation or cursor was used with a different history.
    WrongHistory,
    /// A batch belongs to another planner or is not the outstanding batch.
    StaleBatch,
    /// Local edits require a materialization current with its ledger.
    Stale,
    /// Application state may have diverged; restore or rebuild it.
    Poisoned,
    /// The application codec failed.
    Codec(String),
    /// Application storage failed.
    Storage(String),
    /// The host violated the request/reply protocol.
    Protocol(&'static str),
    /// A counter or identifier space was exhausted.
    Exhausted(&'static str),
    /// An index was outside the collection it refers to.
    OutOfRange(&'static str),
}

} // verus!
