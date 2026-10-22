//! Shared test fixtures: deterministic keys and a small streaming codec.
#![allow(dead_code)]
use ed25519_dalek::{Signer, SigningKey};
use std::io;
use std::sync::Arc;
use stockfoil::*;

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn author(key: &SigningKey) -> Author {
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

pub fn sign(key: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(key.sign(hash.as_bytes()).to_bytes())
}

pub fn format() -> FormatId {
    FormatId::new("stockfoil-test/chunks-v1").unwrap()
}

/// Body: for each member, `(u32 LE length, bytes)*` chunks ending with a zero
/// length. Streams with bounded state in both directions.
#[derive(Clone)]
pub struct ChunkCodec {
    format: FormatId,
    /// Largest chunk the encoder writes.
    pub chunk: usize,
}

impl Default for ChunkCodec {
    fn default() -> Self {
        Self {
            format: format(),
            chunk: 4096,
        }
    }
}

pub struct ChunkDecoder {
    members: usize,
    ended: usize,
    length: [u8; 4],
    have: usize,
    remaining: usize,
    in_chunk: bool,
}

impl ChunkDecoder {
    pub fn new(members: usize) -> Self {
        Self {
            members,
            ended: 0,
            length: [0; 4],
            have: 0,
            remaining: 0,
            in_chunk: false,
        }
    }
}

impl Decoder for ChunkDecoder {
    fn step(&mut self, input: &[u8], eof: bool, output: &mut [u8]) -> io::Result<DecodeProgress> {
        let mut consumed = 0;
        let mut produced = 0;
        loop {
            if self.ended == self.members {
                if consumed < input.len() {
                    return Err(io::Error::other("trailing body bytes"));
                }
                return Ok(DecodeProgress {
                    consumed,
                    produced,
                    status: DecodeStatus::Finished,
                });
            }
            if self.in_chunk {
                let n = self
                    .remaining
                    .min(input.len() - consumed)
                    .min(output.len() - produced);
                output[produced..produced + n].copy_from_slice(&input[consumed..consumed + n]);
                consumed += n;
                produced += n;
                self.remaining -= n;
                if self.remaining == 0 {
                    self.in_chunk = false;
                    continue;
                }
                if produced == output.len() {
                    return Ok(DecodeProgress {
                        consumed,
                        produced,
                        status: DecodeStatus::Working,
                    });
                }
            } else {
                let n = (4 - self.have).min(input.len() - consumed);
                self.length[self.have..self.have + n]
                    .copy_from_slice(&input[consumed..consumed + n]);
                self.have += n;
                consumed += n;
                if self.have == 4 {
                    self.have = 0;
                    let len = u32::from_le_bytes(self.length) as usize;
                    if len == 0 {
                        self.ended += 1;
                        return Ok(DecodeProgress {
                            consumed,
                            produced,
                            status: DecodeStatus::EndMember,
                        });
                    }
                    self.remaining = len;
                    self.in_chunk = true;
                    continue;
                }
            }
            if consumed == input.len() {
                if eof {
                    return Err(io::Error::other("truncated body"));
                }
                return Ok(DecodeProgress {
                    consumed,
                    produced,
                    status: DecodeStatus::NeedInput,
                });
            }
        }
    }
}

/// Reads one payload's canonical bytes from storage, decoding a stored item
/// when the payload is a member of one.
pub struct PayloadSource<K> {
    location: PayloadLocation<K>,
    offset: u64,
    decoder: Option<(ChunkDecoder, usize)>,
    pending: Vec<u8>,
    pending_start: usize,
    done: bool,
}

impl<K: Clone> PayloadSource<K> {
    pub fn new<S: Storage<Key = K>>(storage: &S, id: PayloadId) -> io::Result<Self> {
        Ok(Self {
            location: storage.location(id)?,
            offset: 0,
            decoder: None,
            pending: Vec::new(),
            pending_start: 0,
            done: false,
        })
    }

    /// Fill `out` with the next canonical bytes; `Ok(0)` at the end.
    pub fn read<S: Storage<Key = K>>(
        &mut self,
        storage: &mut S,
        out: &mut [u8],
    ) -> io::Result<usize> {
        match self.location.clone() {
            PayloadLocation::Canonical(blob) => {
                let n = storage.read_at(&blob, self.offset, out)?;
                self.offset += n as u64;
                Ok(n)
            }
            PayloadLocation::Member { item, index } => loop {
                if self.pending_start < self.pending.len() {
                    let n = out.len().min(self.pending.len() - self.pending_start);
                    out[..n]
                        .copy_from_slice(&self.pending[self.pending_start..self.pending_start + n]);
                    self.pending_start += n;
                    return Ok(n);
                }
                if self.done {
                    return Ok(0);
                }
                if self.decoder.is_none() {
                    let mut prefix = Vec::new();
                    let mut buf = [0u8; 4096];
                    let len = loop {
                        if let Some(len) = header_len(&prefix, &Limits::default())
                            .map_err(io::Error::other)?
                            && prefix.len() >= len
                        {
                            break len;
                        }
                        let n = storage.read_at(&item, prefix.len() as u64, &mut buf)?;
                        if n == 0 {
                            return Err(io::Error::other("truncated item"));
                        }
                        prefix.extend_from_slice(&buf[..n]);
                    };
                    self.offset = len as u64;
                    self.decoder = Some((ChunkDecoder::new(index + 1), 0));
                }
                let mut input = [0u8; 4096];
                let n = storage.read_at(&item, self.offset, &mut input)?;
                let (decoder, member) = self.decoder.as_mut().unwrap();
                let mut start = 0;
                let mut output = vec![0u8; 4096];
                loop {
                    let p = decoder.step(&input[start..n], n == 0, &mut output)?;
                    start += p.consumed;
                    if *member == index {
                        self.pending.extend_from_slice(&output[..p.produced]);
                    }
                    match p.status {
                        DecodeStatus::EndMember => {
                            if *member == index {
                                self.done = true;
                            }
                            *member += 1;
                        }
                        DecodeStatus::Finished => self.done = true,
                        DecodeStatus::NeedInput => break,
                        DecodeStatus::Working => {}
                    }
                    if self.done {
                        break;
                    }
                }
                self.offset += start as u64;
                if self.pending.is_empty() && !self.done && n == 0 {
                    return Err(io::Error::other("member not found"));
                }
            },
        }
    }
}

pub struct ChunkEncoder<K> {
    members: Vec<PayloadId>,
    next: usize,
    source: Option<PayloadSource<K>>,
    chunk: usize,
}

impl<S: Storage> Encoder<S> for ChunkEncoder<S::Key> {
    fn step(&mut self, storage: &mut S, output: &mut [u8]) -> io::Result<EncodeProgress> {
        let mut produced = 0;
        while output.len() - produced >= 5 {
            if self.source.is_none() {
                if self.next == self.members.len() {
                    return Ok(EncodeProgress {
                        produced,
                        finished: true,
                    });
                }
                self.source = Some(PayloadSource::new(storage, self.members[self.next])?);
            }
            let room = (output.len() - produced - 4).min(self.chunk);
            let n = self
                .source
                .as_mut()
                .unwrap()
                .read(storage, &mut output[produced + 4..produced + 4 + room])?;
            output[produced..produced + 4].copy_from_slice(&(n as u32).to_le_bytes());
            produced += 4 + n;
            if n == 0 {
                self.source = None;
                self.next += 1;
            }
        }
        Ok(EncodeProgress {
            produced,
            finished: false,
        })
    }
}

impl<S: Storage> Codec<S> for ChunkCodec {
    type Decoder = ChunkDecoder;
    type Encoder = ChunkEncoder<S::Key>;
    fn format(&self) -> &FormatId {
        &self.format
    }
    fn decoder(&self, meta: &Arc<BundleMeta>) -> io::Result<ChunkDecoder> {
        Ok(ChunkDecoder::new(meta.len()))
    }
    fn encoder(&self, item: &ExportItem) -> io::Result<ChunkEncoder<S::Key>> {
        Ok(ChunkEncoder {
            members: item.members.iter().map(|(_, id)| *id).collect(),
            next: 0,
            source: None,
            chunk: self.chunk,
        })
    }
}

pub type TestGraph = Graph<ChunkCodec, MemoryStorage>;

pub fn graph() -> TestGraph {
    Graph::new(ChunkCodec::default(), MemoryStorage::new())
}

/// Commit and sign `payload` on `parents`; returns the hash.
pub fn signed(
    g: &mut TestGraph,
    key: &SigningKey,
    parents: Vec<CommitHash>,
    payload: &[u8],
) -> CommitHash {
    let prepared = g.commit(author(key), parents, payload).unwrap();
    g.sign(author(key), prepared.hash, sign(key, &prepared.hash))
        .unwrap();
    prepared.hash
}

/// A payload whose commit (by `key`, on `parents`) has at least `level` depth.
pub fn payload_at_depth(
    key: &SigningKey,
    parents: &[CommitHash],
    level: usize,
    tag: &str,
) -> Vec<u8> {
    let history = History::new();
    for i in 0u64.. {
        let payload = format!("{tag}-{i}").into_bytes();
        // Hash without delivering: parents need not exist in a scratch history.
        let hash = commit_hash(parents, &author(key), &payload);
        if hash.as_bytes().iter().take_while(|b| **b == 0).count() >= level {
            let _ = &history;
            return payload;
        }
    }
    unreachable!()
}

/// The commit identity rule, reimplemented independently for tests.
pub fn commit_hash(parents: &[CommitHash], author: &Author, payload: &[u8]) -> CommitHash {
    let mut bytes = Vec::new();
    let mut n = parents.len() as u64;
    // J(n): one byte below 248; otherwise a big-endian offset form.
    if n <= 247 {
        bytes.push(n as u8);
    } else if n <= 503 {
        bytes.extend([0xf8, (n - 248) as u8]);
    } else {
        n -= 504;
        bytes.push(0xf9);
        bytes.extend(&(n as u16).to_be_bytes());
    }
    for p in parents {
        bytes.extend(p.as_bytes());
    }
    bytes.extend(author.as_slice());
    bytes.extend(blake3::hash(payload).as_bytes());
    CommitHash::from(blake3::hash(&bytes))
}

/// Export everything `from` has that `cursor` lacks into `to`. Items that
/// arrive before their dependencies stay stored and are admitted afterwards;
/// only admitted items are acknowledged.
pub fn sync(from: &mut TestGraph, to: &mut TestGraph, cursor: &ExportCursor) -> ExportCursor {
    let mut plan = from.export(cursor);
    let mut waiting = vec![];
    for i in 0..plan.len() {
        let mut bytes = Vec::new();
        from.write_item(&plan, i, &mut bytes).unwrap();
        match to.receive(bytes.as_slice()) {
            Ok(_) => plan.acknowledge(i).unwrap(),
            Err(Error::MissingDependency(_)) => waiting.push((i, BlobHash::of_contents(&bytes))),
            Err(e) => panic!("{e}"),
        }
    }
    while !waiting.is_empty() {
        let before = waiting.len();
        waiting.retain(|(i, item)| match to.admit(item) {
            Ok(_) => {
                plan.acknowledge(*i).unwrap();
                false
            }
            Err(Error::MissingDependency(_)) => true,
            Err(e) => panic!("{e}"),
        });
        assert!(
            waiting.len() < before,
            "export plan has an unsatisfiable dependency"
        );
    }
    plan.cursor()
}

/// A `ChunkCodec` body for these canonical member payloads.
pub fn encode_body(payloads: &[&[u8]]) -> Vec<u8> {
    let mut body = Vec::new();
    for p in payloads {
        if !p.is_empty() {
            body.extend((p.len() as u32).to_le_bytes());
            body.extend(*p);
        }
        body.extend(0u32.to_le_bytes());
    }
    body
}


