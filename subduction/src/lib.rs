//! Mapping between Stockfoil items and Subduction blobs + sedimentree metadata.
//!
//! Subduction stores each Stockfoil item as one [`Blob`], described by
//! sedimentree metadata: a [`LooseCommit`] for a standalone commit, a
//! [`Fragment`] for a fragment.
//!
//! - [`Metadata`], the sedimentree description of an item. Publishing uses it
//!   to build what Subduction stores. Receiving uses it to check that what
//!   Subduction claimed about a blob is what Stockfoil admitted from it:
//!   Subduction cannot check its metadata against contents, which may be
//!   encrypted, and it decides what to sync and prune from that metadata.
//! - [`export_item`] and [`export_batch`] encode planned items from a [`Document`]
//!   alongside their [`Metadata`], ready for Subduction's `store_built_batch`.
use sedimentree_core::blob::{Blob, BlobMeta};
use sedimentree_core::crypto::digest::Digest;
use sedimentree_core::fragment::{Fragment, InvalidFragment};
use sedimentree_core::id::SedimentreeId;
use sedimentree_core::loose_commit::id::CommitId;
use sedimentree_core::loose_commit::{InvalidLooseCommit, LooseCommit};
use std::fmt;
use stockfoil::{
    Codec, CommitHash, Document, ExportPlan, ExportedItem, History, ItemId, ParentRef,
};

/// A Subduction blob digest.
pub type BlobDigest = Digest<Blob>;

/// The sedimentree metadata describing one Stockfoil item.
///
/// - A standalone commit is a [`LooseCommit`] with the commit's hash as its
///   head and its distinct parents.
/// - A fragment is a [`Fragment`] with the same head and boundary. Its
///   checkpoints are its members of nonzero depth other than the head: every
///   commit a smaller fragment inside it can start or stop at, which is what
///   Subduction's minimization needs to see that this fragment covers it.
///
/// Peers syncing a document must agree on this mapping, like the format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metadata {
    Commit(LooseCommit),
    Fragment(Fragment),
}

impl Metadata {
    /// Describe a ledger's exported item without reading payloads or borrowing
    /// the history. The caller can use this with any sedimentree transport.
    pub fn from_exported(
        tree: SedimentreeId,
        item: &ExportedItem,
        blob: BlobMeta,
    ) -> Result<Self, Error> {
        match &item.id {
            ItemId::Commit(hash) => {
                let member = item
                    .meta
                    .members
                    .first()
                    .ok_or(stockfoil::Error::Protocol("export has no members"))?;
                let parents = member
                    .parents
                    .iter()
                    .map(|p| match p {
                        ParentRef::External(hash) => Ok(commit_id(hash)),
                        ParentRef::Member(_) => Err(stockfoil::Error::Protocol(
                            "standalone export has an internal parent",
                        )),
                    })
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
                LooseCommit::new(tree, commit_id(hash), parents, blob)
                    .map(Self::Commit)
                    .map_err(Error::Commit)
            }
            ItemId::Fragment(range) => Fragment::new(
                tree,
                commit_id(&range.head),
                range.boundary.iter().map(commit_id).collect(),
                &item.checkpoints.iter().map(commit_id).collect::<Vec<_>>(),
                blob,
            )
            .map(Self::Fragment)
            .map_err(Error::Fragment),
        }
    }

    /// Describe item `id` of `history`, whose members (in bundle order) are
    /// `members`, stored in `tree` as the blob `blob` describes. Every member
    /// must be delivered. Fails if the item exceeds Subduction's limits.
    pub fn describe(
        tree: SedimentreeId,
        history: &History,
        id: &ItemId,
        members: &[CommitHash],
        blob: BlobMeta,
    ) -> Result<Self, Error> {
        match id {
            ItemId::Commit(hash) => {
                let parents = history
                    .parents(hash)
                    .ok_or(stockfoil::Error::UnknownCommit(*hash))?
                    .map(|p| commit_id(&p))
                    .collect();
                LooseCommit::new(tree, commit_id(hash), parents, blob)
                    .map(Self::Commit)
                    .map_err(Error::Commit)
            }
            ItemId::Fragment(range) => {
                let checkpoints: Vec<_> = members
                    .iter()
                    .filter(|m| **m != range.head && depth(m) > 0)
                    .map(commit_id)
                    .collect();
                Fragment::new(
                    tree,
                    commit_id(&range.head),
                    range.boundary.iter().map(commit_id).collect(),
                    &checkpoints,
                    blob,
                )
                .map(Self::Fragment)
                .map_err(Error::Fragment)
            }
        }
    }

    pub fn tree(&self) -> SedimentreeId {
        match self {
            Self::Commit(c) => c.sedimentree_id(),
            Self::Fragment(f) => f.sedimentree_id(),
        }
    }

    pub fn head(&self) -> CommitId {
        match self {
            Self::Commit(c) => c.head(),
            Self::Fragment(f) => f.head(),
        }
    }

