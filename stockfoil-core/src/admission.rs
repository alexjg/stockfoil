//! Receiving a stored item: the one operation that needs host requests.
//!
//! An [`Admission`] owns its progress and buffers and never borrows the history
//! between calls. It asks the host to read the stored item, run the application
//! decoder, and bind payload IDs, and it hashes and verifies everything itself:
//! there is no host reply that means "verified".
//!
//! An operation does not know where its item is stored. The host does: storage
//! keys are the host's business and need not be content hashes (an encrypted
//! store, for example, knows only ciphertext digests). Nothing depends on the
//! stored representation beyond the canonical payloads it decodes to.
//!
//! ```text
//! Read*        parse the header
//! Decode*      hash canonical members; check boundaries, EOF, codec completion
//!              check signatures and coverage            (all without the history)
//! NeedHistory  dependencies and fragment placement      (History::advance)
//! Bind         payload IDs for undelivered members
//! NeedHistory  atomic publication                       (History::advance)
//! ```
use crate::history::Readiness;
use crate::verify::{Verified, Verifier};
use crate::{BundleMeta, CommitHash, Error, FormatId, History, Limits, PayloadId, Receipt};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Options for one received item.
#[derive(Debug, Clone)]
pub struct ReceiveOptions {
    /// The trusted application format. Other formats are rejected before decoding.
    pub format: FormatId,
    pub limits: Limits,
    /// Largest read requested from storage.
    pub read_size: usize,
    /// Largest decoder output accepted per step.
    pub decode_capacity: usize,
    /// Work per `next` call, in bytes hashed (a signature check counts as 16 KiB,
    /// and the coverage check one per member and parent edge visited).
    pub quantum: usize,
}

impl ReceiveOptions {
    pub fn new(format: FormatId) -> Self {
        Self {
            format,
            limits: Limits::default(),
            read_size: 64 * 1024,
            decode_capacity: 64 * 1024,
            quantum: 256 * 1024,
        }
    }
}

/// Matches a reply to the request it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket(u64);

impl Ticket {
    pub fn get(self) -> u64 {
        self.0
    }
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Work for the host. At most one is outstanding per operation; its inputs do
/// not change until it is answered.
#[derive(Debug, Clone, Copy)]
pub enum Request<'a> {
    /// Read up to `max` bytes of the stored item starting at `offset`.
    Read { offset: u64, max: usize },
    /// Run one decoder step. Create this operation's decoder from `meta` on the
    /// first `Decode`; drop it when the operation ends. `eof` means `input`
    /// ends the encoded body.
    Decode {
        meta: &'a Arc<BundleMeta>,
        input: &'a [u8],
        eof: bool,
        capacity: usize,
    },
    /// Allocate a payload ID for every member and bind it to `(item, member)`.
    Bind { meta: &'a Arc<BundleMeta> },
}

/// Decoder progress, as defined by the application codec contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeStatus {
    /// More can be produced without more input. Must have made progress.
    Working,
    /// Every offered input byte was consumed; more input (or EOF) is needed.
    NeedInput,
    /// The output of this step completes the current member. Every member,
    /// including an empty one, ends exactly once, in metadata order.
    EndMember,
    /// Every member has ended. No body bytes may remain.
    Finished,
}

