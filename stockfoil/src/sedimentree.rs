//! Convert Stockfoil exports into Sedimentree metadata.
//!
//! Available with the **`sedimentree`** feature. This module only describes
//! exports: it does not encode or sign blobs, perform I/O, manage cursors, or
//! receive items. Storage, encryption, and synchronization belong to the caller.
//!
//! ```
//! use sedimentree_core::{blob::Blob, id::SedimentreeId};
//! use stockfoil::{Codec, Document, ExportCursor};
//! use stockfoil::sedimentree::{self, Metadata};
//!
//! fn store_exports<C: Codec>(
//!     document: &Document<C>,
//!     tree: SedimentreeId,
//!     cursor: &mut ExportCursor,
//!     mut store: impl FnMut(Metadata, Blob) -> Result<(), Box<dyn std::error::Error>>,
//! ) -> Result<(), Box<dyn std::error::Error>> {
//!     let mut plan = document.ledger().export(cursor);
//!     for index in 0..plan.len() {
//!         let (item, bytes) = document.export_item(&plan, index)?;
//!         let blob = Blob::new(bytes); // Encrypt first if required.
//!         let metadata = sedimentree::describe(tree, &item, blob.meta())?;
//!         store(metadata, blob)?;
//!         plan.acknowledge(index)?;
//!         *cursor = plan.cursor();
//!     }
//!     Ok(())
//! }
//! ```
//!
//! The same conversion works with [`crate::Ledger::export_item`] when payloads
//! live outside memory: encode the item into caller-owned storage, compute its
//! [`BlobMeta`], and pass that metadata to [`describe`]. No blob bytes are needed
//! by the conversion itself.

use crate::{CommitHash, ExportedItem, ItemId, ParentRef};
use sedimentree_core::{
    blob::BlobMeta,
    fragment::{Fragment, InvalidFragment},
    id::SedimentreeId,
    loose_commit::{InvalidLooseCommit, LooseCommit, id::CommitId},
};
use std::collections::BTreeSet;
use std::fmt;

/// Sedimentree metadata describing one exported Stockfoil item.
///
/// Contains neither the blob nor a Subduction signature. Both variants are
/// needed: a standalone commit is not a fragment, even if its hash has nonzero
/// depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metadata {
    Commit(LooseCommit),
    Fragment(Fragment),
}

/// Translate an exported item into Sedimentree metadata.
///
/// Standalone commits use their head and distinct parents. Fragments use their
/// head, boundary set, and exported checkpoints (nonzero-depth members other
/// than the head). Commit hashes are copied byte-for-byte into Sedimentree IDs.
///
/// `item` should be an unmodified export from a [`crate::Ledger`] or
/// [`crate::Document`]. `blob` must describe the exact bytes the application will
/// store, including any encryption or additional framing. This function does
/// not inspect those bytes or authenticate the export.
///
/// # Errors
///
/// Returns [`Error::InvalidExport`] for inconsistent structural fields, or a
/// Sedimentree construction error if its parent, boundary, or checkpoint limits
/// are exceeded. Counts are never truncated and no fallback representation is
/// substituted.
pub fn describe(
    tree: SedimentreeId,
    item: &ExportedItem,
    blob: BlobMeta,
) -> Result<Metadata, Error> {
    if item.id != item.meta.id {
        return Err(Error::InvalidExport("item ID differs from bundle metadata"));
    }
    if item.members.is_empty() || item.members.len() != item.meta.members.len() {
        return Err(Error::InvalidExport("inconsistent member count"));
    }
    match &item.id {
        ItemId::Commit(hash) => {
            if item.members.len() != 1 || item.members[0].0 != *hash {
                return Err(Error::InvalidExport(
                    "standalone commit must contain its head only",
                ));
            }
            if !item.checkpoints.is_empty() {
                return Err(Error::InvalidExport("standalone commit has checkpoints"));
            }
            let parents = item.meta.members[0]
                .parents
                .iter()
                .map(|parent| match parent {
                    ParentRef::External(hash) => Ok(commit_id(hash)),
                    ParentRef::Member(_) => Err(Error::InvalidExport(
                        "standalone commit has an internal parent",
                    )),
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            LooseCommit::new(tree, commit_id(hash), parents, blob)
                .map(Metadata::Commit)
                .map_err(Error::Commit)
        }
        ItemId::Fragment(range) => {
            if !item.members.iter().any(|(hash, _)| *hash == range.head) {
                return Err(Error::InvalidExport("fragment does not contain its head"));
            }
            Fragment::new(
                tree,
                commit_id(&range.head),
                range.boundary.iter().map(commit_id).collect(),
                &item.checkpoints.iter().map(commit_id).collect::<Vec<_>>(),
                blob,
            )
            .map(Metadata::Fragment)
            .map_err(Error::Fragment)
        }
    }
}

fn commit_id(hash: &CommitHash) -> CommitId {
    CommitId::new(*hash.as_bytes())
}

/// Failure to represent an export as Sedimentree metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The supplied export has inconsistent structural fields.
    InvalidExport(&'static str),
    /// Sedimentree cannot represent the standalone commit's parent set.
    Commit(InvalidLooseCommit),
    /// Sedimentree cannot represent the fragment's boundaries or checkpoints.
    Fragment(InvalidFragment),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExport(reason) => write!(f, "invalid Sedimentree export: {reason}"),
            Self::Commit(error) => error.fmt(f),
            Self::Fragment(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidExport(_) => None,
            Self::Commit(error) => Some(error),
            Self::Fragment(error) => Some(error),
        }
    }
}
