//! The trust boundary for received sedimentree history.
//!
//! These APIs are scaffolding: dependency discovery, verification, and admission
//! are not implemented yet. The decoded item types do not specify a wire format.
//! Verification authenticates metadata and its payload hashes, not payload bytes.

use std::marker::PhantomData;

use crate::{Author, CommitHash, CommitMeta, FragmentRange, Signature, VerificationError};

/// Signature evidence supplied with an untrusted item. Merely constructing this
/// value does not establish signature validity or author coverage.
#[derive(Debug, Clone)]
pub struct SignatureEvidence {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

/// An explicitly named prerequisite for verifying or admitting an item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyId {
    Commit(CommitHash),
    Fragment(FragmentRange),
}

/// Decoded, untrusted metadata and evidence. Payload bytes are supplied and
/// checked separately. A future decoder may reconstruct omitted internal hashes
/// before producing these envelopes; no serialization format is fixed here.
#[derive(Debug, Clone)]
pub enum UnverifiedItem {
    Commit {
        meta: CommitMeta,
        signatures: Vec<SignatureEvidence>,
    },
    Fragment {
        range: FragmentRange,
        commits: Vec<CommitMeta>,
        signatures: Vec<SignatureEvidence>,
    },
}

impl UnverifiedItem {
    /// Enumerate the external prerequisites needed to verify this item. Missing
    /// prerequisites must be fetched or queued outside the verified graph.
    pub fn dependencies(&self) -> std::vec::IntoIter<DependencyId> {
        todo!("discover external sedimentree verification dependencies")
    }

    /// Verify identity, structure, and sufficient author-signature coverage using
    /// only this item and its explicitly declared, verified dependencies.
    ///
    /// Success authenticates the declared payload hashes, not the actual payload
    /// bytes. Missing dependencies are distinct from invalid metadata or proofs.
    pub fn verify(
        self,
        _dependencies: &(impl VerifiedDependencies + ?Sized),
    ) -> Result<VerifiedItem, VerificationError> {
        todo!("verify sedimentree metadata and retained signature evidence")
    }
}

/// Immutable evidence that an item's metadata and signature coverage have been
/// verified relative to its explicit dependencies. Not a proof that payload
/// bytes are available or match their declared hashes.
///
/// Only successful verification may construct this value. There is deliberately
/// no unchecked constructor, `Default`, or conversion from raw commit metadata.
///
/// ```compile_fail
/// use stockfoil::VerifiedItem;
/// let item = VerifiedItem {};
/// ```
#[derive(Debug)]
pub struct VerifiedItem {
    _private: (),
}

impl VerifiedItem {
    /// Borrow verified dependency information established by this item, allowing
    /// verification from a small collection of items rather than a whole graph.
    /// Returns `None` when the item does not establish the requested dependency.
    pub fn dependency(&self, _id: &DependencyId) -> Option<VerifiedDependency<'_>> {
        todo!("expose verified dependency evidence from an item")
    }
}

/// A borrowed, unforgeable witness for one verified dependency. It cannot be
/// constructed from an unverified item or unsigned local graph metadata.
///
/// ```compile_fail
/// use stockfoil::VerifiedDependency;
/// let dependency = VerifiedDependency {};
/// ```
#[derive(Debug)]
pub struct VerifiedDependency<'verified> {
    _verified: PhantomData<&'verified ()>,
}

impl VerifiedDependency<'_> {
    pub fn id(&self) -> &DependencyId {
        todo!("identify a verified dependency")
    }

    /// The authenticated commit envelopes established by this dependency.
    /// Access does not require loading their payloads or unrelated graph history.
    pub fn commits(&self) -> std::slice::Iter<'_, CommitMeta> {
        todo!("read verified dependency metadata")
    }
}

/// Resolve already-verified prerequisites. Implementations may use an in-memory
/// graph, storage, or a small set of verified items; no full graph is required.
/// Returning a witness requires evidence produced by successful verification,
/// not a caller-supplied boolean asserting trust.
pub trait VerifiedDependencies {
    fn get(&self, id: &DependencyId) -> Option<VerifiedDependency<'_>>;
}
