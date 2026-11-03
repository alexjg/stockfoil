//! # Stockfoil
//!
//! `stockfoil` is a library for storing and synchronizing authenticated commit
//! graphs.
//!
//! # Received history
//!
//! [`CommitGraph::ingest`] owns verified items and their payloads immediately,
//! queuing them when admitted dependencies are missing. Queued items are not
//! delivered graph history. Admission, including local signing, automatically
//! wakes their dependents. Check [`IngestReport::rejected`] after ingestion,
//! signing, local insertion, and merge for deferred graph-dependent failures;
//! valid dependencies are not rolled back. The queue currently has no limits.
//!
//! # Materialization
//!
//! Use [`CommitGraph::visit_changes`] to build and update a CRDT view. Keep a
//! [`MaterializationCursor`] alongside the materialized state and use one
//! callback for newly delivered payloads and changes in commit participation.
//! This handles cold starts, local edits, merge, signing, and revocation without
//! coordinating separate delivery and diff traversals.
//!
//! Target [`CommitGraph::view_point`] for the working view (including drafts),
//! [`CommitGraph::signed_view_point`] for admitted history, or a captured
//! [`ViewPoint`] for a historical view. For an isolated historical read, copy
//! both the materialized state and its cursor before updating the copy.
//!
//! [`CommitGraph::diff`], [`CommitGraph::traverse`], and
//! [`CommitGraph::traverse_since`] are lower-level tools for specialized queries
//! and graph inspection.
//!
//! # `no_std`
//!
//! Disable the default `std` feature to use this library with `alloc`. Targets
//! must support pointer-width atomics (the graph shares ancestry caches via `Arc`).
//! Use [`collections::HashMap`] for permission maps in feature-independent code.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(all(test, not(feature = "std")))]
extern crate std;

/// Collection types used by the graph's permission-map API.
///
/// With `std` (the default), these are the standard library collections, keeping
/// existing callers compatible. Without it, these are `hashbrown` collections.
/// The no-std default hasher is not intended to resist HashDoS attacks.
pub mod collections {
    #[cfg(not(feature = "std"))]
    pub use hashbrown::{HashMap, HashSet};
    #[cfg(feature = "std")]
    pub use std::collections::{HashMap, HashSet};
}

mod ancestor_clock;
mod verified;
mod author;
pub use author::Author;
mod errors;
pub use errors::{
    DiffError, ExportError, FragmentReadError, IngestError, QueryError,
    SignatureError, VerificationError,
};
mod export;
pub use export::{ExportChanges, ExportCursor};
mod ingest;
pub use ingest::{
    IngestRejection, IngestReport, ItemId, SignatureEvidence, UnverifiedItem, VerifiedItem,
};
mod pending_ingest;
mod blob_hash;
pub use blob_hash::BlobHash;
mod commit_hash;
pub use commit_hash::{CommitHash, InvalidCommitHash};
mod commit_graph;
pub use commit_graph::{CommitGraph};
pub(crate) use verified::clock::NodeIdx;
mod commit_meta;
pub use commit_meta::{CommitContents, CommitMeta};
mod parsing;
mod view_point;
pub use view_point::{CommitDiff, ViewPoint, ViewPointDiff, Vis, VisDiff};
mod materialization;
pub use materialization::{MaterializationChange, MaterializationCursor};
mod signature;
pub use signature::Signature;
mod sedimentree;
pub use sedimentree::{
    Checkpoint, Fragment, FragmentRange, LooseCommit, MinimalCover, SedimentreeSummary,
    SedimentreeView,
};
mod write_permission;
pub use write_permission::WritePermission;
