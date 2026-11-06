use crate::{Author, CommitHash};
use std::fmt;

/// Stockfoil's history, protocol and codec failures. In-memory application
/// failures retain their own types in [`DriveError`](crate::DriveError).
/// Variants describe the first violated rule; an operation that returns one
/// is terminal unless documented otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Error {
    /// A stable machine-readable name for the variant, for bindings and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "Malformed",
            Self::Limit(_) => "Limit",
            Self::WrongFormat => "WrongFormat",
            Self::InvalidItem(_) => "InvalidItem",
            Self::InvalidPublicKey(_) => "InvalidPublicKey",
            Self::InvalidSignature { .. } => "InvalidSignature",
            Self::MissingSignature { .. } => "MissingSignature",
            Self::CorruptPayload(_) => "CorruptPayload",
            Self::InvalidPlacement => "InvalidPlacement",
            Self::MissingDependency(_) => "MissingDependency",
            Self::UnknownCommit(_) => "UnknownCommit",
            Self::MissingParent(_) => "MissingParent",
            Self::NotDraft(_) => "NotDraft",
            Self::WrongHistory => "WrongHistory",
            Self::StaleBatch => "StaleBatch",
            Self::Stale => "Stale",
            Self::Poisoned => "Poisoned",
            Self::Codec(_) => "Codec",
            Self::Storage(_) => "Storage",
            Self::Protocol(_) => "Protocol",
            Self::Exhausted(_) => "Exhausted",
            Self::OutOfRange(_) => "OutOfRange",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "malformed item: {what}"),
            Self::Limit(what) => write!(f, "limit exceeded: {what}"),
            Self::WrongFormat => write!(f, "unexpected application format"),
            Self::InvalidItem(what) => write!(f, "invalid item: {what}"),
            Self::InvalidPublicKey(_) => write!(f, "invalid author public key"),
            Self::InvalidSignature { member, .. } => {
                write!(f, "invalid signature on member {member}")
            }
            Self::MissingSignature { member, .. } => {
                write!(
                    f,
                    "member {member} is not covered by its author's signature"
                )
            }
            Self::CorruptPayload(_) => write!(f, "stored payload does not match its commit"),
            Self::InvalidPlacement => write!(f, "fragment is not canonically placed"),
            Self::MissingDependency(_) => write!(f, "missing admitted dependency"),
            Self::UnknownCommit(_) => write!(f, "unknown commit"),
            Self::MissingParent(_) => write!(f, "parent has not been delivered"),
            Self::NotDraft(_) => write!(f, "commit is not an unadmitted local draft"),
            Self::WrongHistory => write!(f, "used with a different history"),
            Self::StaleBatch => write!(f, "batch is not outstanding on this materialization"),
            Self::Stale => write!(f, "materialization is not current"),
            Self::Poisoned => write!(f, "application state must be restored or rebuilt"),
            Self::Codec(e) => write!(f, "codec failed: {e}"),
            Self::Storage(e) => write!(f, "storage failed: {e}"),
            Self::Protocol(what) => write!(f, "host protocol violation: {what}"),
            Self::Exhausted(what) => write!(f, "{what} exhausted"),
            Self::OutOfRange(what) => write!(f, "no such {what}"),
        }
    }
}

impl std::error::Error for Error {}
