pub use crate::verified::author::Author;

impl Author {
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn as_array(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for Author {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<ed25519_dalek::VerifyingKey> for Author {
    fn from(key: ed25519_dalek::VerifyingKey) -> Self {
        Self(key.to_bytes())
    }
}

impl From<&ed25519_dalek::VerifyingKey> for Author {
    fn from(key: &ed25519_dalek::VerifyingKey) -> Self {
        Self(key.to_bytes())
    }
}

impl From<&ed25519_dalek::SigningKey> for Author {
    fn from(key: &ed25519_dalek::SigningKey) -> Self {
        Self(key.verifying_key().to_bytes())
    }
}

impl TryFrom<&[u8]> for Author {
    type Error = InvalidAuthor;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = <[u8; 32]>::try_from(value).map_err(|e| InvalidAuthor(e.to_string()))?;
        Ok(Self(bytes))
    }
}

#[derive(Debug)]
pub struct InvalidAuthor(String);

impl std::error::Error for InvalidAuthor {}
impl std::fmt::Display for InvalidAuthor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Author;

    #[test]
    fn checked_identity_comparison_covers_every_byte() {
        let bytes = [0xa5; 32];
        let author = Author::from(bytes);
        assert!(author.same_as(&author));
        for position in 0..32 {
            let mut other = bytes;
            other[position] ^= 0xff;
            let other = Author::from(other);
            assert_ne!(author, other);
            assert!(!author.same_as(&other));
            assert!(!other.same_as(&author));
        }
    }
}
