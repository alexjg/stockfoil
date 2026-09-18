#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash([u8; 32]);

impl BlobHash {
    pub fn of_contents(data: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(data);
        Self(hasher.finalize().into())
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl From<[u8; 32]> for BlobHash {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<BlobHash> for [u8; 32] {
    fn from(value: BlobHash) -> Self {
        value.0
    }
}
