//! # Stockfoil
//!
//! Authenticated commit histories for CRDTs, independent of payload contents.
//!
//! - [`History`] holds topology, authorship, logical payload IDs, admission
//!   state, and signature evidence. It never holds payload bytes or calls
//!   application code.
//! - Local commits are hashed with a [`CommitBuilder`], published as drafts,
//!   and admitted once signed ([`History::sign`]).
//! - [`History::export`] plans what a destination is missing; [`wire`] reads
//!   and writes item headers.
//! - [`History::changes`] tells a materializer which commits were delivered or
//!   changed participation, without reading payloads.
mod author;
mod blob_hash;
mod commit_hash;
mod error;
mod evidence;
mod history;
mod meta;
mod reader;
mod signature;
pub mod wire;
mod write_permission;

pub use author::{Author, InvalidAuthor};
pub use blob_hash::{BlobHash, BlobHasher};
pub use commit_hash::{CommitHash, InvalidCommitHash};
pub use error::Error;
pub use history::{
    Batch, Change, Changes, CommitBuilder, CommitDiff, CommitInfo, Delivered, ExportCursor,
    ExportItem, ExportPlan, Heads, History, MaterializationCursor, Prepared, ViewDiff, ViewPoint,
    Vis, VisDiff, WritePermissions,
};
pub use meta::{
    BundleMeta, Evidence, FormatId, FragmentRange, FragmentShape, ItemId, Limits, MemberShape,
    ParentRef, PayloadId,
};
pub use signature::Signature;
pub use write_permission::WritePermission;
