//! Stockfoil storage over Subduction blobs.
//!
//! Subduction stores each Stockfoil item as one blob, described by sedimentree
//! metadata: a [`LooseCommit`] for a standalone commit, a [`Fragment`] for a
//! fragment. This crate has the two pieces an adapter needs:
//!
//! - [`SubductionStorage`], a Stockfoil [`Storage`] keyed by Subduction blob
//!   digests. Subduction hands out whole in-memory blobs today; this storage
//!   holds them and serves the streaming reads and writes Stockfoil asks for.
//!   When Subduction streams blob contents, only this type changes.
//! - [`Metadata`], the sedimentree description of an item. Publishing uses it
//!   to build what Subduction stores. Receiving uses it to check that what
//!   Subduction claimed about a blob is what Stockfoil admitted from it:
//!   Subduction cannot check its metadata against contents, which may be
//!   encrypted, and it decides what to sync and prune from that metadata.
//!
//! Contents may be end-to-end encrypted. Keys are always the digests of the
//! blobs Subduction stores; the bytes held under them are the items, decrypted
//! by the caller. Encryption itself is out of scope here.
//!
//! Nothing read back is trusted. Stockfoil verifies items when it admits them
//! and checks every payload it reads against its commit, so a corrupt,
//! missing, or substituted blob fails admission or reads with an error.
use sedimentree_core::blob::{Blob, BlobMeta};
use sedimentree_core::crypto::digest::Digest;
use sedimentree_core::fragment::{Fragment, InvalidFragment};
use sedimentree_core::id::SedimentreeId;
use sedimentree_core::loose_commit::id::CommitId;
use sedimentree_core::loose_commit::{InvalidLooseCommit, LooseCommit};
use std::collections::HashMap;
use std::fmt;
use std::io;
use stockfoil::{
    Codec, CommitHash, ExportPlan, Graph, History, ItemId, PayloadId, PayloadLocation, Storage,
};

/// A Subduction blob digest: the key of everything [`SubductionStorage`] holds.
pub type BlobDigest = Digest<Blob>;

/// Item and payload bytes for one document, keyed by Subduction blob digest.
///
/// Received items are added with [`Self::insert`] (or [`Self::insert_blob`])
/// and then admitted by key with [`Graph::admit`]. Local payloads written
/// through [`Storage::create`] are kept under the digest of their own bytes;
/// they are never stored in Subduction.
#[derive(Debug, Default)]
pub struct SubductionStorage {
    blobs: HashMap<BlobDigest, Blob>,
    payloads: Vec<PayloadLocation<BlobDigest>>,
}

impl SubductionStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold `item` under `digest`, the digest of the Subduction blob it came
    /// from. Without encryption that is `item`'s own digest
    /// ([`Self::insert_blob`]); with it, the ciphertext's. Replaces anything
    /// already held under `digest`.
    pub fn insert(&mut self, digest: BlobDigest, item: Blob) {
        self.blobs.insert(digest, item);
    }

    /// Hold an unencrypted blob under its own digest, and return the digest.
    pub fn insert_blob(&mut self, blob: Blob) -> BlobDigest {
        let digest = BlobMeta::new(&blob).digest();
        self.insert(digest, blob);
        digest
    }

    pub fn get(&self, digest: &BlobDigest) -> Option<&Blob> {
        self.blobs.get(digest)
    }

    pub fn contains(&self, digest: &BlobDigest) -> bool {
        self.blobs.contains_key(digest)
    }
}

impl Storage for SubductionStorage {
    type Key = BlobDigest;
    type Writer = Vec<u8>;

    fn create(&mut self) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn finish(&mut self, writer: Vec<u8>) -> io::Result<BlobDigest> {
        Ok(self.insert_blob(Blob::new(writer)))
    }

    fn read_at(&mut self, blob: &BlobDigest, offset: u64, out: &mut [u8]) -> io::Result<usize> {
        let bytes = self
            .blobs
            .get(blob)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("blob {blob} not held"))
            })?
            .as_slice();
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let n = out.len().min(bytes.len() - start);
        out[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }

    fn bind(&mut self, location: PayloadLocation<BlobDigest>) -> io::Result<PayloadId> {
        self.payloads.push(location);
        Ok(self.payloads.len() as PayloadId - 1)
    }

    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation<BlobDigest>> {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.payloads.get(i))
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown payload ID"))
    }
}

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
    /// admission's [`Receipt`](stockfoil::Receipt). A mismatch means a writer
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
pub fn export_item<C: Codec<SubductionStorage>>(
    graph: &mut Graph<C, SubductionStorage>,
    plan: &ExportPlan,
    index: usize,
    tree: SedimentreeId,
    seal: impl FnOnce(Vec<u8>) -> Blob,
) -> Result<(Metadata, Blob), Error> {
    let item = graph.history().export_item(plan, index)?;
    let mut bytes = Vec::new();
    graph.write_item(plan, index, &mut bytes)?;
    let blob = seal(bytes);
    let members: Vec<_> = item.members.iter().map(|(hash, _)| *hash).collect();
    let metadata = Metadata::describe(tree, graph.history(), &item.meta.id, &members, blob.meta())?;
    Ok((metadata, blob))
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
