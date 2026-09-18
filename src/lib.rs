//! # Stockfoil
//!
//! `stockfoil` is a library for storing and synchronizing authenticated commit
//! graphs
mod author;
pub use author::Author;
mod errors;
pub use errors::{DiffError, QueryError, ResolveError, SignatureError};
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
    BlobMeta, Checkpoint, CommitRef, CommitSignature, DecodedCommit, DecodedFragment,
    DecodedLooseCommit, EncodedBatch, EncodedItem, Fixup, Fragment, FragmentRange, FragmentRef,
    IngestReport, ItemId, ItemMeta, ItemRef, LooseCommit, SedimentreeSummary, SedimentreeView,
    Side, SignedCommitRef, SummaryError, SyncCodec, SyncError, SyncPlan, VerifiedItem, verify_item,
};
mod write_permission;
pub use write_permission::WritePermission;
