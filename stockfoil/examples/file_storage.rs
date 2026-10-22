//! A file-backed `Storage`, a large signed payload, and a bounded-memory
//! transfer between two graphs.
//!
//! Run with: `cargo run --example file_storage [MiB]` (default 64).
//!
//! - Blobs are files named by their BLAKE3 hash, written to a temporary file,
//!   fsynced, then renamed into place. (Any naming would do: payloads read
//!   back are checked against their commits, not against storage keys.)
//! - Payload IDs are positions in an append-only index of fixed-size records,
//!   so the receiver can reopen its storage and still resolve them.
//! - The codec frames each member as `(u32 LE length, bytes)*` ending with an
//!   empty chunk. Payloads stream through 64 KiB buffers; none is held whole.
use ed25519_dalek::{Signer, SigningKey};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use stockfoil::*;

// ---- storage ----

struct FileStorage {
    blobs: PathBuf,
    index: File,
    locations: Vec<PayloadLocation<BlobHash>>,
    /// The most recently read blob, kept open for sequential reads.
    open: Option<(BlobHash, File)>,
    temp: u64,
}

const RECORD: usize = 1 + 32 + 8;

impl FileStorage {
    fn open(dir: &Path) -> io::Result<Self> {
        let blobs = dir.join("blobs");
        fs::create_dir_all(&blobs)?;
        let mut index = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(dir.join("payloads.idx"))?;
        let mut bytes = Vec::new();
        index.read_to_end(&mut bytes)?;
        // A torn final record was never acknowledged by `bind`; drop it.
        index.set_len((bytes.len() - bytes.len() % RECORD) as u64)?;
        let locations = bytes
            .chunks_exact(RECORD)
            .map(|r| {
                let blob = BlobHash::from(<[u8; 32]>::try_from(&r[1..33]).unwrap());
                let index = u64::from_le_bytes(r[33..].try_into().unwrap()) as usize;
                match r[0] {
                    0 => Ok(PayloadLocation::Canonical(blob)),
                    1 => Ok(PayloadLocation::Member { item: blob, index }),
                    _ => Err(io::Error::other("corrupt payload index")),
                }
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            blobs,
            index,
            locations,
            open: None,
            temp: 0,
        })
    }

    fn path(&self, blob: &BlobHash) -> PathBuf {
        let name: String = blob.as_slice().iter().map(|b| format!("{b:02x}")).collect();
        self.blobs.join(name)
    }
}

struct BlobWriter {
    file: File,
    path: PathBuf,
    hasher: blake3::Hasher,
}

impl Write for BlobWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.hasher.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        // Abandoned (or already renamed) temporary file.
        let _ = fs::remove_file(&self.path);
    }
}

impl Storage for FileStorage {
    type Key = BlobHash;
    type Writer = BlobWriter;

    fn create(&mut self) -> io::Result<BlobWriter> {
        self.temp += 1;
        let path = self
            .blobs
            .join(format!(".tmp-{}-{}", std::process::id(), self.temp));
        Ok(BlobWriter {
            file: File::create(&path)?,
            path,
            hasher: blake3::Hasher::new(),
        })
    }

    fn finish(&mut self, writer: BlobWriter) -> io::Result<BlobHash> {
        writer.file.sync_all()?;
        let blob = BlobHash::from(<[u8; 32]>::from(writer.hasher.finalize()));
        fs::rename(&writer.path, self.path(&blob))?;
        File::open(&self.blobs)?.sync_all()?; // make the rename durable
        Ok(blob)
    }

    fn read_at(&mut self, blob: &BlobHash, offset: u64, out: &mut [u8]) -> io::Result<usize> {
        if self.open.as_ref().is_none_or(|(open, _)| open != blob) {
            self.open = Some((*blob, File::open(self.path(blob))?));
        }
        let file = &mut self.open.as_mut().unwrap().1;
        file.seek(SeekFrom::Start(offset))?;
        file.read(out)
    }

    fn bind(&mut self, location: PayloadLocation<BlobHash>) -> io::Result<PayloadId> {
        let mut record = Vec::with_capacity(RECORD);
        let (tag, blob, index) = match location {
            PayloadLocation::Canonical(blob) => (0, blob, 0),
            PayloadLocation::Member { item, index } => (1, item, index as u64),
        };
        record.push(tag);
        record.extend_from_slice(blob.as_slice());
        record.extend_from_slice(&index.to_le_bytes());
        self.index.write_all(&record)?;
        self.index.sync_data()?;
        self.locations.push(location);
        Ok(self.locations.len() as PayloadId - 1)
    }

    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation<BlobHash>> {
        self.locations
            .get(id as usize)
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown payload ID"))
    }
}

// ---- codec ----

#[derive(Clone)]
struct ChunkCodec(FormatId);

/// Parses chunk framing into canonical member bytes.
struct ChunkDecoder {
    members: usize,
    ended: usize,
    length: [u8; 4],
    have: usize,
    remaining: usize,
}

