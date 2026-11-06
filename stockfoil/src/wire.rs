//! Stockfoil wire format, version 1. See `design/spec.md`.
//!
//! ```text
//! "STKF" 01 | header length (LEB128) | header | application body until end of item
//! ```
//!
//! The header contains columnar metadata; the application body follows it.
//! Items are delimited by their storage or transport, not by this format.
use crate::meta::Evidence;
use crate::reader::Reader;
use crate::{
    Author, BundleMeta, CommitHash, Error, FormatId, FragmentRange, ItemId, Limits, MemberShape,
    ParentRef, Signature,
};
use hexane::{Codec as _, Leb128};
use std::collections::{BTreeMap, BTreeSet};

pub const MAGIC: &[u8; 5] = b"STKF\x01";

fn malformed<T>() -> Result<T, Error> {
    Err(Error::Malformed("invalid wire v1 header"))
}

fn bound(n: usize, max: usize, name: &'static str) -> Result<usize, Error> {
    if n > max {
        Err(Error::Limit(name))
    } else {
        Ok(n)
    }
}

impl<'a> Reader<'a> {
    /// Minimal unsigned LEB128, at most `u32::MAX`.
    fn number(&mut self) -> Result<usize, Error> {
        let rest = self.rest();
        let (n, v) = Leb128::read_unsigned(rest).ok_or(Error::Malformed("number"))?;
        if v > u32::MAX as u64 || Leb128::encode_unsigned(v).as_bytes() != &rest[..n] {
            return malformed();
        }
        self.take(n)?;
        usize::try_from(v).or_else(|_| malformed())
    }
    /// A `number` length, then that many bytes.
    fn blob(&mut self) -> Result<&'a [u8], Error> {
        let n = self.number()?;
        self.take(n)
    }
}

fn put(out: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).expect("wire v1 counts fit in u32");
    out.extend(Leb128::encode_unsigned(n as u64));
}

fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    put(out, bytes.len());
    out.extend_from_slice(bytes);
}

fn integers(values: impl IntoIterator<Item = usize>) -> Vec<u8> {
    let mut enc = hexane::leb128::Encoder::<u64>::default();
    for n in values {
        enc.append(n as u64);
    }
    enc.save()
}

fn booleans(values: impl IntoIterator<Item = bool>) -> Vec<u8> {
    let mut enc = hexane::leb128::Encoder::<bool>::default();
    for v in values {
        enc.append(v);
    }
    enc.save()
}

// Validate run structure and totals before handing bytes to hexane's loaders,
// whose bookkeeping assumes well-formed input.
fn preflight(bytes: &[u8], expected: usize, boolean: bool) -> Result<(), Error> {
    let mut data = bytes;
    let mut total = 0usize;
    while !data.is_empty() {
        let count;
        let literal;
        if boolean {
            let (n, v) = Leb128::read_unsigned(data).ok_or(Error::Malformed("column"))?;
            if Leb128::encode_unsigned(v).as_bytes() != &data[..n] {
                return malformed();
            }
            data = &data[n..];
            count = usize::try_from(v).or_else(|_| malformed())?;
            literal = false;
        } else {
            let (n, v) = Leb128::read_signed(data).ok_or(Error::Malformed("column"))?;
            if v == 0 || v == i64::MIN || Leb128::encode_signed(v).as_bytes() != &data[..n] {
                return malformed();
            }
            data = &data[n..];
            count = usize::try_from(v.unsigned_abs()).or_else(|_| malformed())?;
            literal = v < 0;
        }
        total = total.checked_add(count).ok_or(Error::Malformed("column"))?;
        if total > expected {
            return Err(Error::Limit("column expansion"));
        }
        if !boolean {
            for _ in 0..if literal { count } else { 1 } {
                let (n, v) = Leb128::read_unsigned(data).ok_or(Error::Malformed("column"))?;
                if v > u32::MAX as u64 || Leb128::encode_unsigned(v).as_bytes() != &data[..n] {
                    return malformed();
                }
                data = &data[n..];
            }
        }
    }
    if total != expected {
        return malformed();
    }
    Ok(())
}

fn read_integers(bytes: &[u8], expected: usize) -> Result<Vec<usize>, Error> {
    preflight(bytes, expected, false)?;
    let mut loader = hexane::RleLoadIter::<u64, Leb128>::new(bytes, 1024);
    let mut out = Vec::with_capacity(expected);
    while let Some(run) = loader.try_next_run().or_else(|_| malformed())? {
        let end = out
            .len()
            .checked_add(run.count)
            .ok_or(Error::Malformed("column"))?;
        if end > expected {
            return malformed();
        }
        out.resize(end, usize::try_from(run.value).or_else(|_| malformed())?);
    }
    if out.len() != expected {
        return malformed();
    }
    Ok(out)
}

