//! The actual error type preserves application errors across driving phases.
use super::api_error::Error;
use std::convert::Infallible;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {
/// A failure while driving in-memory application state.
///
/// Unlike [`Error`], this preserves application errors as their original types.
/// `M` is [`Materializer::Error`](crate::Materializer::Error); `E` is an edit
/// closure's error, and defaults to [`Infallible`] for operations without an
/// edit. Match the variants to distinguish applying, checkpointing and editing.
/// Standard error source chains are available when both application error types
/// implement [`std::error::Error`]; string errors still support matching and
/// display without that bound.
///
/// This is not a rollback guarantee. Apply, checkpoint and edit failures can
/// leave application state mutated and poison its driver; restore state and
/// checkpoint together or rebuild before continuing. Preflight failures leave
/// untouched state usable. A Stockfoil failure after an edit may also poison
/// the driver, and a replica operation can record a commit before updating its
/// state fails. Inspect the document before deciding whether to repeat a write.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
#[non_exhaustive]
pub enum DriveError<M, E = Infallible> {
    /// A history, protocol, codec or other Stockfoil failure.
    Stockfoil(Error),
    /// Applying a batch to application state failed.
    Apply(M),
    /// Saving application state together with its checkpoint failed.
    Checkpoint(M),
    /// The local edit closure failed, potentially after mutating state.
    Edit(E),
}

impl<M, E> From<Error> for DriveError<M, E> {
    fn from(error: Error) -> (out: Self) ensures out == Self::Stockfoil(error), {
        Self::Stockfoil(error)
    }
}

#[cfg(verus_keep_ghost)]
impl<M,E> vstd::std_specs::convert::FromSpecImpl<Error> for DriveError<M,E> {
    open spec fn obeys_from_spec() -> bool { true }
    open spec fn from_spec(error:Error) -> Self { Self::Stockfoil(error) }
}


} // verus!
