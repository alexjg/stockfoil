//! The register's bundle codec.
//!
//! Canonical payload (what a commit hash covers), one assignment:
//!
//! ```text
//! uleb(key.len) key uleb(value.len) value
//! ```
//!
//! Encoded body, for each member in metadata order:
//!
//! ```text
//! 0x00 uleb(key.len) key    a literal key, appended to the body's key dictionary
//! 0x01 uleb(index)          a key already in the dictionary
//! uleb(value.len) value
//! ```
//!
//! The dictionary belongs to one body, so a key written by many members of a
//! fragment is stored once. Both directions stream: keys are bounded by
//! [`MAX_KEY_BYTES`], the dictionary by fixed limits, and values pass through
//! in caller-sized pieces however large they are.
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use stockfoil::{
    BlobHash, BundleMeta, Codec, DecodeProgress, DecodeStatus, Decoder, EncodeProgress, Encoder,
    ExportItem, FormatId, Limits, PayloadId, PayloadLocation, Storage, wire,
};

/// Longest key an assignment may have.
pub const MAX_KEY_BYTES: usize = 1024;
const MAX_DICTIONARY_KEYS: usize = 4096;
const MAX_DICTIONARY_BYTES: usize = 256 * 1024;
const LITERAL: u8 = 0;
const REFERENCE: u8 = 1;
/// Encoder read size from storage.
const READ: usize = 4096;

pub const FORMAT: &str = "stockfoil/mv-register/v2";

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

pub(crate) fn put_uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// An unsigned LEB128 integer arriving one byte at a time.
#[derive(Clone, Default)]
struct Uleb {
    value: u64,
    shift: u32,
}

impl Uleb {
    fn push(&mut self, byte: u8) -> io::Result<Option<u64>> {
        if self.shift > 63 || (self.shift == 63 && byte > 1) {
            return Err(invalid("integer overflow"));
        }
        self.value |= u64::from(byte & 0x7f) << self.shift;
        if byte & 0x80 == 0 {
            return Ok(Some(std::mem::take(self).value));
        }
        self.shift += 7;
        Ok(None)
    }
}

fn admits(keys: usize, bytes: usize, key: &[u8]) -> bool {
    keys < MAX_DICTIONARY_KEYS && bytes + key.len() <= MAX_DICTIONARY_BYTES
}

/// The trusted register codec. Incoming items naming another format are
/// rejected by Stockfoil before any decoder runs.
#[derive(Clone)]
pub struct RegisterCodec {
    format: FormatId,
}

impl Default for RegisterCodec {
    fn default() -> Self {
        Self {
            format: FormatId::new(FORMAT).unwrap(),
        }
    }
}

impl RegisterCodec {
    pub fn format_id(&self) -> &FormatId {
        &self.format
    }
}

