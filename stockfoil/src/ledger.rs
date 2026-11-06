//! One document's authenticated history and policy, without its payload bytes.
use crate::{
    Admission, Ancestry, Author, CommitBuilder, CommitHash, Commits, Error, ExportCursor,
    ExportPlan, Follow, FormatId, Heads, History, ItemId, Limits, Next, PayloadId, Prepared, Queue,
    QueueNext, Receipt, ReceiveOptions, Reply, Signature, Signer, ViewPoint, WritePermission,
    WritePermissions,
};
use std::sync::Arc;

/// Everything about one document except its bytes.
///
/// The ledger owns history, write permissions, local drafts and a bounded queue
/// of verified items awaiting dependencies. Payloads are only caller-assigned
/// IDs. It never performs I/O, buffers a payload, or awaits a future.
///
/// Receive by servicing a [`begin_receive`](Self::begin_receive) admission's
/// read/decode requests, then [`admit`](Self::admit) it. Commit by feeding a
/// [`CommitBuilder`] and your storage together, then [`commit`](Self::commit)
/// with the stored payload's ID. Export describes members and a wire header;
/// you encode from your storage with your own buffers and runtime.
///
/// For payloads in memory, use [`Document`](crate::Document) instead.
pub struct Ledger {
    history: History,
    author: Author,
    options: ReceiveOptions,
    permissions: WritePermissions,
    policy_epoch: u64,
    queue: Queue<ItemId>,
}