fn read_booleans(bytes: &[u8], expected: usize) -> Result<Vec<bool>, Error> {
    preflight(bytes, expected, true)?;
    let mut loader = hexane::BoolLoadIter::<Leb128>::new(bytes, 1024);
    let mut out = Vec::with_capacity(expected);
    while let Some(run) = loader.try_next_run().or_else(|_| malformed())? {
        let end = out
            .len()
            .checked_add(run.count)
            .ok_or(Error::Malformed("column"))?;
        if end > expected {
            return malformed();
        }
        out.resize(end, run.value);
    }
    if out.len() != expected || booleans(out.iter().copied()) != bytes {
        return malformed();
    }
    Ok(out)
}

/// Serialize the item prefix: magic, header length, and header.
pub fn encode_header(meta: &BundleMeta, format: &FormatId) -> Vec<u8> {
    let mut h = Vec::new();
    let (fragment, head, boundary) = match &meta.id {
        ItemId::Commit(hash) => (false, *hash, BTreeSet::new()),
        ItemId::Fragment(range) => (true, range.head, range.boundary.clone()),
    };
    h.push(fragment as u8);
    blob(&mut h, format.as_str().as_bytes());
    h.extend(head.as_bytes());
    put(&mut h, boundary.len());
    for b in &boundary {
        h.extend(b.as_bytes());
    }
    let members = &meta.members;
    put(&mut h, members.len());
    let authors: BTreeMap<Author, usize> = members
        .iter()
        .map(|m| m.author)
        .chain(meta.evidence.iter().map(|e| e.author))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, a)| (a, i))
        .collect();
    put(&mut h, authors.len());
    for a in authors.keys() {
        h.extend(a.as_slice());
    }
    let external: BTreeMap<CommitHash, usize> = members
        .iter()
        .flat_map(|m| &m.parents)
        .filter_map(|p| match p {
            ParentRef::External(hash) => Some(*hash),
            ParentRef::Member(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, hash)| (hash, i))
        .collect();
    put(&mut h, external.len());
    for hash in external.keys() {
        h.extend(hash.as_bytes());
    }
    let parents = || {
        members
            .iter()
            .enumerate()
            .flat_map(|(i, m)| m.parents.iter().map(move |p| (i, p)))
    };
    blob(
        &mut h,
        &integers(members.iter().map(|m| authors[&m.author])),
    );
    blob(&mut h, &integers(members.iter().map(|m| m.parents.len())));
    blob(
        &mut h,
        &booleans(parents().map(|(_, p)| matches!(p, ParentRef::Member(_)))),
    );
    blob(
        &mut h,
        &integers(parents().filter_map(|(i, p)| match p {
            ParentRef::Member(j) => Some(i - j),
            ParentRef::External(_) => None,
        })),
    );
    blob(
        &mut h,
        &integers(parents().filter_map(|(_, p)| match p {
            ParentRef::External(hash) => Some(external[hash]),
            ParentRef::Member(_) => None,
        })),
    );
    put(&mut h, meta.evidence.len());
    blob(
        &mut h,
        &integers(meta.evidence.iter().map(|e| authors[&e.author])),
    );
    blob(&mut h, &integers(meta.evidence.iter().map(|e| e.member)));
    for e in &meta.evidence {
        h.extend(e.signature.as_slice());
    }
    let mut out = MAGIC.to_vec();
    put(&mut out, h.len());
    out.extend(h);
    out
}

/// Inspect the start of an item. Returns `Ok(None)` if more bytes are needed,
/// otherwise the total length of magic, length prefix, and header.
pub fn header_len(prefix: &[u8], limits: &Limits) -> Result<Option<usize>, Error> {
    let magic = prefix.len().min(MAGIC.len());
    if prefix[..magic] != MAGIC[..magic] {
        return Err(Error::Malformed("not a wire v1 item"));
    }
    let rest = &prefix[magic..];
    if magic < MAGIC.len() || rest.is_empty() {
        return Ok(None);
    }
    let Some((n, v)) = Leb128::read_unsigned(rest) else {
        // Five bytes always suffice for a u32.
        return if rest.len() >= 5 {
            malformed()
        } else {
            Ok(None)
        };
    };
    if v > u32::MAX as u64 || Leb128::encode_unsigned(v).as_bytes() != &rest[..n] {
        return malformed();
    }
    let len = bound(v as usize, limits.max_header_bytes, "header bytes")?;
    Ok(Some(MAGIC.len() + n + len))
}

