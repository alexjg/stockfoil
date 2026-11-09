use std::fmt;

pub use crate::verified::api_error::Error;

impl Error {
    /// A stable machine-readable name for the variant, for bindings and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "Malformed",
            Self::Limit(_) => "Limit",
            Self::WrongFormat => "WrongFormat",
            Self::InvalidItem(_) => "InvalidItem",
            Self::InvalidPublicKey(_) => "InvalidPublicKey",
            Self::InvalidSignature { .. } => "InvalidSignature",
            Self::MissingSignature { .. } => "MissingSignature",
            Self::CorruptPayload(_) => "CorruptPayload",
            Self::InvalidPlacement => "InvalidPlacement",
            Self::MissingDependency(_) => "MissingDependency",
            Self::UnknownCommit(_) => "UnknownCommit",
            Self::MissingParent(_) => "MissingParent",
            Self::NotDraft(_) => "NotDraft",
            Self::WrongHistory => "WrongHistory",
            Self::StaleBatch => "StaleBatch",
            Self::Stale => "Stale",
            Self::Poisoned => "Poisoned",
            Self::Codec(_) => "Codec",
            Self::Storage(_) => "Storage",
            Self::Protocol(_) => "Protocol",
            Self::Exhausted(_) => "Exhausted",
            Self::OutOfRange(_) => "OutOfRange",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "malformed item: {what}"),
            Self::Limit(what) => write!(f, "limit exceeded: {what}"),
            Self::WrongFormat => write!(f, "unexpected application format"),
            Self::InvalidItem(what) => write!(f, "invalid item: {what}"),
            Self::InvalidPublicKey(_) => write!(f, "invalid author public key"),
            Self::InvalidSignature { member, .. } => {
                write!(f, "invalid signature on member {member}")
            }
            Self::MissingSignature { member, .. } => {
                write!(
                    f,
                    "member {member} is not covered by its author's signature"
                )
            }
            Self::CorruptPayload(_) => write!(f, "stored payload does not match its commit"),
            Self::InvalidPlacement => write!(f, "fragment is not canonically placed"),
            Self::MissingDependency(_) => write!(f, "missing admitted dependency"),
            Self::UnknownCommit(_) => write!(f, "unknown commit"),
            Self::MissingParent(_) => write!(f, "parent has not been delivered"),
            Self::NotDraft(_) => write!(f, "commit is not an unadmitted local draft"),
            Self::WrongHistory => write!(f, "used with a different history"),
            Self::StaleBatch => write!(f, "batch is not outstanding on this materialization"),
            Self::Stale => write!(f, "materialization is not current"),
            Self::Poisoned => write!(f, "application state must be restored or rebuilt"),
            Self::Codec(e) => write!(f, "codec failed: {e}"),
            Self::Storage(e) => write!(f, "storage failed: {e}"),
            Self::Protocol(what) => write!(f, "host protocol violation: {what}"),
            Self::Exhausted(what) => write!(f, "{what} exhausted"),
            Self::OutOfRange(what) => write!(f, "no such {what}"),
        }
    }
}

impl std::error::Error for Error {}
