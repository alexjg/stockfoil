use crate::Error;
use std::convert::Infallible;
use std::fmt;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    fn from(error: Error) -> Self {
        Self::Stockfoil(error)
    }
}

impl<M> DriveError<M> {
    /// Lift an update/signing failure into an operation that can also edit.
    pub(crate) fn with_edit_error<E>(self) -> DriveError<M, E> {
        match self {
            Self::Stockfoil(error) => DriveError::Stockfoil(error),
            Self::Apply(error) => DriveError::Apply(error),
            Self::Checkpoint(error) => DriveError::Checkpoint(error),
            Self::Edit(never) => match never {},
        }
    }
}

impl<M: fmt::Display, E: fmt::Display> fmt::Display for DriveError<M, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stockfoil(error) => error.fmt(f),
            Self::Apply(error) => write!(f, "applying materializer batch failed: {error}"),
            Self::Checkpoint(error) => {
                write!(f, "checkpointing materializer state failed: {error}")
            }
            Self::Edit(error) => write!(f, "editing materializer state failed: {error}"),
        }
    }
}

impl<M, E> std::error::Error for DriveError<M, E>
where
    M: std::error::Error + 'static,
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Stockfoil(error) => error,
            Self::Apply(error) | Self::Checkpoint(error) => error,
            Self::Edit(error) => error,
        })
    }
}