impl Decoder for ChunkDecoder {
    fn step(&mut self, input: &[u8], eof: bool, output: &mut [u8]) -> io::Result<DecodeProgress> {
        let (mut consumed, mut produced) = (0, 0);
        let progress = |consumed, produced, status| {
            Ok(DecodeProgress {
                consumed,
                produced,
                status,
            })
        };
        loop {
            if self.ended == self.members {
                if consumed < input.len() {
                    return Err(io::Error::other("bytes after the last member"));
                }
                return progress(consumed, produced, DecodeStatus::Finished);
            }
            if self.remaining > 0 && produced == output.len() {
                return progress(consumed, produced, DecodeStatus::Working);
            }
            if consumed == input.len() {
                if eof {
                    return Err(io::Error::other("truncated body"));
                }
                return progress(consumed, produced, DecodeStatus::NeedInput);
            }
            if self.remaining > 0 {
                let n = self
                    .remaining
                    .min(input.len() - consumed)
                    .min(output.len() - produced);
                output[produced..produced + n].copy_from_slice(&input[consumed..consumed + n]);
                consumed += n;
                produced += n;
                self.remaining -= n;
                continue;
            }
            self.length[self.have] = input[consumed];
            consumed += 1;
            self.have += 1;
            if self.have == 4 {
                self.have = 0;
                self.remaining = u32::from_le_bytes(self.length) as usize;
                if self.remaining == 0 {
                    self.ended += 1;
                    return progress(consumed, produced, DecodeStatus::EndMember);
                }
            }
        }
    }
}

/// One payload's canonical bytes. A member of a stored item is found by
/// jumping over earlier members' chunks without reading their data.
struct Source<K> {
    blob: K,
    offset: u64,
    /// `(target member, current member, bytes left in this chunk)`.
    member: Option<(usize, usize, u64)>,
}

impl<K> Source<K> {
    fn open<S: Storage<Key = K>>(storage: &mut S, id: PayloadId) -> io::Result<Self> {
        Ok(match storage.location(id)? {
            PayloadLocation::Canonical(blob) => Self {
                blob,
                offset: 0,
                member: None,
            },
            PayloadLocation::Member { item, index } => {
                let mut prefix = [0u8; 16];
                let n = storage.read_at(&item, 0, &mut prefix)?;
                let header = wire::header_len(&prefix[..n], &Limits::default())
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("truncated item"))?;
                Self {
                    blob: item,
                    offset: header as u64,
                    member: Some((index, 0, 0)),
                }
            }
        })
    }

    fn read<S: Storage<Key = K>>(&mut self, storage: &mut S, out: &mut [u8]) -> io::Result<usize> {
        let Some((target, member, left)) = &mut self.member else {
            let n = storage.read_at(&self.blob, self.offset, out)?;
            self.offset += n as u64;
            return Ok(n);
        };
        loop {
            if *member > *target {
                return Ok(0);
            }
            if *left > 0 {
                let max = out.len().min(*left as usize);
                let n = storage.read_at(&self.blob, self.offset, &mut out[..max])?;
                if n == 0 {
                    return Err(io::Error::other("truncated item"));
                }
                self.offset += n as u64;
                *left -= n as u64;
                return Ok(n);
            }
            let mut length = [0u8; 4];
            let mut have = 0;
            while have < 4 {
                match storage.read_at(&self.blob, self.offset + have as u64, &mut length[have..])? {
                    0 => return Err(io::Error::other("truncated item")),
                    n => have += n,
                }
            }
            self.offset += 4;
            match u32::from_le_bytes(length) as u64 {
                0 => *member += 1,
                len if *member < *target => self.offset += len,
                len => *left = len,
            }
        }
    }
}

struct ChunkEncoder<K> {
    members: Vec<PayloadId>,
    next: usize,
    source: Option<Source<K>>,
}

