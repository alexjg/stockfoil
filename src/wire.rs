//! Stockfoil wire v1. See design/codec-wire-v1.md. Hexane is private to this module.
use crate::*;
use hexane::{Codec as _, Leb128};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
mod tests;

pub(crate) fn check_format<E>(id: &str) -> Result<(), Error<E>> {
    if id.is_empty() || id.len() > 128 {
        return Err(Error::InvalidEncoding(
            "codec ID must have 1..128 UTF-8 bytes",
        ));
    }
    Ok(())
}
fn invalid<E>() -> Error<E> {
    Error::InvalidEncoding("invalid wire v1 structure")
}
fn bound<E>(n: usize, max: usize, name: &'static str) -> Result<usize, Error<E>> {
    if n > max {
        Err(Error::LimitExceeded(name))
    } else {
        Ok(n)
    }
}
fn wire_u32<E>(n: usize) -> Result<u32, Error<E>> {
    u32::try_from(n).map_err(|_| Error::InvalidEncoding("wire v1 count exceeds u32"))
}
fn put<E>(out: &mut Vec<u8>, n: usize) -> Result<(), Error<E>> {
    out.extend(Leb128::encode_unsigned(wire_u32::<E>(n)? as u64));
    Ok(())
}
fn blob<E>(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error<E>> {
    put(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take<E>(&mut self, n: usize) -> Result<&'a [u8], Error<E>> {
        if n > self.0.len() {
            return Err(invalid());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn array<const N: usize, E>(&mut self) -> Result<[u8; N], Error<E>> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn number<E>(&mut self) -> Result<usize, Error<E>> {
        let (n, v) = Leb128::read_unsigned(self.0).ok_or_else(invalid)?;
        if v > u32::MAX as u64 || Leb128::encode_unsigned(v).as_bytes() != &self.0[..n] {
            return Err(invalid());
        }
        self.take::<E>(n)?;
        usize::try_from(v).map_err(|_| invalid())
    }
    fn blob<E>(&mut self) -> Result<&'a [u8], Error<E>> {
        let n = self.number()?;
        self.take(n)
    }
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
    for n in values {
        enc.append(n);
    }
    enc.save()
}
// Preflight is essential even before the validating loader: alpha.5's slab
// bookkeeping uses unchecked usize additions, and signed MIN negation. Bound
// every run and checked total first. No untrusted Iterator decoder is used.
fn preflight<E>(bytes: &[u8], expected: usize, boolean: bool) -> Result<(), Error<E>> {
    let mut data = bytes;
    let mut total = 0usize;
    while !data.is_empty() {
        let count;
        let literal;
        if boolean {
            let (n, v) = Leb128::read_unsigned(data).ok_or_else(invalid)?;
            if Leb128::encode_unsigned(v).as_bytes() != &data[..n] {
                return Err(invalid());
            }
            data = &data[n..];
            count = usize::try_from(v).map_err(|_| invalid())?;
            literal = false;
        } else {
            let (n, v) = Leb128::read_signed(data).ok_or_else(invalid)?;
            if v == 0 || v == i64::MIN || Leb128::encode_signed(v).as_bytes() != &data[..n] {
                return Err(invalid());
            }
            data = &data[n..];
            count = usize::try_from(v.unsigned_abs()).map_err(|_| invalid())?;
            literal = v < 0;
        }
        total = total.checked_add(count).ok_or_else(invalid)?;
        if total > expected {
            return Err(Error::LimitExceeded("column expansion"));
        }
        if !boolean {
            for _ in 0..if literal { count } else { 1 } {
                let (n, v) = Leb128::read_unsigned(data).ok_or_else(invalid)?;
                if v > u32::MAX as u64 || Leb128::encode_unsigned(v).as_bytes() != &data[..n] {
                    return Err(invalid());
                }
                data = &data[n..];
            }
        }
    }
    if total != expected {
        return Err(invalid());
    }
    Ok(())
}
fn read_integers<E>(bytes: &[u8], expected: usize) -> Result<Vec<usize>, Error<E>> {
    preflight(bytes, expected, false)?;
    let mut loader = hexane::RleLoadIter::<u64, Leb128>::new(bytes, 1024);
    let mut out = Vec::with_capacity(expected);
    while let Some(run) = loader.try_next_run().map_err(|_| invalid())? {
        let end = out.len().checked_add(run.count).ok_or_else(invalid)?;
        if end > expected {
            return Err(invalid());
        }
        out.resize(end, usize::try_from(run.value).map_err(|_| invalid())?);
    }
    if out.len() != expected {
        return Err(invalid());
    }
    Ok(out)
}
fn read_booleans<E>(bytes: &[u8], expected: usize) -> Result<Vec<bool>, Error<E>> {
    preflight(bytes, expected, true)?;
    let mut loader = hexane::BoolLoadIter::<Leb128>::new(bytes, 1024);
    let mut out = Vec::with_capacity(expected);
    while let Some(run) = loader.try_next_run().map_err(|_| invalid())? {
        let end = out.len().checked_add(run.count).ok_or_else(invalid)?;
        if end > expected {
            return Err(invalid());
        }
        out.resize(end, run.value);
    }
    if out.len() != expected {
        return Err(invalid());
    }
    // Reject otherwise ambiguous empty/trailing zero bool runs too.
    if booleans(out.iter().copied()) != bytes {
        return Err(invalid());
    }
    Ok(out)
}
fn shape(commits: &[CommitMeta]) -> FragmentShape {
    let mut seen = HashMap::new();
    let members = commits
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let parents = m
                .parents()
                .iter()
                .map(|p| {
                    seen.get(p)
                        .copied()
                        .map(ParentRef::Member)
                        .unwrap_or(ParentRef::External(*p))
                })
                .collect();
            seen.insert(*m.hash(), i);
            MemberShape {
                author: *m.author(),
                parents,
            }
        })
        .collect();
    FragmentShape { members }
}

