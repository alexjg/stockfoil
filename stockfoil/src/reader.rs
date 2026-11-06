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
    /// A collection count bounded by the bytes still available. Check before
    /// allocating or looping over an untrusted durable-record length.
    pub fn count(&mut self, minimum_item_bytes: usize) -> Result<usize, Error> {
        let count = usize::try_from(self.u64_le()?)
            .map_err(|_| Error::Malformed("collection count overflow"))?;
        if count > self.remaining() / minimum_item_bytes {
            return Err(Error::Malformed("truncated collection"));
        }
        Ok(count)
    }
    pub fn hashes(&mut self) -> Result<Vec<CommitHash>, Error> {
        let count = self.count(32)?;
        (0..count).map(|_| self.hash()).collect()
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
