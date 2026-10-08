//! Native Rust adapter: a [`History`] plus an application codec and storage,
//! driven synchronously. It executes the same host requests a foreign binding
//! would; it has no receive, publication, or export logic of its own.
use crate::{
    Admission, Author, BlobHash, BundleMeta, CommitHash, DecodeStatus, Error, ExportItem,
    ExportPlan, FormatId, History, ItemId, Limits, Next, PayloadId, Prepared, Receipt,
    ReceiveOptions, Reply, Request, Signature,
};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Where a logical payload's canonical bytes can be reproduced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadLocation {
    /// A blob holding exactly the canonical bytes.
    Canonical(BlobHash),
    /// Member `index` of a stored wire item, decoded with the application codec.
    Member { item: BlobHash, index: usize },
}

/// Content-addressed blobs plus logical payload IDs.
///
/// `finish` must durably accept the written bytes (to the application's
/// standard) and return their BLAKE3 hash. `read_at` returns bytes of an
/// accepted blob, or `Ok(0)` at its end. IDs returned by `bind` must stay
/// resolvable while history refers to them and must never be rebound.
/// Failed operations may leave unused blobs or IDs behind; reclaiming them is
/// the application's business.
pub trait Storage {
    type Writer: Write;
    fn create(&mut self) -> io::Result<Self::Writer>;
    fn finish(&mut self, writer: Self::Writer) -> io::Result<BlobHash>;
    fn read_at(&mut self, blob: &BlobHash, offset: u64, out: &mut [u8]) -> io::Result<usize>;
    fn bind(&mut self, location: PayloadLocation) -> io::Result<PayloadId>;
    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation>;
}

/// One decoder step's progress. See [`DecodeStatus`] for the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeProgress {
    pub consumed: usize,
    pub produced: usize,
    pub status: DecodeStatus,
}

/// Incremental reconstruction of canonical member bytes from an encoded body.
/// Slices are borrowed for one call only; a decoder keeps its own bounded state.
pub trait Decoder {
    fn step(&mut self, input: &[u8], eof: bool, output: &mut [u8]) -> io::Result<DecodeProgress>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeProgress {
    pub produced: usize,
    pub finished: bool,
}

/// Incremental production of an encoded body. It may read anything it needs
/// from storage, including an earlier stored body it can copy unchanged.
pub trait Encoder<S: Storage> {
    fn step(&mut self, storage: &mut S, output: &mut [u8]) -> io::Result<EncodeProgress>;
}

/// An application bundle codec. Incoming format IDs are compared with
/// [`Codec::format`]; they never select code.
pub trait Codec<S: Storage> {
    type Decoder: Decoder;
    type Encoder: Encoder<S>;
    fn format(&self) -> &FormatId;
    /// `meta` is structurally checked but not yet authenticated.
    fn decoder(&self, meta: &Arc<BundleMeta>) -> io::Result<Self::Decoder>;
    fn encoder(&self, item: &ExportItem) -> io::Result<Self::Encoder>;
}

/// The outcome of driving an admission as far as it can go.
#[derive(Debug)]
pub enum Driven {
    Done(Receipt),
    /// Waiting for this dependency; drive again after more history arrives.
    Waiting(CommitHash),
}

/// A history with a native codec and storage. Every method runs to completion.
pub struct Graph<C, S> {
    history: History,
    codec: C,
    storage: S,
    options: ReceiveOptions,
}

const CHUNK: usize = 64 * 1024;

fn storage_error(e: io::Error) -> Error {
    Error::Storage(e.to_string())
}

impl<C: Codec<S>, S: Storage> Graph<C, S> {
    pub fn new(codec: C, storage: S) -> Self {
        let options = ReceiveOptions::new(codec.format().clone());
        Self {
            history: History::new(),
            codec,
            storage,
            options,
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }
    pub fn codec(&self) -> &C {
        &self.codec
    }
    pub fn storage(&self) -> &S {
        &self.storage
    }
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }
    /// The configured application format.
    pub fn format(&self) -> &FormatId {
        self.codec.format()
    }
    pub fn set_limits(&mut self, limits: Limits) {
        self.options.limits = limits;
    }
    /// Options used for admissions started by this graph.
    pub fn receive_options(&self) -> &ReceiveOptions {
        &self.options
    }

