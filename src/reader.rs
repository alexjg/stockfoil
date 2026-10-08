//! A bounds-checked byte reader for wire headers and durable cursors.
use crate::{CommitHash, Error};

/// A bounds-checked byte reader.
pub(crate) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }
    pub fn remaining(&self) -> usize {
        self.0.len()
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.0.len() {
            return Err(Error::Malformed("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    pub fn hash(&mut self) -> Result<CommitHash, Error> {
        Ok(CommitHash::from(self.array::<32>()?))
    }
    pub fn u64_le(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }
    /// The unread bytes, without consuming them.
    pub fn rest(&self) -> &'a [u8] {
        self.0
    }
    pub fn finish(self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Malformed("trailing bytes"))
        }
    }
}
