//! Incremental publication of the certified export inventory.

use std::collections::BTreeSet;

use crate::{ItemId, UnverifiedItem};

/// The `ExportCursor` is used to track changes to the sedimentree encoding of
/// a commit graph when calling [`crate::CommitGraph::export_changes`]
#[derive(Clone, Debug, Default)]
pub struct ExportCursor {
    pub(crate) inventory: BTreeSet<ItemId>,
}

/// Changes from a previous call to [`crate::CommitGraph::export_changes`]
#[derive(Debug)]
pub struct ExportChanges {
    pub added: Vec<UnverifiedItem>,
    pub retired: Vec<ItemId>,
}
