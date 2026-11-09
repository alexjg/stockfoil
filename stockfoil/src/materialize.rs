//! State-independent materialization planning and its in-memory driver.
use crate::CommitHash;

pub use crate::verified::materialization_driver::Materialization;

pub use crate::verified::batch_protocol::{Batch, Event};
pub use crate::verified::materialization_feedback::{Applied, Rejection};
impl Applied {
    /// Mark a commit newly delivered by the pending source batch as permanently
    /// rejected (without tainting its descendants), including during follow-up rounds.
    pub fn reject(mut self, hash: CommitHash, reason: impl Into<String>) -> Self {
        self.rejected.push(Rejection {
            hash,
            reason: reason.into(),
            cascade: false,
        });
        self
    }
}
pub use crate::verified::materializer::{Materializer, Step};
pub use crate::verified::progress::Progress;