/// Parse a complete item prefix as measured by [`header_len`]. The format ID is
/// checked against `expected` before any member sequence is allocated.
pub fn decode_header(
    bytes: &[u8],
    expected: &FormatId,
    limits: &Limits,
) -> Result<BundleMeta, Error> {
    let total = header_len(bytes, limits)?.ok_or(Error::Malformed("truncated header"))?;
    if total != bytes.len() {
        return Err(Error::Malformed("header length mismatch"));
    }
    let mut r = Reader::new(bytes);
    r.take(MAGIC.len())?;
    r.number()?;
    let fragment = match r.take(1)?[0] {
        0 => false,
        1 => true,
        _ => return malformed(),
    };
    let id = r.blob()?;
    if id.is_empty() || id.len() > 128 || std::str::from_utf8(id).is_err() {
        return Err(Error::Malformed("format ID"));
    }
    if id != expected.as_str().as_bytes() {
        return Err(Error::WrongFormat);
    }
    let head = r.hash()?;
    let nb = bound(r.number()?, limits.max_boundaries, "boundaries")?;
    if !fragment && nb != 0 {
        return malformed();
    }
    let mut boundary = BTreeSet::new();
    for _ in 0..nb {
        let h = r.hash()?;
        if boundary.last().is_some_and(|last| *last >= h) {
            return malformed();
        }
        boundary.insert(h);
    }
    let n = bound(r.number()?, limits.max_members, "members")?;
    if n == 0 || (!fragment && n != 1) {
        return malformed();
    }
    let na = r.number()?;
    if na > r.remaining() / 32 {
        return malformed();
    }
    let mut authors = Vec::with_capacity(na);
    for _ in 0..na {
        let a = Author::try_from(r.take(32)?).or_else(|_| malformed())?;
        if authors.last().is_some_and(|last| *last >= a) {
            return malformed();
        }
        authors.push(a);
    }
    let ne = bound(r.number()?, limits.max_parents, "external parents")?;
    if ne > r.remaining() / 32 {
        return malformed();
    }
    let mut external = Vec::with_capacity(ne);
    for _ in 0..ne {
        let h = r.hash()?;
        if external.last().is_some_and(|last| *last >= h) {
            return malformed();
        }
        external.push(h);
    }
    let author_indices = read_integers(r.blob()?, n)?;
    let counts = read_integers(r.blob()?, n)?;
    let mut np = 0usize;
    for count in &counts {
        np = np
            .checked_add(*count)
            .ok_or(Error::Malformed("parent count"))?;
        bound(np, limits.max_parents, "parents")?;
    }
    let kinds = read_booleans(r.blob()?, np)?;
    let ni = kinds.iter().filter(|v| **v).count();
    let internal = read_integers(r.blob()?, ni)?;
    let ext = read_integers(r.blob()?, np - ni)?;
    let (mut kinds, mut internal, mut ext) =
        (kinds.into_iter(), internal.into_iter(), ext.into_iter());
    let mut members = Vec::with_capacity(n);
    for (i, (a, count)) in author_indices.into_iter().zip(counts).enumerate() {
        let author = *authors.get(a).ok_or(Error::Malformed("author index"))?;
        let mut parents = Vec::with_capacity(count);
        for _ in 0..count {
            parents.push(if kinds.next().ok_or(Error::Malformed("parent kind"))? {
                let d = internal.next().ok_or(Error::Malformed("parent distance"))?;
                if d == 0 || d > i {
                    return malformed();
                }
                ParentRef::Member(i - d)
            } else {
                let x = ext.next().ok_or(Error::Malformed("external index"))?;
                ParentRef::External(*external.get(x).ok_or(Error::Malformed("external index"))?)
            });
        }
        members.push(MemberShape { author, parents });
    }
    let ns = bound(r.number()?, limits.max_signatures, "signatures")?;
    let signers = read_integers(r.blob()?, ns)?;
    let targets = read_integers(r.blob()?, ns)?;
    if ns > r.remaining() / 64 {
        return malformed();
    }
    let mut evidence = Vec::with_capacity(ns);
    for (a, member) in signers.into_iter().zip(targets) {
        if member >= n {
            return malformed();
        }
        evidence.push(Evidence {
            author: *authors.get(a).ok_or(Error::Malformed("signer index"))?,
            member,
            signature: Signature::from(r.array::<64>()?),
        });
    }
    r.finish()?;
    Ok(BundleMeta {
        id: if fragment {
            ItemId::Fragment(FragmentRange { head, boundary })
        } else {
            ItemId::Commit(head)
        },
        members,
        evidence,
    })
}

#[cfg(test)]
mod tests;
