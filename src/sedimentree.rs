//! Sedimentree synchronization API scaffolding.
//!
//! All operations are intentionally unimplemented. See
//! `design/sedimentree_sync_api.md` for the contracts these types describe.

use std::{collections::BTreeSet, error::Error, fmt, marker::PhantomData};

use crate::{Author, BlobHash, CommitContents, CommitHash, CommitMeta, Signature};

/// Inclusive head and boundary commits of a canonical fragment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FragmentRange {
    pub head: CommitHash,
    pub boundary: BTreeSet<CommitHash>,
}

/// Truncated interior commit identifier used for covering hints, not verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Checkpoint(pub [u8; 12]);

/// Hash and size of encoded transport bytes, including author proofs.
/// Not the canonical payload hash in a commit's metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobMeta {
    pub hash: BlobHash,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseCommit {
    pub commit: CommitHash,
    pub parents: BTreeSet<CommitHash>,
    pub blob: BlobMeta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub range: FragmentRange,
    pub checkpoints: BTreeSet<Checkpoint>,
    pub blob: BlobMeta,
}

/// Advertised items for one document. Untrusted until their blobs are verified.
/// Local summaries advertise only complete, sealed items, not unsigned drafts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SedimentreeSummary {
    pub loose_commits: Vec<LooseCommit>,
    pub fragments: Vec<Fragment>,
}

impl SedimentreeSummary {
    /// Pure comparison of causal coverage, not merely blob identity. Never
    /// mutates a graph or authorizes deletion of data on either peer.
    pub fn reconcile(&self, _remote: &Self) -> Result<SyncPlan, SummaryError> {
        todo!("sedimentree reconciliation")
    }
}

/// A borrowed export view of boundary indexes. Uses leading zero bytes as depth.
/// Only independently verifiable items in a publishable causal cover appear here.
pub struct SedimentreeView<'graph, P> {
    _graph: PhantomData<&'graph P>,
}

impl<P> SedimentreeView<'_, P> {
    pub fn loose_commits(&self) -> impl Iterator<Item = SignedCommitRef<'_, P>> {
        std::iter::from_fn(|| todo!("sealed loose commits"))
    }

    pub fn fragments(&self) -> impl Iterator<Item = FragmentRef<'_, P>> {
        std::iter::from_fn(|| todo!("sealed fragments"))
    }

    /// Look up a sealed canonical range, including one contained in a larger
    /// covering fragment. Allows saving smaller chunks without new signatures.
    pub fn fragment(&self, _range: &FragmentRange) -> Option<FragmentRef<'_, P>> {
        todo!("canonical fragment lookup")
    }

    /// Encode each fragment as a batch, including all permanent fragment proofs.
    pub fn encode<C: SyncCodec<P>>(
        &self,
        _codec: &mut C,
    ) -> Result<EncodedBatch, SyncError<C::Error>> {
        todo!("encode sedimentree")
    }
}

pub struct CommitRef<'a, P> {
    pub meta: &'a CommitMeta,
    pub payload: &'a P,
}

/// Direct author signature over this commit's own hash.
pub struct SignedCommitRef<'a, P> {
    pub commit: CommitRef<'a, P>,
    pub signature: &'a Signature,
}

/// A complete batch; encoding need not concatenate individual commit encodings.
pub struct FragmentRef<'graph, P> {
    _graph: PhantomData<&'graph P>,
}

impl<P> FragmentRef<'_, P> {
    pub fn range(&self) -> &FragmentRange {
        todo!("fragment range")
    }

    pub fn checkpoints(&self) -> &BTreeSet<Checkpoint> {
        todo!("fragment checkpoints")
    }

    /// Repeatable traversal of the full batch in deterministic causal order.
    pub fn commits(&self) -> impl Iterator<Item = CommitRef<'_, P>> {
        std::iter::from_fn(|| todo!("fragment commits"))
    }

    /// Includes proofs needed to reconstruct contained canonical fragments,
    /// even when they are redundant for authentication of the outer range.
    pub fn signatures(&self) -> impl Iterator<Item = &CommitSignature> {
        std::iter::from_fn(|| todo!("permanent fragment proofs"))
    }
}

