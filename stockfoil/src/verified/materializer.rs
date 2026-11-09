//! The actual application interface, with no assumed application behavior.
use super::{
    batch_protocol::Event, checkpoint::Checkpoint, history::History,
    materialization_feedback::Applied,
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {
/// In-memory application state driven by [`Replica`](crate::Replica) or
/// [`Materialization::update`](crate::Materialization::update) / [`Materialization::step`](crate::Materialization::step).
///
/// Applications with out-of-core or async storage consume [`Batch`](crate::Batch)es from
/// [`Materialization::plan`](crate::Materialization::plan) directly instead of implementing this trait.
pub trait Materializer {
    /// Application error type, returned unchanged inside [`DriveError::Apply`](crate::DriveError::Apply)
    /// or [`DriveError::Checkpoint`](crate::DriveError::Checkpoint).
    #[cfg(not(verus_keep_ghost))]
    type Error: std::fmt::Display;
    #[cfg(verus_keep_ghost)]
    type Error;
    /// Apply a sequence of materialization events to the state.
    fn apply(
        &mut self,
        history: &History,
        events: Vec<Event<Arc<[u8]>>>,
    ) -> Result<Applied, Self::Error>;
    /// Optional hook called with the updated [`Checkpoint`] after a step and
    /// all of its follow-up batches complete.
    fn checkpoint(&mut self, _: &Checkpoint) -> (out: Result<(), Self::Error>)
        default_ensures &*final(self) == &*old(self), out == Ok(()),
    {
        Ok(())
    }
    /// Whether this state can incrementally handle [`Event::Activate`] and
    /// [`Event::Retract`] when a previously delivered commit's visibility
    /// changes.
    ///
    /// Defaults to `false`, which causes [`Materialization`](crate::Materialization) to emit
    /// [`Event::Reset`] and replay active history whenever an already-applied
    /// commit needs to be retracted.
    fn supports_visibility_changes(&self) -> (out: bool) default_ensures !out, {
        false
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    More,
    Current,
}

} // verus!
