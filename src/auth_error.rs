//! Error type placeholders. Details will follow with the implementation.

use std::{error::Error, fmt};

/// Signature verification failed.
#[derive(Debug)]
pub struct SignatureError {
    _private: (),
}

impl fmt::Display for SignatureError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("signature error details")
    }
}

impl Error for SignatureError {}

/// A POV could not be resolved against graph data and signature evidence.
#[derive(Debug)]
pub struct ResolveError {
    _private: (),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("resolution error details")
    }
}

impl Error for ResolveError {}

/// The graph lacks the data needed to compare two POVs.
#[derive(Debug)]
pub struct DiffError {
    _private: (),
}

impl fmt::Display for DiffError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("diff error details")
    }
}

impl Error for DiffError {}

/// A participation query could not be answered, for example for an unknown hash.
#[derive(Debug)]
pub struct QueryError {
    _private: (),
}

impl fmt::Display for QueryError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("query error details")
    }
}

impl Error for QueryError {}
