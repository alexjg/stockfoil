//! # Stockfoil Core
//!
//! Sans-IO state machines and wire format for authenticated CRDT commit
//! histories, independent of payload contents.
//!
//! - [`History`] holds topology, authorship, logical payload IDs, admission
//!   state, and signature evidence. It never holds payload bytes or calls
//!   application code.
//! - [`Admission`] receives a stored item through explicit host requests
//!   (read, decode, bind), verifying everything itself.
//! - [`DecodeItem`] reads and checks wanted member payloads from a stored item.
//! - Local commits are hashed with a [`CommitBuilder`], published as drafts,
//!   and admitted once signed ([`History::sign`]).
//! - [`History::export`] plans what a destination is missing.
//! - [`History::changes`] tells a materializer which commits were delivered or
//!   changed participation, without reading payloads.
mod admission;
mod author;
mod commit_hash;
mod decode;
mod error;
mod evidence;
mod history;
mod meta;
mod reader;
mod signature;
mod verified;
mod verify;
#[cfg(feature = "fuzzing")]
pub mod wire;
#[cfg(not(feature = "fuzzing"))]
pub(crate) mod wire;
mod write_permission;

pub use admission::{
    Admission, Counts, DecodeStatus, Next, ReceiveOptions, Reply, ReplyError, Request, Ticket,
};
pub use author::{Author, InvalidAuthor};
pub use commit_hash::{CommitHash, InvalidCommitHash};
pub use decode::{DecodeItem, DecodeNext};
pub use error::Error;
pub use history::{
    Batch, Change, Changes, CommitBuilder, CommitInfo, Delivered, ExportCursor, ExportItem,
    ExportPlan, Heads, History, MaterializationCursor, PayloadCheck, Prepared, Receipt, ViewPoint,
    Vis, WritePermissions,
};
#[cfg(feature = "fuzzing")]
pub use meta::Evidence;
pub use meta::{
    BundleMeta, FormatId, FragmentRange, ItemId, Limits, MemberShape, ParentRef, PayloadId,
};
pub use signature::Signature;
pub use wire::header_len;
pub use write_permission::WritePermission;