    pub fn blob_meta(&self) -> BlobMeta {
        match self {
            Self::Commit(c) => *c.blob_meta(),
            Self::Fragment(f) => f.summary().blob_meta(),
        }
    }

    /// Check that this metadata, as Subduction holds it, describes the item
    /// Stockfoil admitted from its blob: `id` and `members` from the
    /// admission's [`Receipt`]. A mismatch means a writer
    /// lied about the blob, which could make Subduction prune or skip history
    /// the blob does not contain; the adapter should reject the blob.
    pub fn check(
        &self,
        history: &History,
        id: &ItemId,
        members: &[CommitHash],
    ) -> Result<(), Error> {
        let expected = Self::describe(self.tree(), history, id, members, self.blob_meta())?;
        if &expected == self {
            Ok(())
        } else {
            Err(Error::Mismatch(id.head()))
        }
    }
}

/// Write planned item `index` as a Subduction blob with its metadata, ready
/// for Subduction's `store_built_batch` or `add_built_batch`. `seal` turns
/// the item's bytes into the blob Subduction stores: [`Blob::new`] without
/// encryption, or an encryption whose ciphertext the metadata then describes.
pub fn export_item<C: Codec>(
    graph: &Document<C>,
    plan: &ExportPlan,
    index: usize,
    tree: SedimentreeId,
    seal: impl FnOnce(Vec<u8>) -> Blob,
) -> Result<(Metadata, Blob), Error> {
    let (exported, bytes) = graph.export_item(plan, index)?;
    let blob = seal(bytes);
    let metadata = Metadata::from_exported(tree, &exported, blob.meta())?;
    Ok((metadata, blob))
}

/// Subduction batch inputs: `(commits, fragments)` for `store_built_batch`.
pub type BuiltBatch = (Vec<(LooseCommit, Blob)>, Vec<(Fragment, Blob)>);

/// Write every unacknowledged item in `plan` as Subduction `(LooseCommit, Blob)`
/// and `(Fragment, Blob)` batches, ready for `store_built_batch`.
pub fn export_batch<C: Codec>(
    graph: &Document<C>,
    plan: &ExportPlan,
    tree: SedimentreeId,
    mut seal: impl FnMut(Vec<u8>) -> Blob,
) -> Result<BuiltBatch, Error> {
    let mut commits = Vec::new();
    let mut fragments = Vec::new();
    for index in 0..plan.len() {
        match export_item(graph, plan, index, tree, &mut seal)? {
            (Metadata::Commit(c), blob) => commits.push((c, blob)),
            (Metadata::Fragment(f), blob) => fragments.push((f, blob)),
        }
    }
    Ok((commits, fragments))
}

pub fn commit_id(hash: &CommitHash) -> CommitId {
    CommitId::new(*hash.as_bytes())
}

