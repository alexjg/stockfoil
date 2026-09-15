use eyre::{eyre, Context};

use crate::{Author, BlobHash, CommitHash, Signature};

#[derive(Clone)]
pub struct CommitMeta {
    hash: CommitHash,
    parents: Vec<CommitHash>,
    author: Option<Author>,
    signatures: Vec<Signature>,
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
            signatures: contents.signatures,
            payload_hash: contents.payload,
        }
    }

    pub fn hash(&self) -> &CommitHash {
        &self.hash
    }

    pub fn parents(&self) -> &[CommitHash] {
        &self.parents
    }
}

pub struct CommitContents {
    pub parents: Vec<CommitHash>,
    pub author: Option<Author>,
    pub signatures: Vec<Signature>,
    pub payload: BlobHash,
}

impl CommitContents {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bijoux::u64::encode(self.parents.len() as u64, &mut bytes);
        for parent in &self.parents {
            bytes.extend_from_slice(parent.as_bytes());
        }
        if let Some(author) = &self.author {
            bytes.push(1);
            bytes.extend_from_slice(author.as_slice());
        } else {
            bytes.push(0);
        }
        bijoux::u64::encode(self.signatures.len() as u64, &mut bytes);
        for signature in &self.signatures {
            bytes.extend_from_slice(signature.as_slice());
        }
        bytes.extend_from_slice(self.payload.as_slice());
        bytes
    }

    fn decode(data: &[u8]) -> eyre::Result<Self> {
        let mut input = crate::parsing::Input::new(data);
        let num_parents = input.u64().wrap_err("failed to decode number of parents")? as usize;
        let mut parents = Vec::with_capacity(num_parents);
        for _ in 0..num_parents {
            let parent_bytes = input.bytes(32).wrap_err("failed to read parent hash")?;
            let parent_hash = CommitHash::try_from(parent_bytes).wrap_err("failed to parse parent hash")?;
            parents.push(parent_hash);
        }
        let author_flag = input.bytes(1).wrap_err("failed to read author flag")?;
        let author = match author_flag[0] {
            1 => {
                let author_bytes = input.bytes(32).wrap_err("failed to read author bytes")?;
                Some(Author::try_from(author_bytes).wrap_err("failed to parse author")?)
            },
            0 => None,
            _ => return Err(eyre!("invalid author flag")),
        };
        let num_signatures = input.u64().wrap_err("failed to decode number of signatures")? as usize;
        let mut signatures = Vec::with_capacity(num_signatures);
        for _ in 0..num_signatures {
            let signature_bytes = input.array::<64>().wrap_err("failed to read signature bytes")?;
            let signature = Signature::from(signature_bytes);
            signatures.push(signature);
        }
        let payload_bytes = input.array::<32>().wrap_err("failed to read payload hash")?;
        let payload_hash = BlobHash::from(payload_bytes);
        Ok(Self {
            parents,
            author,
            signatures,
            payload: payload_hash,
        })
    }
}

#[derive(Debug)]
pub struct ParseCommitError(String);

impl std::error::Error for ParseCommitError {}
impl std::fmt::Display for ParseCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ParseCommitError: {}", self.0)
    }
}
