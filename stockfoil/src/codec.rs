use crate::{BundleMeta, Bytes, FormatId};

/// Packs and unpacks commit payloads inside wire items for an in-memory
/// [`Document`](crate::Document) or [`Replica`](crate::Replica).
///
/// A wire item—whether a 1-commit standalone item or an $N$-commit compacted
/// fragment—consists of a header (managed by `stockfoil`) and a body (managed
/// by your `Codec`). Each individual commit has **canonical payload bytes**,
/// which are hashed to compute its [`CommitHash`](crate::CommitHash).
///
/// - [`encode`](Self::encode) receives the item's [`BundleMeta`] (the authors
///   and parent structure of all $N$ commits in ancestor-first order) and the
///   slice of $N$ canonical payloads, and returns a single encoded body buffer.
///   A codec can compress across all $N$ commits together (for example,
///   deduplicating keys or running columnar compression).
/// - [`decode`](Self::decode) receives the parsed [`BundleMeta`] (before
///   signatures have been verified) and the body bytes, and must return
///   `meta.len()` canonical payloads.
///
/// **Lossless round-trip requirement:** Because receivers recompute each
/// member's [`CommitHash`](crate::CommitHash) from the canonical bytes returned
/// by [`decode`](Self::decode) before verifying Ed25519 signatures, decoding an
/// encoded body must return the exact byte-for-byte canonical payload for each
/// member.
///
/// (Applications using [`Ledger`](crate::Ledger) for out-of-core I/O drive
/// [`Admission`](crate::Admission)'s streaming [`Request::Decode`](crate::Request::Decode)
/// steps directly instead of implementing this in-memory trait.)
pub trait Codec {
    type Error: std::fmt::Display;
    fn format(&self) -> &FormatId;
    fn encode(&self, meta: &BundleMeta, members: &[Bytes]) -> Result<Vec<u8>, Self::Error>;
    /// Decode `body` into one canonical payload per entry in `meta.members`.
    /// Note that `meta` is structurally validated but not yet authenticated when
    /// `decode` is called.
    fn decode(&self, meta: &BundleMeta, body: &[u8]) -> Result<Vec<Bytes>, Self::Error>;
}

/// A simple [`Codec`] that concatenates each member's canonical payload with a
/// 64-bit little-endian length prefix.
///
/// Useful when getting started or when individual commit payloads are already
/// self-contained and do not need cross-commit fragment compression.
pub struct Concatenated {
    format: FormatId,
}
impl Concatenated {
    pub fn new(format: FormatId) -> Self {
        Self { format }
    }
}
impl Codec for Concatenated {
    type Error = &'static str;
    fn format(&self) -> &FormatId {
        &self.format
    }
    fn encode(&self, _: &BundleMeta, members: &[Bytes]) -> Result<Vec<u8>, Self::Error> {
        let mut out = Vec::new();
        for member in members {
            out.extend((member.len() as u64).to_le_bytes());
            out.extend_from_slice(member);
        }
        Ok(out)
    }
    fn decode(&self, meta: &BundleMeta, mut body: &[u8]) -> Result<Vec<Bytes>, Self::Error> {
        let mut members = Vec::with_capacity(meta.len().min(body.len() / 8));
        for _ in 0..meta.len() {
            let prefix = body.get(..8).ok_or("truncated member length")?;
            let len = usize::try_from(u64::from_le_bytes(prefix.try_into().unwrap()))
                .map_err(|_| "member length overflow")?;
            body = &body[8..];
            let bytes = body.get(..len).ok_or("truncated member")?;
            members.push(Bytes::from(bytes));
            body = &body[len..];
        }
        if !body.is_empty() {
            return Err("trailing member bytes");
        }
        Ok(members)
    }
}
