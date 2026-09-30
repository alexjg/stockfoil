//! Incremental publication of the certified export inventory.

use std::collections::BTreeSet;

use crate::{ItemId, UnverifiedItem};

/// The `ExportCursor` is used to track changes to the sedimentree encoding of
/// a commit graph when calling [`crate::CommitGraph::export_changes`]
#[derive(Clone, Debug, Default)]
pub struct ExportCursor {
    pub(crate) inventory: BTreeSet<ItemId>,
}

/// Structured inventory changes, encoded before crossing the public API.
#[derive(Debug)]
pub(crate) struct ExportChanges {
    pub added: Vec<UnverifiedItem>,
    pub retired: Vec<ItemId>,
}
