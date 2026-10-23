//! Local commit creation (validate parents, hash canonical bytes, publish a
//! draft) and checking payloads read back from storage.
use crate::meta::{CommitHasher, commit_hash};
use crate::{Author, CommitHash, Error};

/// Hashes one local commit's canonical payload. Created by
/// [`History::begin_commit`](crate::History::begin_commit) after its parents
/// have been checked, so invalid parents are reported before any payload is read.
#[derive(Debug, Clone)]
pub struct CommitBuilder {
    author: Author,
    parents: Vec<CommitHash>,
    payload: blake3::Hasher,
}

impl CommitBuilder {
    pub(super) fn new(author: Author, parents: Vec<CommitHash>) -> Self {
        Self {
            author,
            parents,
            payload: blake3::Hasher::new(),
        }
    }

    /// Append canonical payload bytes. The caller chooses the chunking.
    pub fn update(&mut self, canonical: &[u8]) {
        self.payload.update(canonical);
    }

    pub fn finish(self) -> Prepared {
        let payload_hash: [u8; 32] = self.payload.finalize().into();
        Prepared {
            hash: commit_hash(self.parents.iter(), &self.author, &payload_hash),
            author: self.author,
            parents: self.parents,
        }
    }
}

/// A local commit's identity, ready to sign and publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub hash: CommitHash,
    pub(crate) author: Author,
    pub(crate) parents: Vec<CommitHash>,
}

impl Prepared {
    /// The bytes the author signs with Ed25519: the raw commit hash.
    pub fn signing_bytes(&self) -> [u8; 32] {
        *self.hash.as_bytes()
    }
}

/// Checks that canonical bytes read back from storage are a delivered
/// commit's payload. Created by [`History::check_payload`](crate::History::check_payload).
pub struct PayloadCheck {
    commit: CommitHash,
    author: Author,
    hasher: CommitHasher,
    payload: blake3::Hasher,
}

impl PayloadCheck {
    pub(super) fn new(
        commit: CommitHash,
        author: Author,
        parents: impl ExactSizeIterator<Item = CommitHash>,
    ) -> Self {
        let mut hasher = CommitHasher::new(parents.len());
        for parent in parents {
            hasher.parent(&parent);
        }
        Self {
            commit,
            author,
            hasher,
            payload: blake3::Hasher::new(),
        }
    }

    pub(crate) fn commit(&self) -> CommitHash {
        self.commit
    }

    /// Append canonical payload bytes. The caller chooses the chunking.
    pub fn update(&mut self, canonical: &[u8]) {
        self.payload.update(canonical);
    }

    /// [`Error::CorruptPayload`] unless the bytes reproduce the commit hash.
    pub fn finish(self) -> Result<(), Error> {
        let payload: [u8; 32] = self.payload.finalize().into();
        if self.hasher.finish(&self.author, &payload) == self.commit {
            Ok(())
        } else {
            Err(Error::CorruptPayload(self.commit))
        }
    }
}
