use alloc::string::{String, ToString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Author([u8; 32]);

impl Author {
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn as_array(&self) -> &[u8; 32] {
        &self.0
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

impl core::error::Error for InvalidAuthor {}
impl core::fmt::Display for InvalidAuthor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