/// The host's answer to a [`Request`].
#[derive(Debug, Clone, Copy)]
pub enum Reply<'a> {
    /// `bytes` read at the requested offset. `eof`: nothing follows them.
    /// An empty read must set `eof`.
    Read { bytes: &'a [u8], eof: bool },
    Decoded {
        consumed: usize,
        output: &'a [u8],
        status: DecodeStatus,
    },
    /// One payload ID per member, in member order.
    Bound(&'a [PayloadId]),
    /// The request failed. The operation fails.
    Failed(&'a str),
}

/// A reply that was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    /// Not the outstanding ticket (stale, duplicate, or for another operation).
    /// The operation is unaffected.
    Stale,
    /// The reply kind does not match the request. The operation is unaffected.
    WrongKind,
    /// The reply broke the protocol; the operation has failed with this error.
    Invalid(Error),
}

/// What the host should do next. Exactly one applies.
#[derive(Debug)]
pub enum Next<'a> {
    /// Call [`Admission::next`] again; the work quantum was used up.
    Continue,
    /// Execute this request and answer it with [`Admission::reply`].
    Request(Ticket, Request<'a>),
    /// A request is outstanding; nothing else can proceed until its reply.
    AwaitReply,
    /// Call [`History::advance`].
    NeedHistory,
    /// A dependency is not admitted yet. Call [`History::advance`] again after
    /// [`History::epoch`] changes.
    Waiting(CommitHash),
    /// Admitted.
    Done(&'a Receipt),
    Failed(&'a Error),
}

/// Exact progress counters, available at any time, including after failure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub bytes_read: u64,
    pub encoded_bytes_decoded: u64,
    pub canonical_bytes: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Decode,
    Bind,
}

#[derive(Debug, Clone, Copy)]
struct Outstanding {
    ticket: u64,
    kind: Kind,
    max: usize,
    eof: bool,
}

enum Stage {
    Header { buffer: Vec<u8> },
    Body,
    Verify,
    Place,
    Bind,
    Publish,
    Done(Receipt),
    Failed(Error),
}

/// Receive one stored item. Created by [`History::receive`].
pub struct Admission {
    history: u64,
    options: ReceiveOptions,
    stage: Stage,
    meta: Option<Arc<BundleMeta>>,
    outstanding: Option<Outstanding>,
    offset: u64,
    eof: bool,
    input: Vec<u8>,
    input_start: usize,
    canonical: Vec<u8>,
    canonical_start: usize,
    pending: Option<DecodeStatus>,
    last: Option<DecodeStatus>,
    offered_eof: bool,
    finished: bool,
    verifier: Option<Verifier>,
    verified: Option<Arc<Verified>>,
    payloads: Vec<PayloadId>,
    waiting: Option<(CommitHash, u64)>,
    counts: Counts,
}

impl std::fmt::Debug for Admission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Admission")
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

impl Admission {
    pub(crate) fn new(history: u64, mut options: ReceiveOptions) -> Self {
        // Zero sizes could never make progress; huge ones are capped so that
        // buffer arithmetic cannot overflow.
        const MAX_BUFFER: usize = 1 << 30;
        options.read_size = options.read_size.clamp(1, MAX_BUFFER);
        options.decode_capacity = options.decode_capacity.clamp(1, MAX_BUFFER);
        options.quantum = options.quantum.max(1);
        Self {
            history,
            options,
            stage: Stage::Header { buffer: Vec::new() },
            meta: None,
            outstanding: None,
            offset: 0,
            eof: false,
            input: Vec::new(),
            input_start: 0,
            canonical: Vec::new(),
            canonical_start: 0,
            pending: None,
            last: None,
            offered_eof: false,
            finished: false,
            verifier: None,
            verified: None,
            payloads: Vec::new(),
            waiting: None,
            counts: Counts::default(),
        }
    }

    pub(crate) fn advance(&mut self, history: &mut History) -> Next<'_> {
        if self.history != history.id() {
            return Next::Failed(&WRONG_HISTORY);
        }
        match self.stage {
            Stage::Place => {
                if let Some((hash, epoch)) = self.waiting
                    && epoch == history.epoch()
                {
                    return Next::Waiting(hash);
                }
                let verified = Arc::clone(self.verified.as_ref().unwrap());
                match crate::history::readiness(history, &verified) {
                    Err(e) => self.fail(e),
                    Ok(Readiness::Waiting(hash)) => {
                        self.waiting = Some((hash, history.epoch()));
                        return Next::Waiting(hash);
                    }
                    Ok(Readiness::Ready { undelivered: true }) => {
                        self.waiting = None;
                        self.stage = Stage::Bind;
                    }
                    Ok(Readiness::Ready { undelivered: false }) => {
                        self.waiting = None;
                        let receipt = crate::history::publish_verified(history, &verified, &[]);
                        self.stage = Stage::Done(receipt);
                    }
                }
            }
            Stage::Publish => {
                let verified = Arc::clone(self.verified.as_ref().unwrap());
                // Dependencies only accumulate and placement depends only on
                // existing ancestry, so readiness established earlier still holds.
                let receipt = crate::history::publish_verified(history, &verified, &self.payloads);
                self.stage = Stage::Done(receipt);
            }
            _ => {}
        }
        self.next()
    }
}

static WRONG_HISTORY: Error = Error::WrongHistory;
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_ticket() -> Option<u64> {
    NEXT_TICKET
        .fetch_update(Relaxed, Relaxed, |t| t.checked_add(1))
        .ok()
}

impl Admission {
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The parsed header, once read. Untrusted until admission completes.
    pub fn meta(&self) -> Option<&Arc<BundleMeta>> {
        self.meta.as_ref()
    }

    /// The outstanding request, if any, for hosts that need to see it again.
    pub fn request(&self) -> Option<(Ticket, Request<'_>)> {
        let o = self.outstanding?;
        Some((Ticket(o.ticket), self.describe(o)))
    }

    fn describe(&self, o: Outstanding) -> Request<'_> {
        match o.kind {
            Kind::Read => Request::Read {
                offset: self.offset,
                max: o.max,
            },
            Kind::Decode => Request::Decode {
                meta: self.meta.as_ref().unwrap(),
                input: &self.input[self.input_start..],
                eof: o.eof,
                capacity: o.max,
            },
            Kind::Bind => Request::Bind {
                meta: self.meta.as_ref().unwrap(),
            },
        }
    }

    fn fail(&mut self, error: Error) {
        if !matches!(self.stage, Stage::Done(_) | Stage::Failed(_)) {
            self.outstanding = None;
            self.stage = Stage::Failed(error);
        }
    }

    fn issue(&mut self, kind: Kind, max: usize, eof: bool) -> Next<'_> {
        // Tickets are unique across operations, so a reply meant for another
        // operation is always stale.
        let Some(ticket) = next_ticket() else {
            self.fail(Error::Exhausted("request tickets"));
            return self.next();
        };
        self.counts.requests += 1;
        let o = Outstanding {
            ticket,
            kind,
            max,
            eof,
        };
        self.outstanding = Some(o);
        Next::Request(Ticket(ticket), self.describe(o))
    }

    /// Advance without the history: hashing, verification, and choosing the
    /// next host request. Does at most one work quantum.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Next<'_> {
        if let Err(e) = self.step() {
            self.fail(e);
        }
        if matches!(
            self.stage,
            Stage::Done(_) | Stage::Failed(_) | Stage::Place | Stage::Publish
        ) {
            return match &self.stage {
                Stage::Done(receipt) => Next::Done(receipt),
                Stage::Failed(error) => Next::Failed(error),
                _ => Next::NeedHistory,
            };
        }
        if self.outstanding.is_some() {
            return Next::AwaitReply;
        }
        match self.stage {
            Stage::Header { .. } => {
                let max = self.options.read_size;
                self.issue(Kind::Read, max, false)
            }
            Stage::Body => self.body_request(),
            Stage::Verify => Next::Continue,
            Stage::Bind => self.issue(Kind::Bind, 0, false),
            _ => unreachable!(),
        }
    }

    /// Internal work. Leaves the stage ready for choosing a request.
    fn step(&mut self) -> Result<(), Error> {
        let mut budget = self.options.quantum;
        match &mut self.stage {
            Stage::Header { buffer } => {
                let Some(len) = crate::wire::header_len(buffer, &self.options.limits)? else {
                    if self.eof {
                        return Err(Error::Malformed("truncated header"));
                    }
                    return Ok(());
                };
                if buffer.len() < len {
                    if self.eof {
                        return Err(Error::Malformed("truncated header"));
                    }
                    return Ok(());
                }
                let rest = buffer.split_off(len);
                let meta = Arc::new(crate::wire::decode_header(
                    buffer,
                    &self.options.format,
                    &self.options.limits,
                )?);
                self.verifier = Some(Verifier::new(Arc::clone(&meta), &self.options.limits)?);
                self.meta = Some(meta);
                self.input = rest;
                self.input_start = 0;
                self.stage = Stage::Body;
                Ok(())
            }
            Stage::Body => {
                let verifier = self.verifier.as_mut().unwrap();
                while self.canonical_start < self.canonical.len() && budget > 0 {
                    let end = self.canonical.len().min(self.canonical_start + budget);
                    verifier.update(&self.canonical[self.canonical_start..end])?;
                    budget -= end - self.canonical_start;
                    self.counts.canonical_bytes += (end - self.canonical_start) as u64;
                    self.canonical_start = end;
                }
                if self.canonical_start < self.canonical.len() {
                    return Ok(());
                }
                match self.pending.take() {
                    Some(DecodeStatus::EndMember) => verifier.end_member()?,
                    Some(DecodeStatus::Finished) => self.finished = true,
                    _ => {}
                }
                if self.finished && self.input_start < self.input.len() {
                    return Err(Error::InvalidItem("bytes after the decoder finished"));
                }
                if self.finished && self.eof {
                    if verifier.members_ended() != verifier.meta().len() {
                        return Err(Error::InvalidItem("decoder finished before every member"));
                    }
                    self.input = Vec::new();
                    self.canonical = Vec::new();
                    self.stage = Stage::Verify;
                }
                Ok(())
            }
            Stage::Verify => {
                let verifier = self.verifier.as_mut().unwrap();
                verifier.finish(budget)?;
                if let Some(verified) = verifier.result() {
                    self.verified = Some(Arc::clone(verified));
                    self.verifier = None;
                    self.stage = Stage::Place;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn body_request(&mut self) -> Next<'_> {
        if self.canonical_start < self.canonical.len() {
            return Next::Continue;
        }
        let have_input = self.input_start < self.input.len();
        if self.finished {
            // Confirm that nothing follows the body.
            let max = self.options.read_size;
            return self.issue(Kind::Read, max, false);
        }
        let need_input = matches!(self.last, None | Some(DecodeStatus::NeedInput)) && !have_input;
        if need_input && !self.eof {
            // Keep at most `read_size` undecoded bytes buffered.
            self.input.drain(..self.input_start);
            self.input_start = 0;
            let max = self.options.read_size;
            return self.issue(Kind::Read, max, false);
        }
        if need_input && self.offered_eof && self.last.is_some() {
            self.fail(Error::InvalidItem(
                "decoder needs input after the end of the body",
            ));
            return self.next();
        }
        let eof = self.eof;
        let capacity = self.options.decode_capacity;
        self.issue(Kind::Decode, capacity, eof)
    }

    /// Answer the outstanding request.
    pub fn reply(&mut self, ticket: Ticket, reply: Reply<'_>) -> Result<(), ReplyError> {
        let Some(o) = self.outstanding else {
            return Err(ReplyError::Stale);
        };
        if o.ticket != ticket.0 {
            return Err(ReplyError::Stale);
        }
        let kind = match reply {
            Reply::Read { .. } => Kind::Read,
            Reply::Decoded { .. } => Kind::Decode,
            Reply::Bound(_) => Kind::Bind,
            Reply::Failed(_) => o.kind,
        };
        if kind != o.kind {
            return Err(ReplyError::WrongKind);
        }
        self.outstanding = None;
        let result = self.accept(o, reply);
        if let Err(e) = &result {
            self.fail(e.clone());
        }
        result.map_err(ReplyError::Invalid)
    }

    fn accept(&mut self, o: Outstanding, reply: Reply<'_>) -> Result<(), Error> {
        match reply {
            Reply::Failed(message) => Err(match o.kind {
                Kind::Decode => Error::Codec(message.into()),
                Kind::Read | Kind::Bind => Error::Storage(message.into()),
            }),
            Reply::Read { bytes, eof } => {
                if bytes.len() > o.max {
                    return Err(Error::Protocol("read longer than requested"));
                }
                if bytes.is_empty() && !eof {
                    return Err(Error::Protocol("empty read without EOF"));
                }
                if self.eof && !bytes.is_empty() {
                    return Err(Error::Protocol("bytes after EOF"));
                }
                self.offset = self
                    .offset
                    .checked_add(bytes.len() as u64)
                    .filter(|&n| n <= self.options.limits.max_item_bytes)
                    .ok_or(Error::Limit("item bytes"))?;
                self.counts.bytes_read += bytes.len() as u64;
                self.eof |= eof;
                match &mut self.stage {
                    Stage::Header { buffer } => {
                        let cap = self
                            .options
                            .limits
                            .max_header_bytes
                            .saturating_add(self.options.read_size)
                            .saturating_add(16);
                        if buffer.len() + bytes.len() > cap {
                            return Err(Error::Limit("header bytes"));
                        }
                        buffer.extend_from_slice(bytes);
                    }
                    _ if self.finished && !bytes.is_empty() => {
                        return Err(Error::InvalidItem("bytes after the decoder finished"));
                    }
                    _ => self.input.extend_from_slice(bytes),
                }
                Ok(())
            }
            Reply::Decoded {
                consumed,
                output,
                status,
            } => {
                let offered = self.input.len() - self.input_start;
                if consumed > offered {
                    return Err(Error::Protocol("decoder consumed more than offered"));
                }
                if output.len() > o.max {
                    return Err(Error::Protocol("decoder output exceeds capacity"));
                }
                if status == DecodeStatus::NeedInput && consumed != offered {
                    return Err(Error::Protocol("NeedInput without consuming all input"));
                }
                if status == DecodeStatus::Working && consumed == 0 && output.is_empty() {
                    return Err(Error::Protocol("decoder made no progress"));
                }
                self.input_start += consumed;
                self.counts.encoded_bytes_decoded += consumed as u64;
                self.canonical.clear();
                self.canonical.extend_from_slice(output);
                self.canonical_start = 0;
                self.pending = Some(status);
                self.last = Some(status);
                self.offered_eof = o.eof;
                Ok(())
            }
            Reply::Bound(ids) => {
                let n = self.meta.as_ref().unwrap().len();
                if ids.len() != n {
                    return Err(Error::Protocol("wrong number of payload IDs"));
                }
                self.payloads = ids.to_vec();
                self.stage = Stage::Publish;
                Ok(())
            }
        }
    }
}
