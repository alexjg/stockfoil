//! Local commit creation: validate parents, hash canonical bytes, publish a draft.
use super::History;
use crate::meta::commit_hash;
use crate::{Author, BlobHash, CommitHash, Error};

/// Hashes one local commit's canonical payload. Created by
/// [`History::begin_commit`] after its parents have been checked, so invalid
/// parents are reported before any payload is read.
#[derive(Debug, Clone)]
pub struct CommitBuilder {
    author: Author,
    parents: Vec<CommitHash>,
    payload: blake3::Hasher,
    len: u64,
}

impl CommitBuilder {
    /// Append canonical payload bytes. The caller chooses the chunking.
    pub fn update(&mut self, canonical: &[u8]) {
        self.payload.update(canonical);
        self.len += canonical.len() as u64;
    }

    pub fn finish(self) -> Prepared {
        let payload_hash = BlobHash::from(<[u8; 32]>::from(self.payload.finalize()));
        Prepared {
            hash: commit_hash(self.parents.iter(), &self.author, &payload_hash),
            author: self.author,
            parents: self.parents,
            payload_hash,
            len: self.len,
        }
    }
}

/// A local commit's identity, ready to sign and publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub hash: CommitHash,
    pub author: Author,
    /// Ordered parents, repeats preserved.
    pub parents: Vec<CommitHash>,
    pub payload_hash: BlobHash,
    /// Canonical payload length in bytes.
    pub len: u64,
}

impl Prepared {
    /// The bytes the author signs with Ed25519: the raw commit hash.
    pub fn signing_bytes(&self) -> [u8; 32] {
        *self.hash.as_bytes()
    }
}

impl History {
    /// Start a local commit. Every parent must already be delivered (drafts
    /// included). Parent order and repeats are preserved in the identity.
    pub fn begin_commit(
        &self,
        author: Author,
        parents: Vec<CommitHash>,
    ) -> Result<CommitBuilder, Error> {
        if let Some(missing) = parents.iter().find(|p| !self.by_hash.contains_key(*p)) {
            return Err(Error::MissingParent(*missing));
        }
        Ok(CommitBuilder {
            author,
            parents,
            payload: blake3::Hasher::new(),
            len: 0,
        })
    }
}