/// Leading zero bytes, the depth rule Stockfoil and Subduction share.
fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes().iter().take_while(|b| **b == 0).count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Stockfoil(stockfoil::Error),
    /// The commit has more distinct parents than Subduction allows.
    Commit(InvalidLooseCommit),
    /// The fragment has more boundary commits or checkpoints than Subduction
    /// allows.
    Fragment(InvalidFragment),
    /// Subduction's metadata does not describe the item with this head.
    Mismatch(CommitHash),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stockfoil(e) => e.fmt(f),
            Self::Commit(e) => e.fmt(f),
            Self::Fragment(e) => e.fmt(f),
            Self::Mismatch(head) => {
                write!(f, "Subduction metadata does not describe item {head:?}")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<stockfoil::Error> for Error {
    fn from(e: stockfoil::Error) -> Self {
        Self::Stockfoil(e)
    }
}

use future_form::Sendable;
use futures::future::BoxFuture;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::Mutex;
use stockfoil::{Receipt, Received};
use subduction_core::{
    handshake::{self, Handshake},
    timestamp::TimestampSeconds,
    transport::Transport,
};
use subduction_crypto::nonce::Nonce;
use subduction_tokio::{
    AttemptScope, AuthenticationContext, Connector, Error as TokioError, ErrorKind,
    EstablishedConnection, StoredItem,
};

impl Metadata {
    /// Extract `(Metadata, Blob)` from a [`StoredItem`] yielded by `load_and_watch`.
    pub fn from_stored(item: &StoredItem) -> (Self, Blob) {
        match item {
            StoredItem::Commit(v) => (Self::Commit(v.payload().clone()), v.blob().clone()),
            StoredItem::Fragment(v) => (Self::Fragment(v.payload().clone()), v.blob().clone()),
        }
    }
}

/// Check a completed [`Receipt`] against any pending [`Metadata`] for its head.
pub fn check_receipt(
    history: &History,
    pending: &mut HashMap<CommitHash, Metadata>,
    receipt: &Receipt,
) -> Result<(), Error> {
    if let Some(metadata) = pending.remove(&receipt.id.head()) {
        let members: Vec<_> = receipt.members.iter().map(|m| m.hash).collect();
        metadata.check(history, &receipt.id, &members)?;
    }
    Ok(())
}

/// Receive a [`StoredItem`] (from `load_and_watch`) into `graph`, checking its
/// [`Metadata`] immediately if admitted or recording it in `pending` if the
/// item is queued waiting for a dependency.
pub fn receive_stored<C: Codec>(
    graph: &mut Document<C>,
    pending: &mut HashMap<CommitHash, Metadata>,
    item: &StoredItem,
) -> Result<Received, Error> {
    let (metadata, blob) = Metadata::from_stored(item);
    let head = CommitHash::from(*metadata.head().as_bytes());
    pending.insert(head, metadata);
    match graph.receive(blob.as_slice()) {
        Ok(Received::Admitted {
            items,
            delivered,
            failed,
        }) => {
            for r in &items {
                check_receipt(graph.history(), pending, r)?;
            }
            for (id, _) in &failed {
                pending.remove(&id.head());
            }
            Ok(Received::Admitted {
                items,
                delivered,
                failed,
            })
        }
        Ok(Received::Waiting(dep)) => Ok(Received::Waiting(dep)),
        Ok(Received::Evicted { waiting, evicted }) => {
            // A repeated item can evict an earlier copy of itself. Keep the
            // incoming copy's metadata so its eventual admission is checked.
            if evicted.head() != head {
                pending.remove(&evicted.head());
            }
            Ok(Received::Evicted { waiting, evicted })
        }
        Err(e) => {
            pending.remove(&head);
            Err(Error::Stockfoil(e))
        }
    }
}

/// An in-memory full-duplex byte channel for [`subduction_tokio`] connections.
#[derive(Clone, Debug)]
pub struct Channel {
    tx: async_channel::Sender<Vec<u8>>,
    rx: async_channel::Receiver<Vec<u8>>,
}

impl PartialEq for Channel {
    fn eq(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }
}

impl Transport<Sendable> for Channel {
    type SendError = io::Error;
    type RecvError = io::Error;
    type DisconnectionError = io::Error;

    fn send_bytes(&self, bytes: &[u8]) -> BoxFuture<'_, Result<(), io::Error>> {
        let bytes = bytes.to_vec();
        Box::pin(async move { self.tx.send(bytes).await.map_err(io::Error::other) })
    }

    fn recv_bytes(&self) -> BoxFuture<'_, Result<Vec<u8>, io::Error>> {
        Box::pin(async move { self.rx.recv().await.map_err(io::Error::other) })
    }

    fn disconnect(&self) -> BoxFuture<'_, Result<(), io::Error>> {
        Box::pin(async move {
            self.tx.close();
            self.rx.close();
            Ok(())
        })
    }
}

impl Handshake<Sendable> for Channel {
    type Error = io::Error;

    fn send(&mut self, bytes: Vec<u8>) -> BoxFuture<'_, Result<(), io::Error>> {
        let tx = self.tx.clone();
        Box::pin(async move { tx.send(bytes).await.map_err(io::Error::other) })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Vec<u8>, io::Error>> {
        let rx = self.rx.clone();
        Box::pin(async move { rx.recv().await.map_err(io::Error::other) })
    }
}

/// Create a connected pair of in-memory [`Channel`] endpoints.
pub fn channel_pair() -> (Channel, Channel) {
    let (tx1, rx1) = async_channel::bounded(64);
    let (tx2, rx2) = async_channel::bounded(64);
    (Channel { tx: tx1, rx: rx2 }, Channel { tx: tx2, rx: rx1 })
}

/// A [`subduction_tokio::Connector`] backed by one or more [`Channel`]s,
/// performing a real Ed25519 Subduction handshake on each attempt.
pub struct ChannelConnector {
    channels: Mutex<VecDeque<Channel>>,
}

impl ChannelConnector {
    pub fn new(channels: impl IntoIterator<Item = Channel>) -> Self {
        Self {
            channels: Mutex::new(channels.into_iter().collect()),
        }
    }
}

impl Connector for ChannelConnector {
    fn establish(
        &self,
        attempt: AttemptScope,
        authentication: AuthenticationContext,
    ) -> BoxFuture<'_, Result<EstablishedConnection, TokioError>> {
        Box::pin(async move {
            let channel = self
                .channels
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TokioError::new(ErrorKind::Transport, "no more channels"))?;
            let (authenticated, ()) = handshake::initiate::<Sendable, _, _, _, _>(
                channel,
                |c, _| (c, ()),
                authentication.signer(),
                authentication.audience(),
                TimestampSeconds::new(1),
                Nonce::random(),
            )
            .await
            .map_err(|e| TokioError::with_source(ErrorKind::Authentication, "handshake", e))?;
            attempt.established(authenticated)
        })
    }
}