    /// Start admitting a stored item without driving it. Keep the returned
    /// operation across [`Driven::Waiting`] to avoid re-reading and
    /// re-verifying the item once its dependency arrives.
    pub fn begin_admit(&self, item: BlobHash) -> Admission {
        self.history.receive(item, self.options.clone())
    }

    /// Store an incoming item, then admit it. Fails with
    /// [`Error::MissingDependency`] if history it depends on is not admitted;
    /// the item stays stored and can be admitted later with [`Self::admit`]
    /// (which verifies it again), or use [`Self::begin_admit`] and
    /// [`Self::drive`] to keep the verified operation while it waits.
    pub fn receive(&mut self, mut item: impl Read) -> Result<Receipt, Error> {
        let mut writer = self.storage.create().map_err(storage_error)?;
        io::copy(&mut item, &mut writer).map_err(storage_error)?;
        let hash = self.storage.finish(writer).map_err(storage_error)?;
        self.admit(hash)
    }

    /// Admit an item already in storage.
    pub fn admit(&mut self, item: BlobHash) -> Result<Receipt, Error> {
        let mut op = self.begin_admit(item);
        match self.drive(&mut op)? {
            Driven::Done(receipt) => Ok(receipt),
            Driven::Waiting(hash) => Err(Error::MissingDependency(hash)),
        }
    }

    /// Drive an admission until it is done, fails, or waits for a dependency.
    pub fn drive(&mut self, op: &mut Admission) -> Result<Driven, Error> {
        let mut decoder: Option<C::Decoder> = None;
        let mut buffer = vec![0u8; CHUNK.max(self.options.read_size)];
        loop {
            match op.next() {
                Next::Continue => continue,
                Next::Done(receipt) => return Ok(Driven::Done(receipt.clone())),
                Next::Failed(e) => return Err(e.clone()),
                Next::Waiting(_) | Next::NeedHistory => match self.history.advance(op) {
                    Next::Waiting(hash) => return Ok(Driven::Waiting(hash)),
                    Next::Done(receipt) => return Ok(Driven::Done(receipt.clone())),
                    Next::Failed(e) => return Err(e.clone()),
                    // Any request it issued is executed below.
                    _ => {}
                },
                Next::Request(..) | Next::AwaitReply => {}
            }
            // This driver is synchronous, so an outstanding request is always ours.
            let Some((ticket, request)) = op.request() else {
                continue;
            };
            let outcome = self.execute(request, &mut decoder, &mut buffer);
            let reply = match &outcome {
                Ok(Executed::Read { len, eof }) => Reply::Read {
                    bytes: &buffer[..*len],
                    eof: *eof,
                },
                Ok(Executed::Decoded(p)) => Reply::Decoded {
                    consumed: p.consumed,
                    output: &buffer[..p.produced],
                    status: p.status,
                },
                Ok(Executed::Bound(ids)) => Reply::Bound(ids),
                Err(message) => Reply::Failed(message),
            };
            if let Err(crate::ReplyError::Invalid(e)) = op.reply(ticket, reply) {
                return Err(e);
            }
        }
    }

    fn execute(
        &mut self,
        request: Request<'_>,
        decoder: &mut Option<C::Decoder>,
        buffer: &mut [u8],
    ) -> Result<Executed, String> {
        match request {
            Request::Read { item, offset, max } => {
                let len = max.min(buffer.len());
                let n = self
                    .storage
                    .read_at(&item, offset, &mut buffer[..len])
                    .map_err(|e| e.to_string())?;
                Ok(Executed::Read {
                    len: n,
                    eof: n == 0,
                })
            }
            Request::Decode {
                meta,
                input,
                eof,
                capacity,
            } => {
                if decoder.is_none() {
                    *decoder = Some(self.codec.decoder(meta).map_err(|e| e.to_string())?);
                }
                let capacity = capacity.min(buffer.len());
                let progress = decoder
                    .as_mut()
                    .unwrap()
                    .step(input, eof, &mut buffer[..capacity])
                    .map_err(|e| e.to_string())?;
                Ok(Executed::Decoded(progress))
            }
            Request::Bind { item, meta } => (0..meta.len())
                .map(|index| self.storage.bind(PayloadLocation::Member { item, index }))
                .collect::<io::Result<Vec<_>>>()
                .map(Executed::Bound)
                .map_err(|e| e.to_string()),
        }
    }

