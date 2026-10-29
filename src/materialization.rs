use alloc::vec::Vec;
use crate::collections::HashMap;

use crate::{CommitHash, CommitMeta, ViewPoint, Vis};

/// The delivery history and viewpoint already processed by a materializer.
///
/// Start with [`Self::new`] and advance using [`crate::CommitGraph::visit_changes`].
/// Keep the returned cursor together with the corresponding materializer state;
/// reusing an older cursor replays its changes. Cloning a cursor does not clone
/// that state.
///
/// Cursors own hashes and permissions, not graph-local indices. They can be used
/// on another graph containing their delivered history, even if its insertion
/// order differs. Pending local history must also be present on that graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationCursor {
    pub(crate) delivery_heads: Vec<CommitHash>,
    pub(crate) view_point: ViewPoint,
}

impl MaterializationCursor {
    /// Start with no processed commits and an empty participating history.
    pub fn new() -> Self {
        Self {
            delivery_heads: Vec::new(),
            view_point: ViewPoint::from_parts(Vec::new(), HashMap::new()),
        }
    }

    /// The heads of all delivered history processed by the last traversal,
    /// including commits outside the materialized viewpoint or denied permission.
    pub fn delivery_heads(&self) -> &[CommitHash] {
        &self.delivery_heads
    }

    /// The viewpoint whose participation the materializer last processed.
    pub fn view_point(&self) -> &ViewPoint {
        &self.view_point
    }
}

impl Default for MaterializationCursor {
    fn default() -> Self {
        Self::new()
    }
}

/// A newly delivered commit, a participation change, or both.
///
/// Metadata and payload are borrowed from the graph, never cloned by traversal.
/// This is not a document patch: the materializer determines effects such as an
/// older value reappearing when an overwriting commit stops participating.
#[derive(Debug)]
pub struct MaterializationChange<'graph, P> {
    pub meta: &'graph CommitMeta,
    pub payload: &'graph P,
    /// Whether this commit is outside the cursor's previously processed delivery
    /// history. Store its data even if it does not currently participate.
    pub newly_delivered: bool,
    /// Participation under the cursor's viewpoint and the requested viewpoint.
    /// A newly delivered but non-participating commit has [`Vis::Neither`].
    pub visibility: Vis,
}
