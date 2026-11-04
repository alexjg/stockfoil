//! Decoding wanted member payloads from a stored item through host requests.
//!
//! A [`DecodeItem`] owns its [`PayloadCheck`]s and buffers and never borrows
//! the history after creation. It asks the host to read the stored item and
//! run the application decoder, stops as soon as the last wanted member ends,
//! and checks every wanted member's canonical bytes against its commit.
use crate::admission::next_ticket;
use crate::{
    BundleMeta, CommitHash, Counts, DecodeStatus, Error, History, PayloadCheck, ReceiveOptions,
    Reply, ReplyError, Request, Ticket,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What the host should do next when driving a [`DecodeItem`]. Exactly one applies.
#[derive(Debug)]
pub enum DecodeNext<'a> {
    /// Execute this `Read` or `Decode` request and answer it with [`DecodeItem::reply`].
    Request(Ticket, Request<'a>),
    /// A request is outstanding; nothing else can proceed until its reply.
    AwaitReply,
    /// Canonical bytes of a wanted member. Consumed by this call; call
    /// [`DecodeItem::next`] again to continue. Each wanted member is checked
    /// against its commit when it ends; on [`Error::CorruptPayload`] discard
    /// earlier chunks emitted for that member.
    Chunk { member: usize, bytes: &'a [u8] },
    /// Every wanted member has been decoded and checked against its commit.
    Done,
    /// Failed.
    Failed(&'a Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Decode,
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
    Done,
    Failed(Error),
}

/// Decode and check wanted members from one stored item. Created by [`History::decode_item`].
pub struct DecodeItem {
    options: ReceiveOptions,
    stage: Stage,
    meta: Option<Arc<BundleMeta>>,
    outstanding: Option<Outstanding>,
    offset: u64,
    eof: bool,
    input: Vec<u8>,
    input_start: usize,
    canonical: Vec<u8>,
    chunk_member: Option<usize>,
    pending: Option<DecodeStatus>,
    last: Option<DecodeStatus>,
    offered_eof: bool,
    member: usize,
    last_wanted: usize,
    checks: BTreeMap<usize, PayloadCheck>,
    counts: Counts,
}

impl std::fmt::Debug for DecodeItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeItem")
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

impl DecodeItem {
    pub(crate) fn new(
        history: &History,
        mut options: ReceiveOptions,
        wanted: impl IntoIterator<Item = (usize, CommitHash)>,
    ) -> Result<Self, Error> {
        const MAX_BUFFER: usize = 1 << 30;
        options.read_size = options.read_size.clamp(1, MAX_BUFFER);
        options.decode_capacity = options.decode_capacity.clamp(1, MAX_BUFFER);
        options.quantum = options.quantum.max(1);
        let mut checks = BTreeMap::new();
        for (index, commit) in wanted {
            let check = history.check_payload(&commit)?;
            if let Some(prev) = checks.insert(index, check)
                && prev.commit() != commit
            {
                return Err(Error::InvalidItem("conflicting commits for one member"));
            }
        }
        let (stage, last_wanted) = match checks.keys().next_back() {
            Some(&last) => (Stage::Header { buffer: Vec::new() }, last),
            None => (Stage::Done, 0),
        };
        Ok(Self {
            options,
            stage,
            meta: None,
            outstanding: None,
            offset: 0,
            eof: false,
            input: Vec::new(),
            input_start: 0,
            canonical: Vec::new(),
            chunk_member: None,
            pending: None,
            last: None,
            offered_eof: false,
            member: 0,
            last_wanted,
            checks,
            counts: Counts::default(),
        })
    }
}

impl DecodeItem {
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The parsed header, once read.
    pub fn meta(&self) -> Option<&Arc<BundleMeta>> {
        self.meta.as_ref()
    }

    /// The outstanding request, if any, for hosts that need to see it again.
    pub fn request(&self) -> Option<(Ticket, Request<'_>)> {
        let o = self.outstanding?;
        Some((Ticket::from_raw(o.ticket), self.describe(o)))
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
        }
    }

    fn fail(&mut self, error: Error) {
        if !matches!(self.stage, Stage::Done | Stage::Failed(_)) {
            self.outstanding = None;
            self.chunk_member = None;
            self.stage = Stage::Failed(error);
        }
    }

    fn issue(&mut self, kind: Kind, max: usize, eof: bool) -> DecodeNext<'_> {
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
        DecodeNext::Request(Ticket::from_raw(ticket), self.describe(o))
    }

    /// Advance the operation: check decoded member bytes, emit verified chunks,
    /// or choose the next host request.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> DecodeNext<'_> {
        if let Err(e) = self.step() {
            self.fail(e);
        }
        if let Some(member) = self.chunk_member.take() {
            return DecodeNext::Chunk {
                member,
                bytes: &self.canonical,
            };
        }
        if matches!(self.stage, Stage::Done | Stage::Failed(_)) {
            return match &self.stage {
                Stage::Done => DecodeNext::Done,
                Stage::Failed(error) => DecodeNext::Failed(error),
                _ => unreachable!(),
            };
        }
        if self.outstanding.is_some() {
            return DecodeNext::AwaitReply;
        }
        match self.stage {
            Stage::Header { .. } => {
                let max = self.options.read_size;
                self.issue(Kind::Read, max, false)
            }
            Stage::Body => self.body_request(),
            Stage::Done | Stage::Failed(_) => unreachable!(),
        }
    }

    fn step(&mut self) -> Result<(), Error> {
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
                if let Some((&first, check)) = self.checks.iter().next()
                    && first >= meta.len()
                {
                    return Err(Error::CorruptPayload(check.commit()));
                }
                self.meta = Some(meta);
                self.input = rest;
                self.input_start = 0;
                self.stage = Stage::Body;
                Ok(())
            }
            Stage::Body => {
                let Some(status) = self.pending.take() else {
                    return Ok(());
                };
                if let Some(check) = self.checks.get_mut(&self.member)
                    && !self.canonical.is_empty()
                {
                    check.update(&self.canonical);
                    self.chunk_member = Some(self.member);
                }
                match status {
                    DecodeStatus::EndMember => {
                        if let Some(check) = self.checks.remove(&self.member) {
                            check.finish()?;
                        }
                        self.member += 1;
                        if self.member > self.last_wanted {
                            self.input = Vec::new();
                            self.stage = Stage::Done;
                        }
                    }
                    DecodeStatus::Finished => {
                        let (_, check) = self.checks.range(self.member..).next().unwrap();
                        return Err(Error::CorruptPayload(check.commit()));
                    }
                    DecodeStatus::NeedInput if self.offered_eof => {
                        return Err(Error::Codec("decoder needs input after end of body".into()));
                    }
                    _ => {}
                }
                Ok(())
            }
            Stage::Done | Stage::Failed(_) => Ok(()),
        }
    }

    fn body_request(&mut self) -> DecodeNext<'_> {
        let have_input = self.input_start < self.input.len();
        let need_input = matches!(self.last, None | Some(DecodeStatus::NeedInput)) && !have_input;
        if need_input && !self.eof {
            self.input.drain(..self.input_start);
            self.input_start = 0;
            let max = self.options.read_size;
            return self.issue(Kind::Read, max, false);
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
        if o.ticket != ticket.get() {
            return Err(ReplyError::Stale);
        }
        let kind = match reply {
            Reply::Read { .. } => Kind::Read,
            Reply::Decoded { .. } => Kind::Decode,
            Reply::Bound(_) => return Err(ReplyError::WrongKind),
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
                Kind::Read => Error::Storage(message.into()),
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
                self.counts.canonical_bytes += output.len() as u64;
                self.canonical.clear();
                self.canonical.extend_from_slice(output);
                self.pending = Some(status);
                self.last = Some(status);
                self.offered_eof = o.eof;
                Ok(())
            }
            Reply::Bound(_) => unreachable!(),
        }
    }
}
