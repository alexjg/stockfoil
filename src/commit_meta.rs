use alloc::{string::String, vec::Vec};

use crate::{Author, BlobHash, CommitHash, parsing::ParseError};

#[derive(Debug, Clone)]
pub struct CommitMeta {
    hash: CommitHash,
    parents: Vec<CommitHash>,
    author: Author,
    payload_hash: BlobHash,
}

impl CommitMeta {
    pub fn new(contents: CommitContents) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&contents.encode());
        let hash = CommitHash::from(<[u8; 32]>::from(hasher.finalize()));
        Self {
            hash,
            parents: contents.parents,
            author: contents.author,
            payload_hash: contents.payload,
        }
    }

    pub fn hash(&self) -> &CommitHash {
        &self.hash
    }

    pub fn parents(&self) -> &[CommitHash] {
        &self.parents
    }

    /// Hash of the canonical payload bytes. Applications must check received
    /// payloads against this before attaching them to authenticated metadata.
    pub fn payload_hash(&self) -> &BlobHash {
        &self.payload_hash
    }

    /// The claimed author. This alone is not proof of authorship.
    pub fn author(&self) -> &Author {
        &self.author
    }
}

#[derive(Debug, Clone)]
pub struct CommitContents {
    pub parents: Vec<CommitHash>,
    pub author: Author,
    pub payload: BlobHash,
}

impl CommitContents {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bijoux::u64::encode(self.parents.len() as u64, &mut bytes);
        for parent in &self.parents {
            bytes.extend_from_slice(parent.as_bytes());
        }
        bytes.extend_from_slice(self.author.as_slice());
        bytes.extend_from_slice(self.payload.as_slice());
        bytes
    }

    fn decode(data: &[u8]) -> Result<Self, ParseError> {
        let mut input = crate::parsing::Input::new(data);
        let num_parents = usize::try_from(input.u64()?)
            .map_err(|_| ParseError::InvalidParentCount)?;
        if num_parents > data.len() / 32 {
            return Err(ParseError::InvalidParentCount);
        }
        let mut parents = Vec::with_capacity(num_parents);
        for _ in 0..num_parents {
            parents.push(CommitHash::from(input.array::<32>()?));
        }
        let author_bytes = input.bytes(32)?;
        let author = Author::try_from(author_bytes).map_err(|_| ParseError::InvalidAuthor)?;
        let payload_bytes = input.array::<32>()?;
        let payload_hash = BlobHash::from(payload_bytes);
        input.finish()?;
        Ok(Self {
            parents,
            author,
            payload: payload_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_envelope_round_trips_and_binds_the_author() {
        let author = Author::try_from([1u8; 32].as_slice()).unwrap();
        let payload = BlobHash::of_contents(b"payload");
        let contents = CommitContents {
            parents: Vec::new(),
            author,
            payload,
        };
        let encoded = contents.encode();
        // One parent-count byte, mandatory author key, payload hash.
        // Signatures are detached from the envelope.
        assert_eq!(encoded.len(), 1 + 32 + 32);
        let decoded = CommitContents::decode(&encoded).unwrap();
        assert_eq!(decoded.encode(), encoded);
        let mut trailing_bytes = encoded.clone();
        trailing_bytes.push(0);
        assert!(CommitContents::decode(&trailing_bytes).is_err());
        for len in 0..encoded.len() {
            assert!(CommitContents::decode(&encoded[..len]).is_err());
        }
        let mut oversized_count = Vec::new();
        bijoux::u64::encode(u64::MAX, &mut oversized_count);
        assert!(CommitContents::decode(&oversized_count).is_err());
        let meta = CommitMeta::new(contents);
        assert_eq!(meta.author(), &author);
        assert!(meta.payload_hash == payload);
        assert_eq!(CommitMeta::new(decoded).hash(), meta.hash());

        let another_author = CommitMeta::new(CommitContents {
            parents: Vec::new(),
            author: Author::try_from([2u8; 32].as_slice()).unwrap(),
            payload,
        });
        assert_ne!(another_author.hash(), meta.hash());
    }
}

#[derive(Debug)]
pub struct ParseCommitError(String);

impl core::error::Error for ParseCommitError {}
impl core::fmt::Display for ParseCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ParseCommitError: {}", self.0)
    }
}
