//! A fake foreign host: an in-memory blob store keyed by `BlobHash::of_contents`,
//! per-operation `ChunkDecoder`s, and a payload table filled from the canonical
//! bytes the host itself replied with.
use crate::codec::{ChunkDecoder, chunk_body, format};
use std::collections::HashMap;
use stockfoil::*;

#[derive(Default)]
pub struct Host {
    pub blobs: HashMap<BlobHash, Vec<u8>>,
    /// Payload ID -> canonical bytes.
    pub payloads: Vec<Vec<u8>>,
}

/// How the honest host executes requests.
#[derive(Clone, Copy, Debug)]
pub struct DriveConfig {
    /// Largest read the host returns, even if more was requested (>= 1).
    pub read_limit: usize,
    /// Largest decoder output buffer the host uses (>= 1 for progress).
    pub decode_limit: usize,
    /// Upper bound on `next`/`advance` calls before declaring the op stuck.
    pub max_steps: usize,
}

impl Default for DriveConfig {
    fn default() -> Self {
        Self {
            read_limit: usize::MAX,
            decode_limit: 1 << 16,
            max_steps: 1 << 22,
        }
    }
}

/// Owned reply data.
#[derive(Debug, Clone)]
pub enum Owned {
    Read(Vec<u8>, bool),
    Decoded(usize, Vec<u8>, DecodeStatus),
    Bound(Vec<PayloadId>),
    Failed(String),
}

impl Owned {
    pub fn reply(&self) -> Reply<'_> {
        match self {
            Owned::Read(b, eof) => Reply::Read { bytes: b, eof: *eof },
            Owned::Decoded(c, o, s) => Reply::Decoded {
                consumed: *c,
                output: o,
                status: *s,
            },
            Owned::Bound(ids) => Reply::Bound(ids),
            Owned::Failed(m) => Reply::Failed(m),
        }
    }
}

/// Per-operation host state: the decoder and the canonical member bytes the
/// host has replied with so far.
#[derive(Default, Clone)]
pub struct OpState {
    pub decoder: Option<ChunkDecoder>,
    pub members: Vec<Vec<u8>>,
    pub current: Vec<u8>,
}