impl Ledger {
    /// Empty history, granting only the local author unbounded participation.
    pub fn new(format: FormatId, author: Author) -> Self {
        Self {
            history: History::new(),
            author,
            options: ReceiveOptions::new(format),
            permissions: WritePermissions::from([(author, WritePermission::Unbounded)]),
            policy_epoch: 0,
            queue: Queue::new(),
        }
    }
    pub fn with_permissions(mut self, permissions: WritePermissions) -> Self {
        self.set_permissions(permissions);
        self
    }
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.options.limits = limits;
        self
    }
    /// Configure before receiving any items. Default: 1024; zero declines parking.
    pub fn with_max_parked(mut self, max: usize) -> Self {
        assert!(self.queue.is_empty(), "configure parking before receiving");
        self.queue = Queue::with_capacity(max);
        self
    }
    pub fn author(&self) -> Author {
        self.author
    }
    pub fn format(&self) -> &FormatId {
        &self.options.format
    }
    pub fn history(&self) -> &History {
        &self.history
    }
    pub fn heads(&self, which: Heads) -> Vec<CommitHash> {
        self.history.heads(which).collect()
    }
    pub fn len(&self) -> usize {
        self.history.len()
    }
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
    pub fn epoch(&self) -> u64 {
        self.history
            .epoch()
            .checked_add(self.policy_epoch)
            .expect("epoch space")
    }
    pub fn contains(&self, hash: &CommitHash) -> bool {
        self.history.lookup(hash).is_some()
    }
    pub fn is_admitted(&self, hash: &CommitHash) -> bool {
        self.history.is_admitted(hash)
    }
    pub fn author_of(&self, hash: &CommitHash) -> Option<Author> {
        self.history.lookup(hash).map(|i| i.author)
    }
    pub fn parents(&self, hash: &CommitHash) -> Option<Vec<CommitHash>> {
        self.history.parents(hash).map(Iterator::collect)
    }
    pub fn is_ancestor(&self, a: &CommitHash, d: &CommitHash) -> bool {
        self.history.is_ancestor(a, d)
    }
    pub fn payload_id(&self, hash: &CommitHash) -> Option<PayloadId> {
        self.history.lookup(hash).map(|i| i.payload)
    }
    pub fn ancestry(&self, heads: &[CommitHash]) -> Result<Ancestry, Error> {
        self.history.ancestry(heads, &self.permissions)
    }
    pub fn commits_since(&self, heads: &[CommitHash]) -> Result<Commits<'_>, Error> {
        self.history.commits_since(heads)
    }
    pub fn permissions(&self) -> &WritePermissions {
        &self.permissions
    }
    /// Local policy, not exported. Materializations observe it on their next plan.
    pub fn set_permissions(&mut self, permissions: WritePermissions) {
        if self.permissions != permissions {
            self.permissions = permissions;
            self.policy_epoch = self.policy_epoch.checked_add(1).expect("epoch space");
        }
    }
    pub fn view(&self, follow: &Follow) -> ViewPoint {
        let heads = match follow {
            Follow::Working => self.heads(Heads::Delivered),
            Follow::Signed => self.heads(Heads::Admitted),
            Follow::Pinned(heads) => heads.clone(),
        };
        ViewPoint::from_parts(heads, self.permissions.clone())
    }
    /// Feed canonical bytes to this builder and your storage in any chunking.
    pub fn begin_commit(&self, parents: Option<Vec<CommitHash>>) -> Result<CommitBuilder, Error> {
        self.history.begin_commit(
            self.author,
            parents.unwrap_or_else(|| self.heads(Heads::Delivered)),
        )
    }
    /// Deliver a local draft. Unsigned drafts are not durable: persist a local
    /// journal if signing may be delayed across a restart.
    pub fn commit(&mut self, prepared: Prepared, payload: PayloadId) -> Result<Draft, Error> {
        if prepared.author != self.author {
            return Err(Error::Protocol(
                "prepared commit has a different local author",
            ));
        }
        let draft = Draft {
            hash: prepared.hash,
            author: prepared.author,
            parents: prepared.parents.clone(),
        };
        self.history.publish_commit(prepared, payload)?;
        Ok(draft)
    }
    pub fn drafts(&self) -> impl Iterator<Item = Draft> + '_ {
        self.history
            .drafts()
            .filter(|hash| self.author_of(hash) == Some(self.author))
            .map(|hash| Draft {
                hash,
                author: self.author,
                parents: self.parents(&hash).unwrap(),
            })
    }
    pub fn has_drafts(&self) -> bool {
        self.drafts().next().is_some()
    }
    pub fn sign_drafts(
        &mut self,
        signer: &impl Signer,
        bind: &mut dyn Bind,
    ) -> Result<Signed, Error> {
        let drafts: Vec<_> = self
            .history
            .drafts()
            .filter(|h| self.author_of(h) == Some(signer.author()))
            .collect();
        let mut outcome = Signed::default();
        for hash in drafts {
            if self.is_admitted(&hash) {
                continue;
            }
            let draft = Draft {
                hash,
                author: signer.author(),
                parents: self.parents(&hash).unwrap(),
            };
            let signed = self.attach_signature(&draft, signer.sign(&hash), bind)?;
            outcome.admitted.extend(signed.admitted);
            outcome.released.extend(signed.released);
            outcome.failed.extend(signed.failed);
        }
        Ok(outcome)
    }
    /// Sign elsewhere (including asynchronously), then attach the result here.
    pub fn attach_signature(
        &mut self,
        draft: &Draft,
        signature: Signature,
        bind: &mut dyn Bind,
    ) -> Result<Signed, Error> {
        if self.is_admitted(&draft.hash) {
            return Ok(Signed::default());
        }
        let before: Vec<_> = self.history.drafts().collect();
        self.history.sign(draft.author, draft.hash, signature)?;
        let (released, failed) = self.drain(bind)?;
        let admitted = before.into_iter().filter(|h| self.is_admitted(h)).collect();
        Ok(Signed {
            admitted,
            released,
            failed,
        })
    }
    /// Start bounded-buffer verification. Persist the item before applying its
    /// commits if materialization checkpoints must survive a restart.
    pub fn begin_receive(&self) -> Admission {
        self.history.receive(self.options.clone())
    }
    /// Clone the defaults to tune read/decode buffers and work quantum.
    pub fn receive_options(&self) -> ReceiveOptions {
        self.options.clone()
    }
    /// Start with caller-selected buffer sizes and limits, preserving this
    /// document's trusted format. Replies may always use smaller buffers.
    pub fn begin_receive_with(&self, options: ReceiveOptions) -> Result<Admission, Error> {
        if options.format != *self.format() {
            return Err(Error::WrongFormat);
        }
        Ok(self.history.receive(options))
    }
    pub(crate) fn begin_memory_receive(&self, len: usize) -> Admission {
        let mut options = self.options.clone();
        options.read_size = len.max(1);
        self.history.receive(options)
    }
    /// Place a byte-verified admission, binding payload IDs only on publication.
    /// Parking/eviction never binds. Returned receipts include released items
    /// so transport metadata can be checked against their actual members.
    pub fn admit(&mut self, mut op: Admission, bind: &mut dyn Bind) -> Result<Received, Error> {
        if op.format() != self.format() {
            return Err(Error::WrongFormat);
        }
        if op.members().is_none() {
            return Err(Error::Protocol(
                "admit expects byte verification to be complete",
            ));
        }
        loop {
            match self.history.advance(&mut op) {
                Next::Waiting(waiting) => {
                    let id = op.meta().unwrap().id.clone();
                    return Ok(match self.queue.push(waiting, id, op)? {
                        None => Received::Waiting(waiting),
                        Some(evicted) => Received::Evicted {
                            waiting,
                            evicted: evicted.id,
                        },
                    });
                }
                Next::Request(ticket, crate::Request::Bind { .. }) => {
                    let ids = bind.bind(&op.meta().unwrap().id, op.members().unwrap());
                    op.reply(ticket, Reply::Bound(&ids)).map_err(reply_error)?;
                }
                Next::Done(receipt) => {
                    let mut items = vec![receipt.clone()];
                    let (released, failed) = self.drain(bind)?;
                    items.extend(released);
                    let delivered = items
                        .iter()
                        .flat_map(|r| &r.members)
                        .filter(|m| m.newly_delivered)
                        .map(|m| m.hash)
                        .collect();
                    return Ok(Received::Admitted {
                        items,
                        delivered,
                        failed,
                    });
                }
                Next::Failed(error) => return Err(error.clone()),
                other => {
                    return Err(Error::Protocol(match other {
                        Next::Request(..) | Next::AwaitReply => {
                            "admission still needs byte replies"
                        }
                        _ => "admission is not ready for publication",
                    }));
                }
            }
        }
    }
    #[allow(clippy::type_complexity)] // receipts plus per-item placement failures
    fn drain(
        &mut self,
        bind: &mut dyn Bind,
    ) -> Result<(Vec<Receipt>, Vec<(ItemId, Error)>), Error> {
        let mut receipts = Vec::new();
        let mut failed = Vec::new();
        while let Some(next) = self.queue.poll(&mut self.history) {
            match next {
                QueueNext::Done { receipt, .. } => receipts.push(receipt),
                QueueNext::Failed { context, error } => failed.push((context, error)),
                QueueNext::Bind {
                    context,
                    mut op,
                    ticket,
                    ..
                } => {
                    let ids = bind.bind(&context, op.members().unwrap());
                    op.reply(ticket, Reply::Bound(&ids)).map_err(reply_error)?;
                    match self.history.advance(&mut op) {
                        Next::Done(receipt) => receipts.push(receipt.clone()),
                        Next::Failed(error) => failed.push((context, error.clone())),
                        _ => return Err(Error::Protocol("bound item did not publish")),
                    }
                }
            }
        }
        Ok((receipts, failed))
    }
    pub fn missing(&self) -> Vec<CommitHash> {
        self.queue.missing().collect()
    }
    pub fn parked(&self) -> usize {
        self.queue.len()
    }
    pub fn export(&self, cursor: &ExportCursor) -> ExportPlan {
        self.history.export(cursor)
    }
    pub fn export_item(&self, plan: &ExportPlan, index: usize) -> Result<ExportedItem, Error> {
        let item = self.history.export_item(plan, index)?;
        let checkpoints = match &item.meta.id {
            ItemId::Commit(_) => Vec::new(),
            ItemId::Fragment(range) => item
                .members
                .iter()
                .map(|(h, _)| *h)
                .filter(|h| *h != range.head && crate::meta::depth(h) > 0)
                .collect(),
        };
        Ok(ExportedItem {
            id: item.meta.id.clone(),
            header: item.encode_header(self.format()),
            members: item.members,
            checkpoints,
            meta: item.meta,
        })
    }
}

