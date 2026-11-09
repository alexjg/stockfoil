//! Local commit creation (validate parents, hash canonical bytes, publish a
//! draft) and checking payloads read back from storage.
use crate::meta::CommitHasher;
use crate::{Author, CommitHash, Error};

pub use crate::verified::commit_builder::CommitBuilder;

pub use crate::verified::publication::Prepared;

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
