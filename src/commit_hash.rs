use alloc::string::{String, ToString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitHash([u8; 32]);

impl CommitHash {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl<'a> TryFrom<&'a [u8]> for CommitHash {
    type Error = InvalidCommitHash;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let bytes = <[u8; 32]>::try_from(value).map_err(|e| InvalidCommitHash(e.to_string()))?;
        Ok(Self(bytes))
    }
}

impl From<[u8; 32]> for CommitHash {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<CommitHash> for [u8; 32] {
    fn from(value: CommitHash) -> Self {
        value.0
    }
}

impl From<blake3::Hash> for CommitHash {
    fn from(value: blake3::Hash) -> Self {
        Self(<[u8; 32]>::from(value))
    }
}

#[derive(Debug)]
pub struct InvalidCommitHash(String);

impl core::error::Error for InvalidCommitHash {}

impl core::fmt::Display for InvalidCommitHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
