//! Durable materialization progress. No payload IDs, node indices, or I/O.
#[cfg(test)]
use super::view::{MaterializationCursor, ViewPoint, encode_hashes};
use crate::CommitHash;
#[cfg(test)]
use crate::Error;
#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
pub(crate) use crate::verified::checkpoint::InProgress;
pub use crate::verified::checkpoint::{Checkpoint, Follow};

impl Checkpoint {
    /// Rejected commits and whether each taints its descendants, in hash order.
    pub fn rejected(&self) -> impl ExactSizeIterator<Item = (CommitHash, bool)> + '_ {
        self.rejected
            .iter()
            .map(|(&hash, &cascade)| (hash, cascade))
    }
}

#[cfg(test)]
mod tests;
