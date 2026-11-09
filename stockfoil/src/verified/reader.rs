//! Checked byte consumption shared by wire headers and durable metadata decoders.
use super::{api_error::Error, commit_hash::CommitHash};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) struct Reader<'a>(&'a [u8]);

pub(crate) open spec fn prefix_count(bytes: Seq<u8>) -> u64
    recommends bytes.len() >= 8,
{
    (bytes[0] as u64) | (bytes[1] as u64) << 8 | (bytes[2] as u64) << 16
        | (bytes[3] as u64) << 24 | (bytes[4] as u64) << 32 | (bytes[5] as u64) << 40
        | (bytes[6] as u64) << 48 | (bytes[7] as u64) << 56
}

pub(crate) open spec fn fits_count(bytes: Seq<u8>, width: int) -> bool {
    bytes.len() >= 8 && width > 0 && prefix_count(bytes) <= usize::MAX
        && prefix_count(bytes) <= (bytes.len() - 8) / width
}

impl<'a> Reader<'a> {
    pub(crate) closed spec fn unread(&self) -> Seq<u8> { self.0@ }

    pub(crate) fn new(bytes: &'a [u8]) -> (out: Self)
        ensures out.unread() == bytes@,
    { Self(bytes) }

    pub(crate) fn remaining(&self) -> (out: usize)
        ensures out == self.unread().len(),
    { self.0.len() }

    pub(crate) fn take(&mut self, n: usize) -> (out: Result<&'a [u8], Error>)
        ensures match out {
            Ok(bytes) => n <= old(self).unread().len() && bytes@ == old(self).unread().take(n as int)
                && final(self).unread() == old(self).unread().skip(n as int),
            Err(e) => e is Malformed && n > old(self).unread().len() && final(self).unread() == old(self).unread(),
        },
    {
        if n > self.0.len() { return Err(Error::Malformed("truncated")); }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> (out: Result<[u8; N], Error>)
        ensures match out {
            Ok(bytes) => N <= old(self).unread().len() && bytes@ == old(self).unread().take(N as int)
                && final(self).unread() == old(self).unread().skip(N as int),
            Err(e) => e is Malformed && N > old(self).unread().len() && final(self).unread() == old(self).unread(),
        },
    {
        let bytes = self.take(N)?;
        let mut out = [0u8; N];
        let mut i = 0usize;
        while i < N
            invariant i <= N, bytes.len() == N,
                forall|j: int| 0 <= j < i ==> out@[j] == bytes@[j],
            decreases N - i,
        { out[i] = bytes[i]; i += 1; }
        assert(out@ =~= bytes@);
        Ok(out)
    }

    pub(crate) fn hash(&mut self) -> (out: Result<CommitHash, Error>)
        ensures match out {
            Ok(hash) => 32 <= old(self).unread().len() && hash.0@ == old(self).unread().take(32)
                && final(self).unread() == old(self).unread().skip(32),
            Err(e) => e is Malformed && 32 > old(self).unread().len() && final(self).unread() == old(self).unread(),
        },
    { Ok(CommitHash(self.array::<32>()?)) }

    pub(crate) fn u64_le(&mut self) -> (out: Result<u64, Error>)
        ensures match out {
            Ok(n) => 8 <= old(self).unread().len() && n == prefix_count(old(self).unread())
                && final(self).unread() == old(self).unread().skip(8),
            Err(e) => e is Malformed && 8 > old(self).unread().len() && final(self).unread() == old(self).unread(),
        },
    {
        let bytes = self.array::<8>()?;
        Ok((bytes[0] as u64) | (bytes[1] as u64) << 8 | (bytes[2] as u64) << 16
            | (bytes[3] as u64) << 24 | (bytes[4] as u64) << 32 | (bytes[5] as u64) << 40
            | (bytes[6] as u64) << 48 | (bytes[7] as u64) << 56)
    }

    /// Validate an untrusted collection length before allocation or iteration.
    pub(crate) fn count(&mut self, minimum_item_bytes: usize) -> (out: Result<usize, Error>)
        requires minimum_item_bytes > 0,
        ensures
            final(self).unread() == if old(self).unread().len() < 8 { old(self).unread() } else { old(self).unread().skip(8) },
            match out {
                Ok(n) => fits_count(old(self).unread(), minimum_item_bytes as int) && n == prefix_count(old(self).unread())
                    && n <= final(self).unread().len() / (minimum_item_bytes as nat),
                Err(e) => e is Malformed && !fits_count(old(self).unread(), minimum_item_bytes as int),
            },
    {
        let count = match usize::try_from(self.u64_le()?) {
            Ok(n) => n, Err(_) => return Err(Error::Malformed("collection count overflow")),
        };
        if count > self.remaining() / minimum_item_bytes { return Err(Error::Malformed("truncated collection")); }
        Ok(count)
    }

    pub(crate) fn hashes(&mut self) -> (out: Result<Vec<CommitHash>, Error>)
        ensures match out {
            Ok(hashes) => fits_count(old(self).unread(), 32)
                && hashes.len() == prefix_count(old(self).unread())
                && final(self).unread() == old(self).unread().skip(8 + 32 * hashes.len())
                && forall|i: int| 0 <= i < hashes.len() ==> hashes@[i].0@ == old(self).unread().subrange(8 + 32*i, 8 + 32*(i+1)),
            Err(e) => e is Malformed && !fits_count(old(self).unread(), 32)
                && final(self).unread() == if old(self).unread().len() < 8 { old(self).unread() } else { old(self).unread().skip(8) },
        },
    {
        let ghost original = self.unread();
        let count = self.count(32)?;
        let mut hashes: Vec<CommitHash> = Vec::new();
        while hashes.len() < count
            invariant fits_count(original, 32), count == prefix_count(original), hashes.len() <= count,
                self.unread() == original.skip(8 + 32 * hashes.len()),
                32 * count <= original.len() - 8,
                forall|i: int| 0 <= i < hashes.len() ==> hashes@[i].0@ == original.subrange(8 + 32*i, 8 + 32*(i+1)),
            decreases count - hashes.len(),
        {
            let hash = self.hash()?;
            hashes.push(hash);
        }
        Ok(hashes)
    }

    pub(crate) fn rest(&self) -> (out: &'a [u8]) ensures out@ == self.unread(), { self.0 }

    pub(crate) fn finish(self) -> (out: Result<(), Error>)
        ensures match out { Ok(()) => self.unread().len() == 0, Err(e) => e is Malformed && self.unread().len() > 0 },
    {
        if self.0.is_empty() { Ok(()) } else { Err(Error::Malformed("trailing bytes")) }
    }
}
} // verus!
