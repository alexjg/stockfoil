//! # Stockfoil
//!
//! Authenticated commit histories for CRDTs, independent of payload contents.
//!
//! - [`History`] holds topology, authorship, logical payload IDs, admission
//!   state, and signature evidence. It never holds payload bytes or calls
//!   application code.
//! - [`Admission`] receives a stored item through explicit host requests
//!   (read, decode, bind), verifying everything itself.
//! - Local commits are hashed with a [`CommitBuilder`], published as drafts,
//!   and admitted once signed ([`History::sign`]).
//! - [`History::export`] plans what a destination is missing; [`wire`] writes
//!   item headers.
//! - [`History::changes`] tells a materializer which commits were delivered or
//!   changed participation, without reading payloads.
//! - [`Graph`] bundles a history with a native codec and storage for
//!   synchronous Rust use.
mod admission;
mod author;
mod blob_hash;
mod commit_hash;
mod error;
mod evidence;
mod graph;
mod history;
mod meta;
mod reader;
mod signature;
mod verify;
pub mod wire;
mod write_permission;

pub use admission::{
    Admission, Counts, DecodeStatus, Next, ReceiveOptions, Reply, ReplyError, Request, Ticket,
};
pub use author::{Author, InvalidAuthor};
pub use blob_hash::{BlobHash, BlobHasher};
pub use commit_hash::{CommitHash, InvalidCommitHash};
pub use error::Error;
pub use graph::{
    Codec, DecodeProgress, Decoder, Driven, EncodeProgress, Encoder, Graph, MemoryStorage,
    PayloadLocation, Storage,
};
pub use history::{
    Batch, Change, Changes, CommitBuilder, CommitDiff, CommitInfo, Delivered, ExportCursor,
    ExportItem, ExportPlan, Heads, History, MaterializationCursor, PayloadCheck, Prepared, Receipt,
    ViewDiff, ViewPoint, Vis, VisDiff, WritePermissions,
};
pub use meta::{
    BundleMeta, Evidence, FormatId, FragmentRange, FragmentShape, ItemId, Limits, MemberShape,
    ParentRef, PayloadId,
};
pub use signature::Signature;
pub use write_permission::WritePermission;
