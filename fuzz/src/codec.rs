//! Keys, the commit identity rule, and the test codec.
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

/// Check generated evidence independently of stockfoil's verifier. A random
/// public key can alias a known signer, so "signed with the wrong key" is not
/// by itself proof of invalidity.
pub fn signature_is_valid(author: &Author, hash: &CommitHash, signature: &Signature) -> bool {
    ed25519_dalek::VerifyingKey::from_bytes(author.as_array()).is_ok_and(|key| {
        key.verify_strict(
            hash.as_bytes(),
            &ed25519_dalek::Signature::from(<[u8; 64]>::from(*signature)),
        )
        .is_ok()
    })
}

#[cfg(test)]
mod signature_tests {
    use super::*;

    #[test]
    fn arbitrary_author_can_alias_the_supposedly_wrong_signing_key() {
        let signer = key(3);
        let stranger = Author::try_from(signer.verifying_key().as_bytes().as_slice()).unwrap();
        let hash = CommitHash::from([42; 32]);
        assert!(signature_is_valid(&stranger, &hash, &sign(&signer, &hash)));
        assert!(!signature_is_valid(&stranger, &hash, &sign(&key(1), &hash)));
        assert!(!signature_is_valid(
            &stranger,
            &CommitHash::from([43; 32]),
            &sign(&signer, &hash),
        ));
    }
}

pub fn format() -> FormatId {
    FormatId::new("stockfoil-test/chunks-v1").unwrap()
}

/// Sedimentree depth: leading zero bytes of a commit hash.
pub fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes().iter().take_while(|b| **b == 0).count()
}

/// The commit identity rule, reimplemented independently.
pub fn commit_hash(parents: &[CommitHash], author: &Author, payload: &[u8]) -> CommitHash {
    commit_hash_with_payload_hash(parents, author, blake3::hash(payload).as_bytes())
}

pub fn commit_hash_with_payload_hash(
    parents: &[CommitHash],
    author: &Author,
    payload_hash: &[u8; 32],
) -> CommitHash {
    let mut bytes = Vec::with_capacity(4 + 32 * parents.len() + 64);
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
    bytes.extend(payload_hash);
    CommitHash::from(blake3::hash(&bytes))
}

/// Extend `prefix` with a counter until the commit reaches depth `level`.
pub fn grind(author: &Author, parents: &[CommitHash], level: usize, prefix: &[u8]) -> Vec<u8> {
    for i in 0u64.. {
        let mut payload = prefix.to_vec();
        payload.extend(i.to_le_bytes());
        if depth(&commit_hash(parents, author, &payload)) >= level {
            return payload;
        }
    }
    unreachable!()
}

/// Encode canonical member payloads as a ChunkCodec body, one chunk per member.
pub fn chunk_body<'a>(payloads: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for p in payloads {
        if !p.is_empty() {
            out.extend((p.len() as u32).to_le_bytes());
            out.extend(p);
        }
        out.extend(0u32.to_le_bytes());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeProgress {
    pub consumed: usize,
    pub produced: usize,
    pub status: DecodeStatus,
}

#[derive(Clone)]
pub struct ChunkCodec {
    format: FormatId,
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

#[derive(Clone)]
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

fn sync(from: &mut TestDocument, to: &mut TestDocument, cursor: &ExportCursor) {
    let export = from.export(cursor);
    for i in 0..export.len() {
        to.receive(&export.item(i).unwrap().1).unwrap();
    }
}
