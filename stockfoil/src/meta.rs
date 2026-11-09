//! Structural metadata shared by the wire format, verification, history, and codecs.
use crate::{Author, CommitHash, Error, Signature};

pub use crate::verified::history_data::{FragmentRange, ItemId, PayloadId};

impl ItemId {
    pub fn head(&self) -> CommitHash {
        match self {
            Self::Commit(hash) => *hash,
            Self::Fragment(range) => range.head,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberShape {
    pub author: Author,
    /// Order and multiplicity are part of the commit identity.
    pub parents: Vec<ParentRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentRef {
    /// An earlier member of the same bundle.
    Member(usize),
    /// A commit outside the bundle.
    External(CommitHash),
}

/// A signature by `author` over the hash of member `member`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(feature = "fuzzing")]
pub struct Evidence {
    pub author: Author,
    pub member: usize,
    pub signature: Signature,
}

/// A signature by `author` over the hash of member `member`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(not(feature = "fuzzing"))]
pub(crate) struct Evidence {
    pub author: Author,
    pub member: usize,
    pub signature: Signature,
}

/// Structural bundle metadata. Untrusted until its bundle has been admitted;
/// a decoder receives it before any authentication has happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleMeta {
    pub id: ItemId,
    /// Ancestors-first member structure.
    pub members: Vec<MemberShape>,
    #[cfg(feature = "fuzzing")]
    pub evidence: Vec<Evidence>,
    #[cfg(not(feature = "fuzzing"))]
    pub(crate) evidence: Vec<Evidence>,
}

impl BundleMeta {
    pub fn len(&self) -> usize {
        self.members.len()
    }
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
    pub fn head(&self) -> CommitHash {
        self.id.head()
    }
}

/// A trusted application format identifier: 1–128 UTF-8 bytes, compared exactly.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormatId(Box<str>);

impl FormatId {
    pub fn new(id: &str) -> Result<Self, Error> {
        if id.is_empty() || id.len() > 128 {
            return Err(Error::Limit("format ID must have 1..=128 bytes"));
        }
        Ok(Self(id.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-item structural limits for untrusted input. They are hard errors.
///
/// Metadata memory grows with members and parents. The byte limits default to
/// unbounded so that payloads of any size can stream; tighten them to bound the
/// hashing work a single item can demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_header_bytes: usize,
    pub max_members: usize,
    pub max_parents: usize,
    pub max_signatures: usize,
    pub max_boundaries: usize,
    /// Total canonical bytes across all members.
    pub max_canonical_bytes: u64,
    /// Total stored item bytes, header included.
    pub max_item_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header_bytes: 64 << 20,
            // Depth-2 fragments average 65,536 members; leave room for large ones.
            max_members: 1 << 20,
            max_parents: 4 << 20,
            max_signatures: 1 << 20,
            max_boundaries: 65_536,
            max_canonical_bytes: u64::MAX,
            max_item_bytes: u64::MAX,
        }
    }
}

pub(crate) use crate::verified::publication::depth;

/// Incremental commit identity:
/// `BLAKE3(J(parent count) || parents || author || BLAKE3(payload))`.
pub(crate) struct CommitHasher(blake3::Hasher);

impl CommitHasher {
    pub fn new(parent_count: usize) -> Self {
        let mut hasher = blake3::Hasher::new();
        let mut count = Vec::with_capacity(10);
        bijoux::u64::encode(parent_count as u64, &mut count);
        hasher.update(&count);
        Self(hasher)
    }
    pub fn parent(&mut self, parent: &CommitHash) {
        self.0.update(parent.as_bytes());
    }
    pub fn finish(mut self, author: &Author, payload: &[u8; 32]) -> CommitHash {
        self.0.update(author.as_slice());
        self.0.update(payload);
        CommitHash::from(self.0.finalize())
    }
}

#[cfg(test)]
pub(crate) fn commit_hash<'a>(
    parents: impl ExactSizeIterator<Item = &'a CommitHash>,
    author: &Author,
    payload: &[u8; 32],
) -> CommitHash {
    let mut hasher = CommitHasher::new(parents.len());
    for parent in parents {
        hasher.parent(parent);
    }
    hasher.finish(author, payload)
}
