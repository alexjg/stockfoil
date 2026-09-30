//! Application payload representation and encoded transport API.
use crate::*;
use std::{collections::BTreeSet, fmt};

/// Both encodings are required. Canonical bytes must remain immutable for the
/// lifetime of retained payloads (including payloads held through shared handles).
/// Decoders must enforce the supplied cooperative allocation/work budgets: core
/// cannot sandbox arbitrary application code or undo its external side effects.
pub trait PayloadCodec {
    type Payload: Clone;
    type Error: std::error::Error + 'static;
    /// Trusted application format/version identifier (1..128 UTF-8 bytes).
    /// Incoming identifiers are compared, never used to select code.
    fn format_id(&self) -> &str;
    /// Deterministic canonical bytes: these, not fragment packing, bind identity.
    fn encode_commit(&self, payload: &Self::Payload) -> Result<Vec<u8>, Self::Error>;
    /// Decode hostile input within the cooperative limits. Core requires an
    /// exact canonical re-encoding before authenticating the result.
    fn decode_commit(
        &self,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Self::Payload, Self::Error>;
    /// Compress the entire aligned batch; there is deliberately no default.
    /// Each reconstructed payload must retain its original canonical bytes.
    fn encode_fragment(
        &self,
        shape: &FragmentShape,
        payloads: &[&Self::Payload],
    ) -> Result<Vec<u8>, Self::Error>;
    /// Decode exactly one payload per member without boundary-history payloads.
    /// Shape and body are structurally bounded but not yet authenticated.
    fn decode_fragment(
        &self,
        shape: &FragmentShape,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<Self::Payload>, Self::Error>;
}
/// Ancestors-first logical structure supplied to application batch codecs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentShape {
    pub members: Vec<MemberShape>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberShape {
    pub author: Author,
    /// Authenticated order and multiplicity must not be sorted or deduplicated.
    pub parents: Vec<ParentRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentRef {
    /// Index strictly earlier than the current member.
    Member(usize),
    /// Outside membership; not necessarily a declared range boundary.
    External(CommitHash),
}

/// Per-item budgets for untrusted inbound ingestion only, not local commits or
/// exports, and not aggregate graph or pending-queue limits. A growing certified
/// fragment may require the receiver to explicitly increase these limits before
/// retrying it; no authenticated chunking or fallback is promised. Application
/// code must cooperate: its arbitrary allocations cannot be sandboxed by core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_item_bytes: usize,
    pub max_members: usize,
    pub max_parents: usize,
    pub max_signatures: usize,
    pub max_payload_bytes: usize,
}
impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_item_bytes: 8 * 1024 * 1024,
            max_members: 65536,
            max_parents: 1_000_000,
            max_signatures: 65536,
            max_payload_bytes: 8 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Draft {
    pub hash: CommitHash,
    pub signing_bytes: [u8; 32],
    pub report: IngestReport,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportMetadata {
    Commit {
        hash: CommitHash,
        parents: BTreeSet<CommitHash>,
    },
    Fragment {
        range: FragmentRange,
        checkpoints: BTreeSet<Checkpoint>,
    },
}
impl TransportMetadata {
    pub fn id(&self) -> ItemId {
        match self {
            Self::Commit { hash, .. } => ItemId::Commit(*hash),
            Self::Fragment { range, .. } => ItemId::Fragment(range.clone()),
        }
    }
}
/// An authenticated export's opaque wire bytes and derived transport metadata.
/// Produced by [`CommitGraph::export_changes`]; feed `bytes` to [`CommitGraph::ingest`].
#[derive(Clone, Debug)]
pub struct EncodedItem {
    pub metadata: TransportMetadata,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ExportChanges {
    pub added: Vec<EncodedItem>,
    pub retired: Vec<ItemId>,
}

/// A sequential merge failed after zero or more successful item ingests.
/// Earlier admissions and deferred rejections remain observable in `report`;
/// its pending count reflects the receiver at the time of failure.
#[derive(Debug)]
pub struct MergeError<E> {
    pub report: IngestReport,
    pub error: Error<E>,
}
impl<E: fmt::Display> fmt::Display for MergeError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "merge failed: {}", self.error)
    }
}
impl<E: std::error::Error + 'static> std::error::Error for MergeError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug)]
pub enum Error<E> {
    Codec(E),
    InvalidEncoding(&'static str),
    WrongCodec,
    NonCanonicalPayload,
    PayloadHashMismatch,
    LimitExceeded(&'static str),
    Verification(VerificationError),
    Ingest(IngestError),
}
impl<E> Error<E> {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Codec(_) => "CodecError",
            Self::InvalidEncoding(_) => "InvalidEncoding",
            Self::WrongCodec => "WrongCodec",
            Self::NonCanonicalPayload => "NonCanonicalPayload",
            Self::PayloadHashMismatch => "PayloadHashMismatch",
            Self::LimitExceeded(_) => "LimitExceeded",
            Self::Verification(VerificationError::InvalidItem) => "InvalidItem",
            Self::Verification(VerificationError::MissingSignature { .. }) => "MissingSignature",
            Self::Verification(VerificationError::InvalidSignature(
                SignatureError::InvalidPublicKey(_),
            )) => "InvalidPublicKey",
            Self::Verification(VerificationError::InvalidSignature(_)) => "InvalidSignature",
            Self::Ingest(_) => "InvalidFragment",
        }
    }
}
impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())?;
        match self {
            Self::Codec(e) => write!(f, ": {e}"),
            Self::InvalidEncoding(e) | Self::LimitExceeded(e) => write!(f, ": {e}"),
            Self::Verification(e) => write!(f, ": {e}"),
            Self::Ingest(e) => write!(f, ": {e}"),
            _ => Ok(()),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(e) => Some(e),
            Self::Verification(e) => Some(e),
            Self::Ingest(e) => Some(e),
            _ => None,
        }
    }
}
