//! # Stockfoil
//!
//! `stockfoil` is a library for storing and synchronizing authenticated commit
//! graphs
mod ancestor_clock;
mod author;
pub use author::Author;
mod errors;
pub use errors::{
    DiffError, ExportError, FragmentReadError, IngestError, QueryError, ResolveError,
    SignatureError, VerificationError,
};
mod ingest;
pub use ingest::{ItemId, SignatureEvidence, UnverifiedItem, VerifiedItem};
mod blob_hash;
pub use blob_hash::BlobHash;
mod commit_hash;
pub use commit_hash::{CommitHash, InvalidCommitHash};
mod commit_graph;
pub use commit_graph::{CommitGraph, NodeIdx};
mod commit_meta;
pub use commit_meta::{CommitContents, CommitMeta};
mod parsing;
mod view_point;
pub use view_point::{CommitDiff, ResolvedViewPoint, ViewPoint, ViewPointDiff, Vis, VisDiff};
mod signature;
pub use signature::Signature;
mod sedimentree;
pub use sedimentree::{
    Checkpoint, Fragment, FragmentRange, LooseCommit, MinimalCover, SedimentreeSummary,
    SedimentreeView,
};
mod write_permission;
pub use write_permission::WritePermission;
