//! Shared test fixtures: deterministic keys and a test codec.
#![allow(dead_code)]
use ed25519_dalek::SigningKey;
use stockfoil::DecodeStatus;
use stockfoil::*;

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn author(key: &SigningKey) -> Author {
    Author::from(key)
}

pub fn sign(key: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(ed25519_dalek::Signer::sign(key, hash.as_bytes()))
}

pub fn format() -> FormatId {
    FormatId::new("stockfoil-test/chunks-v1").unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeProgress {
    pub consumed: usize,
    pub produced: usize,
    pub status: DecodeStatus,
}

/// Body: for each member, `(u32 LE length, bytes)*` chunks ending with a zero
/// length.
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

impl Codec for ChunkCodec {
    type Error = &'static str;

    fn format(&self) -> &FormatId {
        &self.format
    }

    fn encode(&self, _meta: &BundleMeta, payloads: &[Bytes]) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        for p in payloads {
            for chunk in p.chunks(self.chunk.max(1)) {
                out.extend((chunk.len() as u32).to_le_bytes());
                out.extend_from_slice(chunk);
            }
            out.extend(0u32.to_le_bytes());
        }
        Ok(out)
    }

    fn decode(&self, meta: &BundleMeta, body: &[u8]) -> Result<Vec<Bytes>, &'static str> {
        let mut decoder = ChunkDecoder::new(meta.len());
        let mut members = Vec::with_capacity(meta.len());
        let mut current = Vec::new();
        let mut buf = vec![0u8; 4096];
        let mut pos = 0;
        loop {
            let p = decoder.step(&body[pos..], true, &mut buf)?;
            pos += p.consumed;
            current.extend_from_slice(&buf[..p.produced]);
            match p.status {
                DecodeStatus::EndMember => members.push(Bytes::from(std::mem::take(&mut current))),
                DecodeStatus::Finished => return Ok(members),
                DecodeStatus::NeedInput => return Err("truncated body"),
                DecodeStatus::Working => {}
            }
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

    pub fn step(
        &mut self,
        input: &[u8],
        eof: bool,
        output: &mut [u8],
    ) -> Result<DecodeProgress, &'static str> {
        let mut consumed = 0;
        let mut produced = 0;
        loop {
            if self.ended == self.members {
                if consumed < input.len() {
                    return Err("trailing body bytes");
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
                    return Err("truncated body");
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

pub type TestDocument = Document<ChunkCodec>;

pub fn document() -> TestDocument {
    document_for(&key(1))
}
pub fn document_for(key: &SigningKey) -> TestDocument {
    Document::new(ChunkCodec::default(), author(key))
}

/// A different author's write is made on a separate replica and received.
pub fn signed(
    g: &mut TestDocument,
    key: &SigningKey,
    parents: Vec<CommitHash>,
    payload: &[u8],
) -> CommitHash {
    if g.author() != author(key) {
        let mut other = document_for(key);
        sync(g, &mut other, &ExportCursor::new());
        let hash = signed(&mut other, key, parents, payload);
        sync(&mut other, g, &ExportCursor::new());
        return hash;
    }
    let draft = g.commit(payload, Some(parents)).unwrap();
    g.attach_signature(&draft, sign(key, &draft.hash)).unwrap();
    draft.hash
}

/// A payload whose commit (by `key`, on `parents`) has at least `level` depth.
pub fn payload_at_depth(
    key: &SigningKey,
    parents: &[CommitHash],
    level: usize,
    tag: &str,
) -> Vec<u8> {
    for i in 0u64.. {
        let payload = format!("{tag}-{i}").into_bytes();
        let hash = commit_hash(parents, &author(key), &payload);
        if hash.as_bytes().iter().take_while(|b| **b == 0).count() >= level {
            return payload;
        }
    }
    unreachable!()
}

/// The commit identity rule, reimplemented independently for tests.
pub fn commit_hash(parents: &[CommitHash], author: &Author, payload: &[u8]) -> CommitHash {
    let mut bytes = Vec::new();
    let mut n = parents.len() as u64;
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
/// arrive before their dependencies are queued in `to` and admitted once their
/// dependencies arrive.
pub fn sync(from: &mut TestDocument, to: &mut TestDocument, cursor: &ExportCursor) -> ExportCursor {
    let mut plan = from.export(cursor);
    for i in 0..plan.len() {
        let bytes = plan.item(i).unwrap().1;
        to.receive(&bytes).unwrap();
        plan.acknowledge(i).unwrap();
    }
    assert_eq!(to.ledger().parked(), 0, "export plan left an item waiting");
    plan.cursor()
}