pub(crate) fn reply_error(error: crate::ReplyError) -> Error {
    match error {
        crate::ReplyError::Invalid(error) => error,
        _ => Error::Protocol("invalid admission reply"),
    }
}

/// Answers which IDs were assigned to a verified item's stored payloads. It
/// does not perform I/O: stage bytes while decoding, then return their IDs.
pub trait Bind {
    fn bind(&mut self, item: &ItemId, members: &[CommitHash]) -> Vec<PayloadId>;
}
impl<F: FnMut(&ItemId, &[CommitHash]) -> Vec<PayloadId>> Bind for F {
    fn bind(&mut self, item: &ItemId, members: &[CommitHash]) -> Vec<PayloadId> {
        self(item, members)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub hash: CommitHash,
    pub author: Author,
    pub parents: Vec<CommitHash>,
}
impl Draft {
    pub fn signing_bytes(&self) -> &[u8; 32] {
        self.hash.as_bytes()
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signed {
    pub admitted: Vec<CommitHash>,
    pub released: Vec<Receipt>,
    /// Parked items whose placement failed after dependencies arrived.
    pub failed: Vec<(ItemId, Error)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Admitted {
        items: Vec<Receipt>,
        delivered: Vec<CommitHash>,
        failed: Vec<(ItemId, Error)>,
    },
    Waiting(CommitHash),
    Evicted {
        waiting: CommitHash,
        evicted: ItemId,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedItem {
    pub id: ItemId,
    pub members: Vec<(CommitHash, PayloadId)>,
    pub checkpoints: Vec<CommitHash>,
    pub header: Vec<u8>,
    pub meta: Arc<crate::BundleMeta>,
}
impl ExportedItem {
    pub fn head(&self) -> CommitHash {
        self.id.head()
    }
}