pub(crate) fn encode<'a, C: PayloadCodec>(
    codec: &C,
    item: UnverifiedItem,
    mut lookup: impl FnMut(&CommitHash) -> Option<&'a C::Payload>,
) -> Result<EncodedItem, Error<C::Error>>
where
    C::Payload: 'a,
{
    check_format(codec.format_id())?;
    let (metadata, commits, signatures) = match item {
        UnverifiedItem::Commit { meta, signatures } => (
            TransportMetadata::Commit {
                hash: *meta.hash(),
                parents: meta.parents().iter().copied().collect(),
            },
            vec![meta],
            signatures,
        ),
        UnverifiedItem::Fragment {
            range,
            commits,
            signatures,
            checkpoints,
        } => (
            TransportMetadata::Fragment { range, checkpoints },
            commits,
            signatures,
        ),
    };
    // Local history is trusted, but all serialized counts/indices must fit the
    // wire schema. DecodeLimits are solely an inbound resource policy.
    wire_u32::<C::Error>(commits.len())?;
    wire_u32::<C::Error>(signatures.len())?;
    if commits.is_empty() {
        return Err(invalid());
    }
    for m in &commits {
        wire_u32::<C::Error>(m.parents().len())?;
    }
    let shape = shape(&commits);
    let mut payloads = Vec::with_capacity(commits.len());
    let mut canonical = None;
    for m in &commits {
        let p = lookup(m.hash()).ok_or_else(invalid)?;
        let bytes = codec.encode_commit(p).map_err(Error::Codec)?;
        if BlobHash::of_contents(&bytes) != *m.payload_hash() {
            return Err(Error::PayloadHashMismatch);
        }
        payloads.push(p);
        // Hash-and-drop fragment canonical bytes; only standalone encoding
        // needs to retain them as the wire body.
        if matches!(metadata, TransportMetadata::Commit { .. }) {
            canonical = Some(bytes);
        }
    }
    let body = match &metadata {
        TransportMetadata::Commit { .. } => canonical.ok_or_else(invalid)?,
        TransportMetadata::Fragment { .. } => codec
            .encode_fragment(&shape, &payloads)
            .map_err(Error::Codec)?,
    };
    let mut out = b"STKF\x01".to_vec();
    out.push(matches!(metadata, TransportMetadata::Fragment { .. }) as u8);
    blob(&mut out, codec.format_id().as_bytes())?;
    let boundary = match &metadata {
        TransportMetadata::Commit { hash, .. } => {
            out.extend_from_slice(hash.as_bytes());
            BTreeSet::new()
        }
        TransportMetadata::Fragment { range, .. } => {
            out.extend_from_slice(range.head.as_bytes());
            range.boundary.clone()
        }
    };
    put(&mut out, boundary.len())?;
    for h in boundary {
        out.extend_from_slice(h.as_bytes());
    }
    put(&mut out, commits.len())?;
    let authors: BTreeSet<_> = shape
        .members
        .iter()
        .map(|m| m.author)
        .chain(signatures.iter().map(|s| s.author))
        .collect();
    let authors: BTreeMap<_, _> = authors
        .into_iter()
        .enumerate()
        .map(|(i, a)| (a, i))
        .collect();
    put(&mut out, authors.len())?;
    for a in authors.keys() {
        out.extend_from_slice(a.as_slice());
    }
    let external: BTreeSet<_> = shape
        .members
        .iter()
        .flat_map(|m| &m.parents)
        .filter_map(|p| match p {
            ParentRef::External(h) => Some(*h),
            _ => None,
        })
        .collect();
    let external: BTreeMap<_, _> = external
        .into_iter()
        .enumerate()
        .map(|(i, h)| (h, i))
        .collect();
    put(&mut out, external.len())?;
    for h in external.keys() {
        out.extend_from_slice(h.as_bytes());
    }
    blob(
        &mut out,
        &integers(shape.members.iter().map(|m| authors[&m.author])),
    )?;
    blob(
        &mut out,
        &integers(shape.members.iter().map(|m| m.parents.len())),
    )?;
    blob(
        &mut out,
        &booleans(
            shape
                .members
                .iter()
                .flat_map(|m| &m.parents)
                .map(|p| matches!(p, ParentRef::Member(_))),
        ),
    )?;
    blob(
        &mut out,
        &integers(shape.members.iter().enumerate().flat_map(|(i, m)| {
            m.parents.iter().filter_map(move |p| match p {
                ParentRef::Member(j) => Some(i - j),
                _ => None,
            })
        })),
    )?;
    blob(
        &mut out,
        &integers(
            shape
                .members
                .iter()
                .flat_map(|m| &m.parents)
                .filter_map(|p| match p {
                    ParentRef::External(h) => Some(external[h]),
                    _ => None,
                }),
        ),
    )?;
    put(&mut out, signatures.len())?;
    let indices: HashMap<_, _> = commits
        .iter()
        .enumerate()
        .map(|(i, m)| (*m.hash(), i))
        .collect();
    if indices.len() != commits.len() || signatures.iter().any(|s| !indices.contains_key(&s.commit))
    {
        return Err(invalid());
    }
    blob(
        &mut out,
        &integers(signatures.iter().map(|s| authors[&s.author])),
    )?;
    blob(
        &mut out,
        &integers(signatures.iter().map(|s| indices[&s.commit])),
    )?;
    for s in signatures {
        out.extend_from_slice(s.signature.as_slice());
    }
    blob(&mut out, &body)?;
    Ok(EncodedItem {
        metadata,
        bytes: out,
    })
}
struct Parsed<'a> {
    fragment: bool,
    head: CommitHash,
    boundary: BTreeSet<CommitHash>,
    shape: FragmentShape,
    evidence: Vec<(Author, usize, Signature)>,
    body: &'a [u8],
}
fn parse<'a, E>(
    bytes: &'a [u8],
    format: &str,
    limits: &DecodeLimits,
) -> Result<Parsed<'a>, Error<E>> {
    bound(bytes.len(), limits.max_item_bytes, "item bytes")?;
    let mut r = Reader(bytes);
    if r.take::<E>(5)? != b"STKF\x01" {
        return Err(Error::InvalidEncoding("magic or version"));
    }
    let fragment = match r.take::<E>(1)?[0] {
        0 => false,
        1 => true,
        _ => return Err(invalid()),
    };
    let id_len = bound(r.number()?, 128, "codec ID")?;
    let id = std::str::from_utf8(r.take(id_len)?).map_err(|_| invalid())?;
    check_format(id)?;
    if id != format {
        return Err(Error::WrongCodec);
    }
    let head = CommitHash::from(r.array::<32, E>()?);
    let nb = bound(r.number()?, limits.max_parents, "boundaries")?;
    if !fragment && nb != 0 {
        return Err(invalid());
    }
    let mut boundary = BTreeSet::new();
    for _ in 0..nb {
        let h = CommitHash::from(r.array::<32, E>()?);
        if boundary.last().is_some_and(|last| *last >= h) {
            return Err(invalid());
        }
        boundary.insert(h);
    }
    let n = bound(r.number()?, limits.max_members, "members")?;
    if n == 0 || (!fragment && n != 1) {
        return Err(invalid());
    }
    let na = bound(
        r.number()?,
        n.saturating_add(limits.max_signatures),
        "authors",
    )?;
    // Check fixed-width table availability before any count-driven allocation.
    if na > r.0.len() / 32 {
        return Err(invalid());
    }
    let mut authors = Vec::with_capacity(na);
    for _ in 0..na {
        let a = Author::try_from(r.take::<E>(32)?).map_err(|_| invalid())?;
        if authors.last().is_some_and(|last| *last >= a) {
            return Err(invalid());
        }
        authors.push(a);
    }
    let ne = bound(r.number()?, limits.max_parents, "external hashes")?;
    if ne > r.0.len() / 32 {
        return Err(invalid());
    }
    let mut external = Vec::with_capacity(ne);
    for _ in 0..ne {
        let h = CommitHash::from(r.array::<32, E>()?);
        if external.last().is_some_and(|last| *last >= h) {
            return Err(invalid());
        }
        external.push(h);
    }
    let author_indices = read_integers(r.blob()?, n)?;
    let counts = read_integers(r.blob()?, n)?;
    let mut np = 0usize;
    for count in &counts {
        np = np.checked_add(*count).ok_or_else(invalid)?;
        bound(np, limits.max_parents, "parents")?;
    }
    let kinds = read_booleans(r.blob()?, np)?;
    let ni = kinds.iter().filter(|v| **v).count();
    let internal = read_integers(r.blob()?, ni)?;
    let ext = read_integers(r.blob()?, np - ni)?;
    let mut kinds = kinds.into_iter();
    let mut internal = internal.into_iter();
    let mut ext = ext.into_iter();
    let mut members = Vec::with_capacity(n);
    for (i, (a, count)) in author_indices.into_iter().zip(counts).enumerate() {
        let author = *authors.get(a).ok_or_else(invalid)?;
        let mut parents = Vec::with_capacity(count);
        for _ in 0..count {
            parents.push(if kinds.next().ok_or_else(invalid)? {
                let d = internal.next().ok_or_else(invalid)?;
                if d == 0 || d > i {
                    return Err(invalid());
                }
                ParentRef::Member(i - d)
            } else {
                ParentRef::External(
                    *external
                        .get(ext.next().ok_or_else(invalid)?)
                        .ok_or_else(invalid)?,
                )
            });
        }
        members.push(MemberShape { author, parents });
    }
    let ns = bound(r.number()?, limits.max_signatures, "signatures")?;
    let signers = read_integers(r.blob()?, ns)?;
    let targets = read_integers(r.blob()?, ns)?;
    if ns > r.0.len() / 64 {
        return Err(invalid());
    }
    let mut evidence = Vec::with_capacity(ns);
    for (a, i) in signers.into_iter().zip(targets) {
        if i >= n {
            return Err(invalid());
        }
        evidence.push((
            *authors.get(a).ok_or_else(invalid)?,
            i,
            Signature::from(r.array::<64, E>()?),
        ));
    }
    let body = r.blob()?;
    if !r.0.is_empty() {
        return Err(Error::InvalidEncoding("trailing bytes"));
    }
    Ok(Parsed {
        fragment,
        head,
        boundary,
        shape: FragmentShape { members },
        evidence,
        body,
    })
}
pub(crate) fn decode<C: PayloadCodec>(
    codec: &C,
    limits: &DecodeLimits,
    bytes: &[u8],
) -> Result<(UnverifiedItem, HashMap<CommitHash, C::Payload>), Error<C::Error>> {
    check_format(codec.format_id())?;
    let p = parse(bytes, codec.format_id(), limits)?;
    let payloads = if p.fragment {
        codec
            .decode_fragment(&p.shape, p.body, limits)
            .map_err(Error::Codec)?
    } else {
        bound(
            p.body.len(),
            limits.max_payload_bytes,
            "canonical payload bytes",
        )?;
        vec![codec.decode_commit(p.body, limits).map_err(Error::Codec)?]
    };
    if payloads.len() != p.shape.members.len() {
        return Err(Error::InvalidEncoding("decoded payload count"));
    }
    let mut total = 0usize;
    let mut commits: Vec<CommitMeta> = Vec::with_capacity(payloads.len());
    let mut stored = HashMap::new();
    for (m, payload) in p.shape.members.iter().zip(payloads) {
        let bytes = codec.encode_commit(&payload).map_err(Error::Codec)?;
        total = total.checked_add(bytes.len()).ok_or_else(invalid)?;
        bound(total, limits.max_payload_bytes, "canonical payload bytes")?;
        if !p.fragment && bytes != p.body {
            return Err(Error::NonCanonicalPayload);
        }
        let parents = m
            .parents
            .iter()
            .map(|r| match r {
                ParentRef::Member(i) => *commits[*i].hash(),
                ParentRef::External(h) => *h,
            })
            .collect();
        let meta = CommitMeta::new(CommitContents {
            parents,
            author: m.author,
            payload: BlobHash::of_contents(&bytes),
        });
        if stored.insert(*meta.hash(), payload).is_some() {
            return Err(invalid());
        }
        commits.push(meta);
    }
    // External means outside membership, not merely an alternate full-hash
    // spelling of a member reference. Check after all hashes are reconstructed.
    if p.shape
        .members
        .iter()
        .flat_map(|m| &m.parents)
        .any(|parent| matches!(parent, ParentRef::External(hash) if stored.contains_key(hash)))
    {
        return Err(invalid());
    }
    let signatures = p
        .evidence
        .into_iter()
        .map(|(author, i, signature)| SignatureEvidence {
            author,
            commit: *commits[i].hash(),
            signature,
        })
        .collect();
    let item = if p.fragment {
        UnverifiedItem::Fragment {
            range: FragmentRange {
                head: p.head,
                boundary: p.boundary,
            },
            commits,
            signatures,
            checkpoints: BTreeSet::new(),
        }
    } else {
        let meta = commits.pop().ok_or_else(invalid)?;
        if *meta.hash() != p.head {
            return Err(Error::PayloadHashMismatch);
        }
        UnverifiedItem::Commit { meta, signatures }
    };
    Ok((item, stored))
}