impl<S: Storage<Key = BlobHash>> Codec<S> for RegisterCodec {
    type Decoder = RegisterDecoder;
    type Encoder = RegisterEncoder;
    fn format(&self) -> &FormatId {
        &self.format
    }
    fn decoder(&self, meta: &Arc<BundleMeta>) -> io::Result<RegisterDecoder> {
        Ok(RegisterDecoder::new(meta.len()))
    }
    fn encoder(&self, item: &ExportItem) -> io::Result<RegisterEncoder> {
        Ok(RegisterEncoder::new(
            item.members.iter().map(|&(_, id)| id).collect(),
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Tag,
    KeyLen,
    Key,
    Reference,
    ValueLen,
    Value,
}

/// Encoded body to canonical members.
pub struct RegisterDecoder {
    members: usize,
    ended: usize,
    field: Field,
    uleb: Uleb,
    key: Vec<u8>,
    key_len: usize,
    remaining: u64,
    dictionary: Vec<Box<[u8]>>,
    dictionary_bytes: usize,
    /// Canonical prefix bytes (lengths and key) not yet output.
    pending: Vec<u8>,
    flushed: usize,
}

impl RegisterDecoder {
    pub fn new(members: usize) -> Self {
        Self {
            members,
            ended: 0,
            field: Field::Tag,
            uleb: Uleb::default(),
            key: Vec::new(),
            key_len: 0,
            remaining: 0,
            dictionary: Vec::new(),
            dictionary_bytes: 0,
            pending: Vec::new(),
            flushed: 0,
        }
    }

    fn byte(&mut self, byte: u8) -> io::Result<()> {
        match self.field {
            Field::Tag => {
                self.field = match byte {
                    LITERAL => Field::KeyLen,
                    REFERENCE => Field::Reference,
                    _ => return Err(invalid("unknown member tag")),
                }
            }
            Field::KeyLen => {
                if let Some(len) = self.uleb.push(byte)? {
                    if len > MAX_KEY_BYTES as u64 {
                        return Err(invalid("key too long"));
                    }
                    self.key.clear();
                    self.key_len = len as usize;
                    self.field = Field::Key;
                    if len == 0 {
                        self.literal_key();
                    }
                }
            }
            Field::Key => {
                self.key.push(byte);
                if self.key.len() == self.key_len {
                    self.literal_key();
                }
            }
            Field::Reference => {
                if let Some(index) = self.uleb.push(byte)? {
                    let key = usize::try_from(index)
                        .ok()
                        .and_then(|i| self.dictionary.get(i))
                        .ok_or_else(|| invalid("unknown key reference"))?;
                    self.key.clear();
                    self.key.extend_from_slice(key);
                    self.key_known();
                }
            }
            Field::ValueLen => {
                if let Some(len) = self.uleb.push(byte)? {
                    put_uleb(&mut self.pending, len);
                    self.remaining = len;
                    self.field = Field::Value;
                }
            }
            Field::Value => unreachable!("values are copied, not parsed"),
        }
        Ok(())
    }

    fn literal_key(&mut self) {
        if admits(self.dictionary.len(), self.dictionary_bytes, &self.key) {
            self.dictionary_bytes += self.key.len();
            self.dictionary.push(self.key.as_slice().into());
        }
        self.key_known();
    }

    fn key_known(&mut self) {
        put_uleb(&mut self.pending, self.key.len() as u64);
        self.pending.extend_from_slice(&self.key);
        self.field = Field::ValueLen;
    }
}

impl Decoder for RegisterDecoder {
    fn step(&mut self, input: &[u8], eof: bool, output: &mut [u8]) -> io::Result<DecodeProgress> {
        if output.is_empty() {
            return Err(invalid("no output capacity"));
        }
        let (mut consumed, mut produced) = (0, 0);
        let progress = |consumed, produced, status| {
            Ok(DecodeProgress {
                consumed,
                produced,
                status,
            })
        };
        loop {
            if self.flushed < self.pending.len() {
                let n = (self.pending.len() - self.flushed).min(output.len() - produced);
                output[produced..produced + n]
                    .copy_from_slice(&self.pending[self.flushed..self.flushed + n]);
                produced += n;
                self.flushed += n;
                if self.flushed < self.pending.len() {
                    return progress(consumed, produced, DecodeStatus::Working);
                }
                self.pending.clear();
                self.flushed = 0;
            }
            if self.field == Field::Value {
                if self.remaining == 0 {
                    self.ended += 1;
                    self.field = Field::Tag;
                    return progress(consumed, produced, DecodeStatus::EndMember);
                }
                if produced == output.len() {
                    return progress(consumed, produced, DecodeStatus::Working);
                }
                if consumed < input.len() {
                    let n = usize::try_from(self.remaining)
                        .unwrap_or(usize::MAX)
                        .min(input.len() - consumed)
                        .min(output.len() - produced);
                    output[produced..produced + n].copy_from_slice(&input[consumed..consumed + n]);
                    consumed += n;
                    produced += n;
                    self.remaining -= n as u64;
                    continue;
                }
            } else if self.field == Field::Tag && self.ended == self.members {
                if consumed < input.len() {
                    return Err(invalid("bytes after the last member"));
                }
                return progress(consumed, produced, DecodeStatus::Finished);
            } else if consumed < input.len() {
                consumed += 1;
                self.byte(input[consumed - 1])?;
                continue;
            }
            if eof {
                return Err(invalid("truncated body"));
            }
            return progress(consumed, produced, DecodeStatus::NeedInput);
        }
    }
}

/// Canonical bytes of one stored payload, read with bounded buffers.
pub(crate) enum PayloadReader {
    Canonical { blob: BlobHash, offset: u64 },
    Member(Box<MemberReader>),
}

/// Decodes member `target` of a stored item, skipping earlier members. It can
/// move on to a later member of the same item without starting over.
pub(crate) struct MemberReader {
    item: BlobHash,
    target: usize,
    member: usize,
    /// Read position in the item; `None` until the header has been skipped.
    offset: Option<u64>,
    decoder: RegisterDecoder,
    input: Vec<u8>,
    start: usize,
    eof: bool,
    scratch: Vec<u8>,
    done: bool,
}

impl PayloadReader {
    pub fn open(location: PayloadLocation<BlobHash>) -> Self {
        match location {
            PayloadLocation::Canonical(blob) => Self::Canonical { blob, offset: 0 },
            PayloadLocation::Member { item, index } => Self::Member(Box::new(MemberReader {
                item,
                target: index,
                member: 0,
                offset: None,
                // Member extraction stops at `target`, so the count is irrelevant.
                decoder: RegisterDecoder::new(usize::MAX),
                input: Vec::with_capacity(READ),
                start: 0,
                eof: false,
                scratch: vec![0; READ],
                done: false,
            })),
        }
    }

    /// Reuse this reader for `location` if it is a later member of the item
    /// being decoded; otherwise open a new one.
    pub fn reopen(self, location: PayloadLocation<BlobHash>) -> Self {
        match (self, location) {
            (Self::Member(mut r), PayloadLocation::Member { item, index })
                if r.item == item && r.done && index >= r.member =>
            {
                r.target = index;
                r.done = false;
                Self::Member(r)
            }
            _ => Self::open(location),
        }
    }

    /// Fill `out` with the next canonical bytes; `Ok(0)` at the end.
    pub fn read<S: Storage<Key = BlobHash>>(
        &mut self,
        storage: &mut S,
        out: &mut [u8],
    ) -> io::Result<usize> {
        match self {
            Self::Canonical { blob, offset } => {
                let n = storage.read_at(blob, *offset, out)?;
                *offset += n as u64;
                Ok(n)
            }
            Self::Member(r) => r.read(storage, out),
        }
    }
}

impl MemberReader {
    fn read<S: Storage<Key = BlobHash>>(
        &mut self,
        storage: &mut S,
        out: &mut [u8],
    ) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let offset = match self.offset {
            Some(offset) => offset,
            None => {
                // The magic and header length fit in 16 bytes; skip the header.
                let mut prefix = [0u8; 16];
                let n = storage.read_at(&self.item, 0, &mut prefix)?;
                let len = wire::header_len(&prefix[..n], &Limits::default())
                    .map_err(io::Error::other)?
                    .ok_or_else(|| invalid("truncated item"))?;
                *self.offset.insert(len as u64)
            }
        };
        let mut offset = offset;
        loop {
            if self.done {
                return Ok(0);
            }
            if self.start == self.input.len() && !self.eof {
                self.input.resize(READ, 0);
                let n = storage.read_at(&self.item, offset, &mut self.input)?;
                self.input.truncate(n);
                self.start = 0;
                offset += n as u64;
                self.offset = Some(offset);
                self.eof = n == 0;
            }
            let skipping = self.member < self.target;
            let output = if skipping {
                &mut self.scratch[..]
            } else {
                &mut *out
            };
            let p = self
                .decoder
                .step(&self.input[self.start..], self.eof, output)?;
            self.start += p.consumed;
            match p.status {
                DecodeStatus::EndMember => {
                    self.done = self.member == self.target;
                    self.member += 1;
                }
                DecodeStatus::Finished => return Err(invalid("member not found")),
                DecodeStatus::NeedInput | DecodeStatus::Working => {}
            }
            if !skipping && (p.produced > 0 || self.done) {
                return Ok(p.produced);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Open,
    KeyLen,
    Key,
    ValueLen,
    Value,
    /// The value ended; the canonical payload must end too.
    End,
}

/// Canonical members (read from storage) to an encoded body.
pub struct RegisterEncoder {
    members: Vec<PayloadId>,
    next: usize,
    source: Option<PayloadReader>,
    input: Vec<u8>,
    start: usize,
    stage: Stage,
    uleb: Uleb,
    key: Vec<u8>,
    key_len: usize,
    remaining: u64,
    dictionary: HashMap<Box<[u8]>, u64>,
    dictionary_bytes: usize,
    pending: Vec<u8>,
    flushed: usize,
}

impl RegisterEncoder {
    pub fn new(members: Vec<PayloadId>) -> Self {
        Self {
            members,
            next: 0,
            source: None,
            input: Vec::with_capacity(READ),
            start: 0,
            stage: Stage::Open,
            uleb: Uleb::default(),
            key: Vec::new(),
            key_len: 0,
            remaining: 0,
            dictionary: HashMap::new(),
            dictionary_bytes: 0,
            pending: Vec::new(),
            flushed: 0,
        }
    }

    /// Make canonical input available. `false` at the end of the payload.
    fn fill<S: Storage<Key = BlobHash>>(&mut self, storage: &mut S) -> io::Result<bool> {
        if self.start < self.input.len() {
            return Ok(true);
        }
        self.input.resize(READ, 0);
        let n = self
            .source
            .as_mut()
            .unwrap()
            .read(storage, &mut self.input)?;
        self.input.truncate(n);
        self.start = 0;
        Ok(n > 0)
    }

    fn key_known(&mut self) {
        if let Some(&index) = self.dictionary.get(self.key.as_slice()) {
            self.pending.push(REFERENCE);
            put_uleb(&mut self.pending, index);
        } else {
            self.pending.push(LITERAL);
            put_uleb(&mut self.pending, self.key.len() as u64);
            self.pending.extend_from_slice(&self.key);
            if admits(self.dictionary.len(), self.dictionary_bytes, &self.key) {
                self.dictionary_bytes += self.key.len();
                let index = self.dictionary.len() as u64;
                self.dictionary.insert(self.key.as_slice().into(), index);
            }
        }
        self.stage = Stage::ValueLen;
    }
}

impl<S: Storage<Key = BlobHash>> Encoder<S> for RegisterEncoder {
    fn step(&mut self, storage: &mut S, output: &mut [u8]) -> io::Result<EncodeProgress> {
        let mut produced = 0;
        loop {
            if self.flushed < self.pending.len() {
                let n = (self.pending.len() - self.flushed).min(output.len() - produced);
                output[produced..produced + n]
                    .copy_from_slice(&self.pending[self.flushed..self.flushed + n]);
                produced += n;
                self.flushed += n;
                if self.flushed < self.pending.len() {
                    return Ok(EncodeProgress {
                        produced,
                        finished: false,
                    });
                }
                self.pending.clear();
                self.flushed = 0;
            }
            match self.stage {
                Stage::Open => {
                    let Some(&id) = self.members.get(self.next) else {
                        return Ok(EncodeProgress {
                            produced,
                            finished: true,
                        });
                    };
                    let location = storage.location(id)?;
                    self.source = Some(match self.source.take() {
                        Some(reader) => reader.reopen(location),
                        None => PayloadReader::open(location),
                    });
                    self.input.clear();
                    self.start = 0;
                    self.stage = Stage::KeyLen;
                }
                Stage::Value => {
                    if self.remaining == 0 {
                        self.stage = Stage::End;
                        continue;
                    }
                    if produced == output.len() {
                        return Ok(EncodeProgress {
                            produced,
                            finished: false,
                        });
                    }
                    if !self.fill(storage)? {
                        return Err(invalid("payload shorter than its value length"));
                    }
                    let n = usize::try_from(self.remaining)
                        .unwrap_or(usize::MAX)
                        .min(self.input.len() - self.start)
                        .min(output.len() - produced);
                    output[produced..produced + n]
                        .copy_from_slice(&self.input[self.start..self.start + n]);
                    self.start += n;
                    produced += n;
                    self.remaining -= n as u64;
                }
                Stage::End => {
                    if self.fill(storage)? {
                        return Err(invalid("bytes after the value"));
                    }
                    self.next += 1;
                    self.stage = Stage::Open;
                }
                Stage::KeyLen | Stage::Key | Stage::ValueLen => {
                    if !self.fill(storage)? {
                        return Err(invalid("truncated payload"));
                    }
                    let byte = self.input[self.start];
                    self.start += 1;
                    match self.stage {
                        Stage::KeyLen => {
                            if let Some(len) = self.uleb.push(byte)? {
                                if len > MAX_KEY_BYTES as u64 {
                                    return Err(invalid("key too long"));
                                }
                                self.key.clear();
                                self.key_len = len as usize;
                                self.stage = Stage::Key;
                                if len == 0 {
                                    self.key_known();
                                }
                            }
                        }
                        Stage::Key => {
                            self.key.push(byte);
                            if self.key.len() == self.key_len {
                                self.key_known();
                            }
                        }
                        _ => {
                            if let Some(len) = self.uleb.push(byte)? {
                                put_uleb(&mut self.pending, len);
                                self.remaining = len;
                                self.stage = Stage::Value;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Assignment;
    use stockfoil::MemoryStorage;

    fn canonical(pairs: &[(&str, &str)]) -> Vec<Vec<u8>> {
        pairs
            .iter()
            .map(|(k, v)| Assignment::new(*k, *v).encode())
            .collect()
    }

    /// Encode payloads stored as canonical blobs, `out` bytes per step.
    fn encode(storage: &mut MemoryStorage, members: &[Vec<u8>], out: usize) -> Vec<u8> {
        let ids = members
            .iter()
            .map(|m| {
                let blob = storage.insert(m.clone());
                storage.bind(PayloadLocation::Canonical(blob)).unwrap()
            })
            .collect();
        let mut encoder = RegisterEncoder::new(ids);
        let mut body = Vec::new();
        let mut buf = vec![0u8; out];
        loop {
            let p = encoder.step(storage, &mut buf).unwrap();
            body.extend_from_slice(&buf[..p.produced]);
            if p.finished {
                return body;
            }
            assert!(p.produced > 0);
        }
    }

    /// Decode `body`, offering `input` bytes and `output` capacity per step.
    fn decode(
        body: &[u8],
        members: usize,
        input: usize,
        output: usize,
    ) -> io::Result<Vec<Vec<u8>>> {
        let mut decoder = RegisterDecoder::new(members);
        let mut out = vec![Vec::new()];
        let mut buf = vec![0u8; output];
        let mut pos = 0;
        let mut window = 0;
        loop {
            if pos == window {
                window = (pos + input).min(body.len());
            }
            let eof = window == body.len();
            let p = decoder.step(&body[pos..window], eof, &mut buf)?;
            pos += p.consumed;
            out.last_mut()
                .unwrap()
                .extend_from_slice(&buf[..p.produced]);
            match p.status {
                DecodeStatus::EndMember => out.push(Vec::new()),
                DecodeStatus::Finished => {
                    out.pop();
                    return Ok(out);
                }
                DecodeStatus::NeedInput => assert_eq!(pos, window),
                DecodeStatus::Working => assert!(p.consumed + p.produced > 0),
            }
        }
    }

    #[test]
    fn round_trips_with_tiny_buffers() {
        let big = "v".repeat(10_000);
        let members = canonical(&[("title", "a"), ("", ""), ("title", &big), ("x", "")]);
        for out in [1, 2, 7, 4096] {
            let mut storage = MemoryStorage::new();
            let body = encode(&mut storage, &members, out);
            assert_eq!(body, encode(&mut MemoryStorage::new(), &members, 64 * 1024));
            for (input, output) in [(1, 1), (3, 5), (64, 1), (1, 64), (1 << 16, 1 << 16)] {
                assert_eq!(
                    decode(&body, members.len(), input, output).unwrap(),
                    members
                );
            }
        }
    }

    #[test]
    fn keys_repeated_across_members_are_stored_once() {
        let key = "a-rather-long-shared-key";
        let pairs: Vec<_> = (0..100)
            .map(|i| (key, if i % 2 == 0 { "x" } else { "y" }))
            .collect();
        let members = canonical(&pairs);
        let body = encode(&mut MemoryStorage::new(), &members, 4096);
        let canonical_len: usize = members.iter().map(Vec::len).sum();
        assert_eq!(body.len(), (1 + 1 + key.len() + 2) + 99 * (1 + 1 + 2));
        assert!(body.len() * 5 < canonical_len);
    }

    #[test]
    fn rejects_malformed_and_truncated_bodies() {
        let members = canonical(&[("k", "v"), ("k", "w")]);
        let body = encode(&mut MemoryStorage::new(), &members, 4096);
        assert_eq!(body, [0, 1, b'k', 1, b'v', 1, 0, 1, b'w']);
        for len in 0..body.len() {
            assert!(decode(&body[..len], 2, 4, 4).is_err(), "prefix {len}");
        }
        let mut trailing = body.clone();
        trailing.push(0);
        assert!(decode(&trailing, 2, 4, 4).is_err());
        assert!(decode(&body, 3, 4, 4).is_err());
        for bad in [
            vec![2, 0, 0],                                  // unknown tag
            vec![1, 0, 0],                                  // reference to an empty dictionary
            vec![0, 0x81, 0x08, 0],                         // key longer than MAX_KEY_BYTES
            [vec![0, 0], vec![0xff; 10], vec![1]].concat(), // value length overflow
        ] {
            assert!(decode(&bad, 1, 16, 16).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn member_reader_extracts_members_in_sequence() {
        let members = canonical(&[("a", "1"), ("b", "22"), ("a", "333")]);
        let mut storage = MemoryStorage::new();
        let body = encode(&mut storage, &members, 4096);
        // A stored item is a wire header followed by the body.
        let meta = BundleMeta {
            id: stockfoil::ItemId::Commit(stockfoil::CommitHash::from([1; 32])),
            shape: stockfoil::FragmentShape { members: vec![] },
            evidence: vec![],
        };
        let mut item = wire::encode_header(&meta, &FormatId::new(FORMAT).unwrap());
        item.extend_from_slice(&body);
        let item = storage.insert(item);
        let mut reader: Option<PayloadReader> = None;
        for (index, expected) in members.iter().enumerate().skip(1) {
            let location = PayloadLocation::Member { item, index };
            let mut r = match reader.take() {
                Some(r) => r.reopen(location),
                None => PayloadReader::open(location),
            };
            let mut got = Vec::new();
            let mut buf = [0u8; 1];
            while r.read(&mut storage, &mut buf).unwrap() == 1 {
                got.push(buf[0]);
            }
            assert_eq!(&got, expected);
            reader = Some(r);
        }
    }
}