impl<S: Storage> Encoder<S> for ChunkEncoder<S::Key> {
    fn step(&mut self, storage: &mut S, output: &mut [u8]) -> io::Result<EncodeProgress> {
        let mut produced = 0;
        while output.len() - produced > 4 {
            if self.source.is_none() {
                let Some(&id) = self.members.get(self.next) else {
                    return Ok(EncodeProgress {
                        produced,
                        finished: true,
                    });
                };
                self.source = Some(Source::open(storage, id)?);
            }
            let chunk = &mut output[produced + 4..];
            let n = self.source.as_mut().unwrap().read(storage, chunk)?;
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
        &self.0
    }
    fn decoder(&self, meta: &Arc<BundleMeta>) -> io::Result<ChunkDecoder> {
        Ok(ChunkDecoder {
            members: meta.len(),
            ended: 0,
            length: [0; 4],
            have: 0,
            remaining: 0,
        })
    }
    fn encoder(&self, item: &ExportItem) -> io::Result<ChunkEncoder<S::Key>> {
        let members = item.members.iter().map(|&(_, id)| id).collect();
        Ok(ChunkEncoder {
            members,
            next: 0,
            source: None,
        })
    }
}

// ---- demo ----

/// Deterministic pseudo-random bytes, generated as they are read.
struct Generated {
    left: u64,
    state: u64,
}

impl Read for Generated {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out.len().min(self.left as usize);
        for chunk in out[..n].chunks_mut(8) {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            chunk.copy_from_slice(&self.state.to_le_bytes()[..chunk.len()]);
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Peak resident memory, where the platform reports it.
fn peak_rss() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l[6..].trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

type FileGraph = Graph<ChunkCodec, FileStorage>;
type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Send everything `from` has to `to` through a file, which stands in for a
/// network stream. Returns the acknowledged cursor.
fn send(from: &mut FileGraph, to: &mut FileGraph, transfer: &Path) -> Fallible<ExportCursor> {
    let mut plan = from.export(&ExportCursor::new());
    for index in 0..plan.len() {
        let started = Instant::now();
        let mut out = BufWriter::new(File::create(transfer)?);
        let written = from.write_item(&plan, index, &mut out)?;
        out.into_inner()?.sync_all()?;
        let receipt = to.receive(BufReader::new(File::open(transfer)?))?;
        plan.acknowledge(index).unwrap();
        let commits = receipt.members.len();
        println!(
            "  sent {written} bytes, admitted {commits} commit(s) in {:.1?}",
            started.elapsed()
        );
    }
    fs::remove_file(transfer)?;
    Ok(plan.cursor())
}

/// Stream a commit's payload out of `graph`, which checks it against the commit.
fn verify(graph: &mut FileGraph, expected: &Prepared) -> Fallible<()> {
    assert_eq!(
        graph.read_payload(&expected.hash, io::sink())?,
        expected.len
    );
    Ok(())
}

/// Stream payload `id` straight from storage, without a history to check it
/// against, and compare its hash.
fn verify_stored(storage: &mut FileStorage, id: PayloadId, expected: &Prepared) -> Fallible<()> {
    let mut source = Source::open(storage, id)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match source.read(storage, &mut buf)? {
            0 => break,
            n => hasher.update(&buf[..n]),
        };
    }
    assert_eq!(
        BlobHash::from(<[u8; 32]>::from(hasher.finalize())),
        expected.payload_hash
    );
    Ok(())
}

fn main() -> Fallible<()> {
    let mib: u64 = std::env::args().nth(1).map_or(Ok(64), |a| a.parse())?;
    let root = std::env::temp_dir().join(format!("stockfoil-file-storage-{}", std::process::id()));
    let format = FormatId::new("stockfoil-example/chunks-v1")?;
    let open = |name: &str| -> io::Result<FileGraph> {
        Ok(Graph::new(
            ChunkCodec(format.clone()),
            FileStorage::open(&root.join(name))?,
        ))
    };
    let (mut sender, mut receiver, mut relay) =
        (open("sender")?, open("receiver")?, open("relay")?);
    let key = SigningKey::from_bytes(&[7; 32]);
    let author = Author::try_from(key.verifying_key().as_bytes().as_slice())?;

    // Hash and store the payload in one streaming pass, then sign the commit.
    let started = Instant::now();
    let payload = Generated {
        left: mib << 20,
        state: 0x9e37_79b9_7f4a_7c15,
    };
    let prepared = sender.commit(author, vec![], payload)?;
    sender.sign(
        author,
        prepared.hash,
        key.sign(&prepared.signing_bytes()).to_bytes().into(),
    )?;
    println!(
        "committed and signed {mib} MiB in {:.1?}",
        started.elapsed()
    );

    println!("sender -> receiver");
    let transfer = root.join("transfer.item");
    let cursor = send(&mut sender, &mut receiver, &transfer)?;
    assert!(sender.export(&cursor).is_empty());

    // The receiver's payload is a member of its stored item. Reading it, or
    // relaying it, decodes that item again.
    let id = receiver
        .history()
        .lookup(&prepared.hash)
        .ok_or("not admitted")?
        .payload;
    if let PayloadLocation::Member { item, index } = receiver.storage().location(id)? {
        let path = receiver.storage().path(&item);
        println!(
            "receiver payload {id} is member {index} of {}",
            path.display()
        );
    }
    verify(&mut receiver, &prepared)?;
    println!("receiver -> relay");
    send(&mut receiver, &mut relay, &transfer)?;
    verify(&mut relay, &prepared)?;

    // Reopen the receiver's storage: blobs and the payload index persisted.
    drop(receiver);
    verify_stored(open("receiver")?.storage_mut(), id, &prepared)?;
    println!(
        "payload verified after reopening storage; peak RSS {}",
        peak_rss()
    );

    drop((sender, relay));
    fs::remove_dir_all(&root)?;
    Ok(())
}
