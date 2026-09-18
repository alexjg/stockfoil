use std::{error::Error, fmt};

use crate::{Author, CommitHash};

/// Signature verification failed. Rejected signatures do not change the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    InvalidPublicKey(Author),
    InvalidSignature { author: Author, commit: CommitHash },
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPublicKey(author) => write!(f, "invalid Ed25519 public key: {author:?}"),
            Self::InvalidSignature { author, commit } => {
                write!(f, "invalid signature by {author:?} for {commit:?}")
            }
        }
    }
}

impl Error for SignatureError {}

/// A viewpoint could not be resolved against graph data and signature evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The commit is unknown or still waiting for causal delivery.
    MissingCommit(CommitHash),
    MissingSignature {
        author: Author,
        frontier: CommitHash,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommit(hash) => write!(f, "missing delivered commit: {hash:?}"),
            Self::MissingSignature { author, frontier } => {
                write!(
                    f,
                    "missing signature coverage by {author:?} for {frontier:?}"
                )
            }
        }
    }
}

impl Error for ResolveError {}

/// The graph lacks the data needed to compare two viewpoints.
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

impl From<DiffError> for ResolveError {
    fn from(error: DiffError) -> Self {
        match error {
            DiffError::MissingCommit(hash) => Self::MissingCommit(hash),
        }
    }
}

/// A participation query used a hash that is unknown or not causally delivered.
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
