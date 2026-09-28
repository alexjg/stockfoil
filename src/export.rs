//! Incremental publication of the certified export inventory.
//!
//! API scaffold: cursor construction and change computation are not implemented yet.

use crate::{ItemId, UnverifiedItem};

/// The `ExportCursor` is used to track changes to the sedimentree encoding of
/// a commit graph when calling [`crate::CommitGraph::export_changes`]
#[derive(Clone, Debug)]
pub struct ExportCursor {
    // Representation intentionally left unspecified until implementation.
    _private: (),
}

impl Default for ExportCursor {
    /// Start with an empty published inventory.
    fn default() -> Self {
        todo!("construct an empty export cursor")
    }
}

/// Changes from a previously call to [`crate::CommitGraph::export_changes`] 
#[derive(Debug)]
pub struct ExportChanges {
    pub added: Vec<UnverifiedItem>,
    pub retired: Vec<ItemId>,
}
