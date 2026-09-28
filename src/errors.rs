use std::{error::Error, fmt};

use crate::{Author, CommitHash, ItemId};

/// Signature verification failed. Rejected signatures do not change the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// Separate signatures are accepted only for known, unadmitted local work.
    NotPendingLocalCommit(CommitHash),
    InvalidPublicKey(Author),
    InvalidSignature {
        author: Author,
        commit: CommitHash,
    },
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPendingLocalCommit(hash) => write!(f, "not a pending local commit: {hash:?}"),
            Self::InvalidPublicKey(author) => write!(f, "invalid Ed25519 public key: {author:?}"),
            Self::InvalidSignature { author, commit } => {
                write!(f, "invalid signature by {author:?} for {commit:?}")
            }
        }
    }
}

impl Error for SignatureError {}

/// The graph lacks delivered history needed to compare viewpoints or advance
/// a materialization cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    MissingCommit(CommitHash),
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommit(hash) => write!(f, "missing delivered commit: {hash:?}"),
        }
    }
}

impl Error for DiffError {}

/// A participation query used a hash that is not present in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    UnknownCommit(CommitHash),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommit(hash) => write!(f, "unknown delivered commit: {hash:?}"),
        }
    }
}

impl Error for QueryError {}

/// A canonical fragment could not be read. No traversal callbacks are invoked
/// when validation fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentReadError {
    /// The head is not present in the graph.
    MissingCommit(CommitHash),
    /// No canonical fragment is indexed here (including standalone checkpoints).
    NotFragment(CommitHash),
    /// The head or its ancestors are withheld by publication policy.
    NotPublishable(CommitHash),
    /// The supplied boundaries differ from the head's canonical range.
    BoundaryMismatch(CommitHash),
}

impl fmt::Display for FragmentReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommit(hash) => write!(f, "missing delivered fragment head: {hash:?}"),
            Self::NotFragment(hash) => write!(f, "commit does not head a fragment: {hash:?}"),
            Self::NotPublishable(hash) => write!(f, "fragment is not publishable: {hash:?}"),
            Self::BoundaryMismatch(hash) => {
                write!(f, "non-canonical fragment boundaries: {hash:?}")
            }
        }
    }
}

impl Error for FragmentReadError {}

/// An untrusted sedimentree item could not be verified in isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationError {
    /// The reconstructed metadata does not match the declared item identity or
    /// canonical structure. The serialization-specific details are still deferred.
    InvalidItem,
    InvalidSignature(SignatureError),
    MissingSignature {
        author: Author,
        commit: CommitHash,
    },
}

impl fmt::Display for VerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidItem => write!(f, "invalid sedimentree item identity or structure"),
            Self::InvalidSignature(error) => write!(f, "invalid item signature: {error}"),
            Self::MissingSignature { author, commit } => {
                write!(f, "missing signature coverage by {author:?} for {commit:?}")
            }
        }
    }
}

impl Error for VerificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSignature(error) => Some(error),
            _ => None,
        }
    }
}

/// A statelessly verified item could not be admitted at the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError {
    /// The claimed fragment range or members do not match dependency history.
    InvalidFragment,
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFragment => write!(f, "non-canonical fragment or dependency placement"),
        }
    }
}

impl Error for IngestError {}

/// The requested item is neither a current cover entry nor its signed head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    NotInMinimalCover(ItemId),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInMinimalCover(id) => write!(f, "item is not in the minimal cover: {id:?}"),
        }
    }
}

impl Error for ExportError {}