#[derive(Debug, Clone)]
pub struct DecodedCommit<P> {
    pub contents: CommitContents,
    pub payload: P,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitSignature {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

#[derive(Debug, Clone)]
pub struct DecodedLooseCommit<P> {
    pub commit: DecodedCommit<P>,
    pub signature: Signature,
}

#[derive(Debug, Clone)]
pub struct DecodedFragment<P> {
    pub commits: Vec<DecodedCommit<P>>,
    pub signatures: Vec<CommitSignature>,
}

/// CRDT-defined whole-buffer encoding. Decoding is not verification.
///
/// Fragment encoding sees all commits together and may share tables or encode
/// columns/runs. It must reconstruct exact commit contents and every required
/// proof without consulting other fragments. Canonical payload bytes are hash
/// inputs, not a mandated physical layout for the compressed blob.
pub trait SyncCodec<P> {
    type Error;

    fn encode_commit(&mut self, commit: SignedCommitRef<'_, P>) -> Result<Vec<u8>, Self::Error>;
    fn encode_fragment(&mut self, fragment: FragmentRef<'_, P>) -> Result<Vec<u8>, Self::Error>;
    fn decode_commit(&mut self, bytes: &[u8]) -> Result<DecodedLooseCommit<P>, Self::Error>;
    fn decode_fragment(&mut self, bytes: &[u8]) -> Result<DecodedFragment<P>, Self::Error>;
    fn canonical_payload(&mut self, payload: &P) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemMeta {
    LooseCommit(LooseCommit),
    Fragment(Fragment),
}

/// An untrusted transport/storage object. Verification checks metadata, bytes,
/// exact range membership, and author coverage before anything enters the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedItem {
    pub metadata: ItemMeta,
    pub bytes: Vec<u8>,
}

/// Owns encoded items so advertised blob hashes can actually be served.
#[derive(Debug)]
pub struct EncodedBatch {
    _private: (),
}

impl EncodedBatch {
    pub fn items(&self) -> &[EncodedItem] {
        todo!("encoded sedimentree items")
    }

    pub fn summary(&self) -> SedimentreeSummary {
        todo!("encoded sedimentree summary")
    }

    /// Exact representation lookup; never searches unpublished working history.
    pub fn get(&self, _item: &ItemRef) -> Option<&EncodedItem> {
        todo!("encoded item lookup")
    }
}

/// Constructible only by verification. Does not certify causal delivery or
/// application write permissions. Consumed by `CommitGraph::ingest_verified`.
#[derive(Debug)]
pub struct VerifiedItem<P> {
    _payload: PhantomData<P>,
}

/// Verify an item without any graph or externally cached author proofs.
/// Missing signatures are errors, including proofs required by contained
/// canonical subfragments. External boundary parents need not be available.
pub fn verify_item<P, C: SyncCodec<P>>(
    _item: &EncodedItem,
    _codec: &mut C,
) -> Result<VerifiedItem<P>, SyncError<C::Error>> {
    todo!("standalone sedimentree item verification")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Newly delivered commits, including older queued commits released by this
    /// item, in parent-before-child order. Duplicates are never redelivered.
    pub delivered: Vec<CommitHash>,
    /// Newly accepted commits from this item still awaiting external parents.
    pub pending: Vec<CommitHash>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemId {
    Commit(CommitHash),
    Fragment(FragmentRange),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemRef {
    pub id: ItemId,
    pub blob: BlobMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

/// Conditional redundancy advice, never authority to delete unverified data or
/// the permanent proofs needed to recreate canonical fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixup {
    pub side: Side,
    pub redundant: ItemRef,
    pub covered_by: Vec<ItemRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// Items the remote has and we want.
    pub wants: Vec<ItemRef>,
    /// Items we have and the remote needs.
    pub needs: Vec<ItemRef>,
    pub fixups: Vec<Fixup>,
}

/// Placeholder; error details will follow with the implementation.
#[derive(Debug)]
pub struct SyncError<E> {
    _codec: PhantomData<E>,
}

impl<E> fmt::Display for SyncError<E> {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("sync error details")
    }
}

impl<E: fmt::Debug> Error for SyncError<E> {}

/// Placeholder for malformed or inconsistent summary errors.
#[derive(Debug)]
pub struct SummaryError {
    _private: (),
}

impl fmt::Display for SummaryError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("summary error details")
    }
}

impl Error for SummaryError {}