    /// Create, store, and deliver a local draft. Sign `signing_bytes` of the
    /// result and pass the signature to [`Self::sign`] to admit it.
    pub fn commit(
        &mut self,
        author: Author,
        parents: Vec<CommitHash>,
        mut payload: impl Read,
    ) -> Result<Prepared, Error> {
        let mut builder = self.history.begin_commit(author, parents)?;
        let mut writer = self.storage.create().map_err(storage_error)?;
        let mut chunk = vec![0u8; CHUNK];
        loop {
            let n = match payload.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(storage_error(e)),
            };
            builder.update(&chunk[..n]);
            writer.write_all(&chunk[..n]).map_err(storage_error)?;
        }
        let prepared = builder.finish();
        let blob = self.storage.finish(writer).map_err(storage_error)?;
        if blob != prepared.payload_hash {
            return Err(Error::Storage("stored payload hash differs".into()));
        }
        let id = self
            .storage
            .bind(PayloadLocation::Canonical(blob))
            .map_err(storage_error)?;
        self.history.publish_commit(prepared.clone(), id)?;
        Ok(prepared)
    }

    pub fn sign(
        &mut self,
        author: Author,
        hash: CommitHash,
        signature: Signature,
    ) -> Result<Vec<ItemId>, Error> {
        self.history.sign(author, hash, signature)
    }

    pub fn export(&self, cursor: &crate::ExportCursor) -> ExportPlan {
        self.history.export(cursor)
    }

    /// Write planned item `index` (header and encoded body). Returns bytes written.
    pub fn write_item(
        &mut self,
        plan: &ExportPlan,
        index: usize,
        mut out: impl Write,
    ) -> Result<u64, Error> {
        let item = self.history.export_item(plan, index)?;
        let header = crate::wire::encode_header(&item.meta, self.codec.format());
        out.write_all(&header).map_err(storage_error)?;
        let mut encoder = self
            .codec
            .encoder(&item)
            .map_err(|e| Error::Codec(e.to_string()))?;
        let mut chunk = vec![0u8; CHUNK];
        let mut written = header.len() as u64;
        loop {
            let progress = encoder
                .step(&mut self.storage, &mut chunk)
                .map_err(|e| Error::Codec(e.to_string()))?;
            if progress.produced > chunk.len() {
                return Err(Error::Protocol("encoder output exceeds capacity"));
            }
            out.write_all(&chunk[..progress.produced])
                .map_err(storage_error)?;
            written += progress.produced as u64;
            if progress.finished {
                return Ok(written);
            }
            if progress.produced == 0 {
                return Err(Error::Protocol("encoder made no progress"));
            }
        }
    }

    /// Write planned item `index` into this graph's storage.
    pub fn store_item(&mut self, plan: &ExportPlan, index: usize) -> Result<BlobHash, Error> {
        let mut writer = self.storage.create().map_err(storage_error)?;
        self.write_item(plan, index, &mut writer)?;
        self.storage.finish(writer).map_err(storage_error)
    }

    /// Write one payload's canonical bytes to `out`.
    pub fn read_payload(&mut self, id: PayloadId, mut out: impl Write) -> Result<u64, Error> {
        match self.storage.location(id).map_err(storage_error)? {
            PayloadLocation::Canonical(blob) => {
                let mut chunk = vec![0u8; CHUNK];
                let mut offset = 0u64;
                loop {
                    let n = self
                        .storage
                        .read_at(&blob, offset, &mut chunk)
                        .map_err(storage_error)?;
                    if n == 0 {
                        return Ok(offset);
                    }
                    out.write_all(&chunk[..n]).map_err(storage_error)?;
                    offset += n as u64;
                }
            }
            PayloadLocation::Member { item, index } => {
                let mut written = 0u64;
                self.decode_item(item, |member, bytes| {
                    if member == index {
                        out.write_all(bytes)?;
                        written += bytes.len() as u64;
                    }
                    Ok(member <= index)
                })?;
                Ok(written)
            }
        }
    }

