use std::fmt;

pub use crate::verified::drive_error::DriveError;

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
