//! # Stockfoil
//!
//! `stockfoil` is a library for storing and synchronizing authenticated commit
//! graphs
//!
//! ```rust,ignore
//! ```
mod author;
pub use author::Author;
mod auth_error;
pub use auth_error::{DiffError, QueryError, ResolveError, SignatureError};
mod blob_hash;
pub use blob_hash::BlobHash;
mod commit_hash;
pub use commit_hash::{CommitHash, InvalidCommitHash};
mod commit_graph;
pub use commit_graph::{CommitGraph, NodeIdx};
mod commit_meta;
pub use commit_meta::{CommitContents, CommitMeta};
mod parsing;
mod pov;
pub use pov::{CommitDiff, Pov, PovDiff, ResolvedPov, Vis, VisDiff};
mod signature;
pub use signature::Signature;
mod write_permission;
pub use write_permission::WritePermission;