    /// One payload's canonical bytes.
    pub fn payload(&mut self, id: PayloadId) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        self.read_payload(id, &mut bytes)?;
        Ok(bytes)
    }

    /// Decode a stored item once, passing each canonical chunk with its member
    /// index to `visit`. Members end in order; `visit` returns `false` to stop.
    /// Use this to read many payloads that share one compressed item.
    pub fn decode_item(
        &mut self,
        item: BlobHash,
        mut visit: impl FnMut(usize, &[u8]) -> io::Result<bool>,
    ) -> Result<(), Error> {
        let mut prefix = Vec::new();
        let mut chunk = vec![0u8; CHUNK];
        let mut offset = 0u64;
        let header_len = loop {
            if let Some(len) = crate::wire::header_len(&prefix, &self.options.limits)?
                && prefix.len() >= len
            {
                break len;
            }
            let n = self
                .storage
                .read_at(&item, offset, &mut chunk)
                .map_err(storage_error)?;
            if n == 0 {
                return Err(Error::Malformed("truncated header"));
            }
            prefix.extend_from_slice(&chunk[..n]);
            offset += n as u64;
        };
        let mut input = prefix.split_off(header_len);
        let meta = Arc::new(crate::wire::decode_header(
            &prefix,
            self.codec.format(),
            &self.options.limits,
        )?);
        let mut decoder = self
            .codec
            .decoder(&meta)
            .map_err(|e| Error::Codec(e.to_string()))?;
        let mut output = vec![0u8; CHUNK];
        let mut member = 0usize;
        let mut eof = false;
        let mut start = 0usize;
        loop {
            if start == input.len() && !eof {
                input.clear();
                start = 0;
                let n = self
                    .storage
                    .read_at(&item, offset, &mut chunk)
                    .map_err(storage_error)?;
                offset += n as u64;
                eof = n == 0;
                input.extend_from_slice(&chunk[..n]);
            }
            let p = decoder
                .step(&input[start..], eof, &mut output)
                .map_err(|e| Error::Codec(e.to_string()))?;
            if p.consumed > input.len() - start || p.produced > output.len() {
                return Err(Error::Protocol("decoder reported impossible counts"));
            }
            if p.status == DecodeStatus::Working && p.consumed == 0 && p.produced == 0 {
                return Err(Error::Protocol("decoder made no progress"));
            }
            start += p.consumed;
            if p.produced > 0 || p.status == DecodeStatus::EndMember {
                let keep = visit(member, &output[..p.produced]).map_err(storage_error)?;
                if !keep {
                    return Ok(());
                }
            }
            match p.status {
                DecodeStatus::EndMember => member += 1,
                DecodeStatus::Finished => return Ok(()),
                DecodeStatus::NeedInput if eof => {
                    return Err(Error::Codec("decoder needs input after end of body".into()));
                }
                _ => {}
            }
        }
    }
}

enum Executed {
    Read { len: usize, eof: bool },
    Decoded(DecodeProgress),
    Bound(Vec<PayloadId>),
}

/// In-memory [`Storage`], for tests and small applications.
#[derive(Debug, Default, Clone)]
pub struct MemoryStorage {
    blobs: HashMap<BlobHash, Arc<Vec<u8>>>,
    payloads: Vec<PayloadLocation>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, blob: &BlobHash) -> Option<&[u8]> {
        self.blobs.get(blob).map(|b| b.as_slice())
    }
    pub fn insert(&mut self, bytes: Vec<u8>) -> BlobHash {
        let hash = BlobHash::of_contents(&bytes);
        self.blobs.insert(hash, Arc::new(bytes));
        hash
    }
    pub fn blob_count(&self) -> usize {
        self.blobs.len()
    }
}

impl Storage for MemoryStorage {
    type Writer = Vec<u8>;
    fn create(&mut self) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn finish(&mut self, writer: Vec<u8>) -> io::Result<BlobHash> {
        Ok(self.insert(writer))
    }
    fn read_at(&mut self, blob: &BlobHash, offset: u64, out: &mut [u8]) -> io::Result<usize> {
        let bytes = self
            .blobs
            .get(blob)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown blob"))?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let n = out.len().min(bytes.len() - start);
        out[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }
    fn bind(&mut self, location: PayloadLocation) -> io::Result<PayloadId> {
        self.payloads.push(location);
        Ok(self.payloads.len() as PayloadId - 1)
    }
    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation> {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.payloads.get(i))
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown payload ID"))
    }
}
