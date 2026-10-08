//! A canonical payload larger than 4 GiB is received with bounded memory.
//!
//! Slow outside release builds, so ignored by default:
//! `cargo test --release --test large -- --ignored --nocapture`
mod support;
use std::collections::BTreeSet;
use std::io;
use stockfoil::*;
use support::*;

/// Over 4 GiB by default; `STOCKFOIL_LARGE_BYTES` overrides it for quick runs.
fn payload_len() -> u64 {
    std::env::var("STOCKFOIL_LARGE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or((4 << 30) + 12_345)
}
const CHUNK: u64 = 1 << 20;

/// Deterministic payload bytes, generated rather than stored: little-endian
/// words derived from their index.
fn fill_payload(offset: u64, out: &mut [u8]) {
    for (j, b) in out.iter_mut().enumerate() {
        let i = offset + j as u64;
        let word = (i / 8).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        *b = word.to_le_bytes()[(i % 8) as usize];
    }
}

/// One stored item: a header plus a ChunkCodec body, generated on demand.
struct Synthetic {
    header: Vec<u8>,
    item: BlobHash,
    bound: Vec<PayloadLocation>,
}

impl Synthetic {
    fn body_len() -> u64 {
        payload_len() + 4 * payload_len().div_ceil(CHUNK) + 4
    }
    fn len(&self) -> u64 {
        self.header.len() as u64 + Self::body_len()
    }
    /// Generate item bytes at `offset`.
    fn fill(&self, mut offset: u64, out: &mut [u8]) -> usize {
        let end = (offset + out.len() as u64).min(self.len());
        let mut written = 0;
        while offset < end {
            let o = &mut out[written..];
            let n = if offset < self.header.len() as u64 {
                let h = &self.header[offset as usize..];
                let n = h.len().min(o.len());
                o[..n].copy_from_slice(&h[..n]);
                n
            } else {
                // Full chunk frames, then an optional partial frame, then a zero length.
                let body = offset - self.header.len() as u64;
                let full = payload_len() / CHUNK;
                let rem = payload_len() % CHUNK;
                let (chunk, within, len) = if body < full * (CHUNK + 4) {
                    (body / (CHUNK + 4), body % (CHUNK + 4), CHUNK)
                } else {
                    let tail = body - full * (CHUNK + 4);
                    if rem > 0 && tail < rem + 4 {
                        (full, tail, rem)
                    } else {
                        let at = tail - if rem > 0 { rem + 4 } else { 0 };
                        (full + 1, at, 0)
                    }
                };
                if within < 4 {
                    let bytes = (len as u32).to_le_bytes();
                    let n = ((4 - within) as usize).min(o.len());
                    o[..n].copy_from_slice(&bytes[within as usize..within as usize + n]);
                    n
                } else {
                    let n = ((len + 4 - within) as usize).min(o.len());
                    fill_payload(chunk * CHUNK + within - 4, &mut o[..n]);
                    n
                }
            };
            let n = n.min((end - offset) as usize);
            written += n;
            offset += n as u64;
        }
        written
    }
}

impl Storage for Synthetic {
    type Writer = io::Sink;
    fn create(&mut self) -> io::Result<io::Sink> {
        Ok(io::sink())
    }
    fn finish(&mut self, _: io::Sink) -> io::Result<BlobHash> {
        Err(io::Error::other("read-only"))
    }
    fn read_at(&mut self, blob: &BlobHash, offset: u64, out: &mut [u8]) -> io::Result<usize> {
        assert_eq!(*blob, self.item);
        Ok(self.fill(offset, out))
    }
    fn bind(&mut self, location: PayloadLocation) -> io::Result<PayloadId> {
        self.bound.push(location);
        Ok(self.bound.len() as PayloadId - 1)
    }
    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation> {
        Ok(self.bound[id as usize])
    }
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[test]
#[ignore = "hashes about 13 GiB; run in release"]
fn payload_over_four_gib_streams_with_bounded_memory() {
    let alice = key(1);
    // Commit identity requires the payload hash: stream it once.
    let mut builder = History::new().begin_commit(author(&alice), vec![]).unwrap();
    let mut buf = vec![0u8; CHUNK as usize];
    let mut offset = 0;
    while offset < payload_len() {
        let n = CHUNK.min(payload_len() - offset) as usize;
        fill_payload(offset, &mut buf[..n]);
        builder.update(&buf[..n]);
        offset += n as u64;
    }
    let prepared = builder.finish();
    assert_eq!(prepared.len, payload_len());
    let meta = BundleMeta {
        id: ItemId::Commit(prepared.hash),
        shape: FragmentShape {
            members: vec![MemberShape {
                author: author(&alice),
                parents: vec![],
            }],
        },
        evidence: vec![Evidence {
            author: author(&alice),
            member: 0,
            signature: sign(&alice, &prepared.hash),
        }],
    };
    let mut storage = Synthetic {
        header: wire::encode_header(&meta, &format()),
        item: BlobHash::from([0; 32]),
        bound: vec![],
    };
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0;
    loop {
        let n = storage.fill(offset, &mut buf);
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        offset += n as u64;
    }
    storage.item = BlobHash::from(<[u8; 32]>::from(hasher.finalize()));
    drop(buf);
    let before = peak_rss_kib();

    let item = storage.item;
    let mut graph = Graph::new(ChunkCodec::default(), storage);
    let receipt = graph.admit(item).unwrap();
    assert_eq!(receipt.members.len(), 1);
    assert!(graph.history().is_admitted(&prepared.hash));
    assert_eq!(
        graph
            .history()
            .heads(Heads::Admitted)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([prepared.hash])
    );
    if let (Some(before), Some(after)) = (before, peak_rss_kib()) {
        println!("peak RSS before {before} KiB, after {after} KiB");
        assert!(after < before + 64 * 1024, "admission used too much memory");
    }
}

/// The generator is checked against the real codec layout on a small scale.
#[test]
fn synthetic_layout_matches_the_chunk_codec() {
    const SMALL: u64 = CHUNK * 2 + 12_345;
    let mut payload = vec![0u8; SMALL as usize];
    fill_payload(0, &mut payload);
    let mut body = Vec::new();
    for chunk in payload.chunks(CHUNK as usize) {
        body.extend((chunk.len() as u32).to_le_bytes());
        body.extend(chunk);
    }
    body.extend(0u32.to_le_bytes());
    // The generator's first frames match the codec's for any payload length.
    let synthetic = Synthetic {
        header: vec![],
        item: BlobHash::from([0; 32]),
        bound: vec![],
    };
    let mut generated = vec![0u8; (CHUNK + 8) as usize];
    synthetic.fill(0, &mut generated);
    assert_eq!(generated[..], body[..generated.len()]);
    // And the tail of the real item: the partial frame and the terminator.
    let tail = Synthetic::body_len() - (payload_len() % CHUNK) - 8;
    let mut generated = vec![0u8; ((payload_len() % CHUNK) + 8) as usize];
    assert_eq!(synthetic.fill(tail, &mut generated), generated.len());
    assert_eq!(
        &generated[..4],
        &((payload_len() % CHUNK) as u32).to_le_bytes()
    );
    assert_eq!(&generated[generated.len() - 4..], &[0, 0, 0, 0]);
    let _ = encode_body;
}