impl OpState {
    /// Record a decode reply that was accepted.
    pub fn record(&mut self, output: &[u8], status: DecodeStatus) {
        self.current.extend_from_slice(output);
        if status == DecodeStatus::EndMember {
            self.members.push(std::mem::take(&mut self.current));
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Done(Receipt),
    Waiting(CommitHash),
    Failed(Error),
    Stuck,
}

/// An owned description of a request, so the admission can be mutably used
/// while the fuzzer decides what to answer.
#[derive(Debug, Clone)]
pub enum Req {
    Read { item: BlobHash, offset: u64, max: usize },
    Decode { members: usize, input: Vec<u8>, eof: bool, capacity: usize },
    Bind { item: BlobHash, members: usize },
}

impl Req {
    /// A request from the admission of stored item `item`.
    pub fn of(item: BlobHash, r: &Request<'_>) -> Self {
        match *r {
            Request::Read { offset, max } => Req::Read { item, offset, max },
            Request::Decode { meta, input, eof, capacity } => Req::Decode {
                members: meta.len(),
                input: input.to_vec(),
                eof,
                capacity,
            },
            Request::Bind { meta } => Req::Bind { item, members: meta.len() },
        }
    }
}

impl Host {
    pub fn store(&mut self, bytes: Vec<u8>) -> BlobHash {
        let hash = BlobHash::of_contents(&bytes);
        self.blobs.insert(hash, bytes);
        hash
    }

    /// Bind fresh payload IDs to the canonical member bytes in `st`.
    pub fn bind(&mut self, members: usize, st: &OpState) -> Vec<PayloadId> {
        (0..members)
            .map(|i| {
                self.payloads.push(st.members.get(i).cloned().unwrap_or_default());
                (self.payloads.len() - 1) as PayloadId
            })
            .collect()
    }

    /// Honest execution of a request. Does not record decode output in `st`
    /// (call [`OpState::record`] once the reply is accepted).
    pub fn execute(&mut self, req: &Req, st: &mut OpState, cfg: &DriveConfig) -> Owned {
        match req {
            Req::Read { item, offset, max } => {
                let Some(bytes) = self.blobs.get(item) else {
                    return Owned::Failed("unknown blob".into());
                };
                let max = cfg.read_limit.max(1).min(*max);
                let start = usize::try_from(*offset).unwrap_or(usize::MAX).min(bytes.len());
                let end = start.saturating_add(max).min(bytes.len());
                Owned::Read(bytes[start..end].to_vec(), end == bytes.len())
            }
            Req::Decode { members, input, eof, capacity } => {
                let d = st.decoder.get_or_insert_with(|| ChunkDecoder::new(*members));
                let mut out = vec![0; cfg.decode_limit.min(*capacity)];
                match stockfoil::Decoder::step(d, input, *eof, &mut out) {
                    Ok(p) => {
                        out.truncate(p.produced);
                        Owned::Decoded(p.consumed, out, p.status)
                    }
                    Err(e) => Owned::Failed(e.to_string()),
                }
            }
            Req::Bind { members, .. } => Owned::Bound(self.bind(*members, st)),
        }
    }

    /// Drive the admission `op` of stored item `item` honestly until done,
    /// failed, waiting, or `max_steps`.
    pub fn run(
        &mut self,
        history: &mut History,
        op: &mut Admission,
        item: BlobHash,
        cfg: &DriveConfig,
    ) -> Outcome {
        let mut st = OpState::default();
        for _ in 0..cfg.max_steps {
            let req = match op.next() {
                Next::Continue => continue,
                Next::Done(r) => return Outcome::Done(r.clone()),
                Next::Failed(e) => return Outcome::Failed(e.clone()),
                Next::NeedHistory | Next::Waiting(_) => match history.advance(op) {
                    Next::Done(r) => return Outcome::Done(r.clone()),
                    Next::Failed(e) => return Outcome::Failed(e.clone()),
                    Next::Waiting(h) => return Outcome::Waiting(h),
                    Next::Request(t, r) => Some((t, Req::of(item, &r))),
                    _ => None,
                },
                Next::Request(t, r) => Some((t, Req::of(item, &r))),
                Next::AwaitReply => {
                    let (t, r) = op.request().expect("AwaitReply without a request");
                    Some((t, Req::of(item, &r)))
                }
            };
            if let Some((ticket, req)) = req {
                let owned = self.execute(&req, &mut st, cfg);
                match op.reply(ticket, owned.reply()) {
                    Ok(()) => {
                        if let Owned::Decoded(_, out, status) = &owned {
                            st.record(out, *status);
                        }
                    }
                    Err(ReplyError::Invalid(e)) => return Outcome::Failed(e),
                    Err(e) => panic!("honest reply to the outstanding request rejected: {e:?}"),
                }
            }
        }
        Outcome::Stuck
    }

    /// Serialize every item `history` would export to a fresh destination,
    /// using the payload table for member bodies.
    pub fn export_all(&self, history: &History) -> Vec<(ItemId, Vec<u8>)> {
        let plan = history.export(&ExportCursor::new());
        (0..plan.len())
            .map(|i| {
                let item = history
                    .export_item(&plan, i)
                    .expect("planned item cannot be exported");
                let mut bytes = wire::encode_header(&item.meta, &format());
                let payloads: Vec<&[u8]> = item
                    .members
                    .iter()
                    .map(|(_, id)| self.payloads[*id as usize].as_slice())
                    .collect();
                bytes.extend(chunk_body(payloads));
                (item.meta.id.clone(), bytes)
            })
            .collect()
    }
}

/// Receive `items` into `history`, retrying waiting items until nothing more
/// can be admitted. Returns the items still waiting. Panics if an item fails.
pub fn receive_all(
    history: &mut History,
    host: &mut Host,
    items: &[Vec<u8>],
    cfg: &DriveConfig,
) -> Vec<(BlobHash, CommitHash)> {
    let mut pending: Vec<BlobHash> = items.iter().map(|b| host.store(b.clone())).collect();
    loop {
        let mut waiting = Vec::new();
        let before = pending.len();
        for blob in pending {
            let mut op = history.receive(ReceiveOptions::new(format()));
            match host.run(history, &mut op, blob, cfg) {
                Outcome::Done(_) => {}
                Outcome::Waiting(h) => waiting.push((blob, h)),
                Outcome::Failed(e) => panic!("valid item rejected: {e:?}"),
                Outcome::Stuck => panic!("honest admission did not terminate"),
            }
        }
        if waiting.is_empty() || waiting.len() == before {
            return waiting;
        }
        pending = waiting.into_iter().map(|(b, _)| b).collect();
    }
}
