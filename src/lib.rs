//! # Stockfoil
//!
//! `stockfoil` is a library for storing and synchronizing authenticated commit
//! graphs
//!
//! ```rust,ignore
//! ```
mod author;
pub use author::Author;
mod blob_hash;
pub use blob_hash::BlobHash;
mod commit_hash;
pub use commit_hash::{CommitHash, InvalidCommitHash};
mod commit_graph;
pub use commit_graph::CommitGraph;
mod commit_meta;
pub use commit_meta::{CommitMeta, CommitContents};
mod parsing;
mod signature;
pub use signature::Signature;
