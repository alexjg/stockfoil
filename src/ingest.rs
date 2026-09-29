//! Stateless verification of received history. Dependencies are assumed
//! authenticated; admission establishes that premise and checks canonical fragment
//! placement against their history before mutation. Signatures cover each
//! author's causal frontier among all supplied members.
//! Payload bytes and application-specific decoding remain separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Author, Checkpoint, CommitHash, CommitMeta, FragmentRange, Signature, SignatureError,
    VerificationError,
};

/// Transport evidence: a signature with an explicit signer and target.
/// Constructing this value establishes neither validity nor author coverage.
/// Internally the graph stores signatures on the commit they sign.
#[derive(Debug, Clone)]
pub struct SignatureEvidence {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

/// A standalone commit at any depth or a canonical fragment, not a dependency witness.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemId {
    Commit(CommitHash),
    Fragment(FragmentRange),
}

/// Outcomes of ingestion and any queued work it unblocked.
///
/// Returned by received ingestion, local insertion, signing, and merge. A valid
/// dependency remains admitted even if it unlocks an invalid queued fragment;
/// inspect `rejected` on every operation which can admit history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Certificates admitted by this call, including local certificates and
    /// duplicates. This is not a stream of newly delivered commits: use
    /// [`crate::CommitGraph::visit_changes`] for materialization.
    pub admitted: Vec<ItemId>,
    /// Previously queued items which failed graph-dependent admission checks.
    /// These items have been removed from the queue, without delivering any of
    /// their members. Their dependents remain pending unless another item can
    /// provide the missing commits.
    pub rejected: Vec<IngestRejection>,
    /// Number of received items still waiting after this call (not local drafts).
    pub pending: usize,
}

/// A queued item rejected once its dependencies became admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestRejection {
    pub item: ItemId,
    pub error: crate::IngestError,
}

/// Owned, decoded transport contents, convenient for in-memory exchange.
/// This is not a wire format. Streaming/application-specific decoders can use
/// [`Self::verify_fragment`] without first constructing a vector of commits.
#[derive(Debug, Clone)]
pub enum UnverifiedItem {
    Commit {
        meta: CommitMeta,
        signatures: Vec<SignatureEvidence>,
    },
    Fragment {
        range: FragmentRange,
        /// Normalized members: the head and its ancestry minus boundaries and
        /// all boundary ancestry. Admission checks this canonical placement.
        commits: Vec<CommitMeta>,
        signatures: Vec<SignatureEvidence>,
        /// Non-authoritative reconciliation hints, ignored by verification.
        /// Admission and re-export derive canonical metadata locally: sorted,
        /// deduplicated 12-byte prefixes of nonzero-depth interior commits,
        /// excluding the head, boundaries, and all boundary ancestry.
        /// These never replace full authenticated member IDs.
        checkpoints: BTreeSet<Checkpoint>,
    },
}

impl UnverifiedItem {
    /// The claimed identity, not yet checked against the supplied metadata.
    pub fn id(&self) -> ItemId {
        match self {
            Self::Commit { meta, .. } => ItemId::Commit(*meta.hash()),
            Self::Fragment { range, .. } => ItemId::Fragment(range.clone()),
        }
    }

    /// All the causal dependencies of this item
    pub fn dependencies(&self) -> impl Iterator<Item = CommitHash> + '_ {
        let hashes: BTreeSet<_> = match self {
            Self::Commit { meta, .. } => meta.parents().iter().copied().collect(),
            Self::Fragment { range, commits, .. } => {
                let mut parents = external_parents(commits.iter());
                parents.extend(&range.boundary);
                parents
            }
        };
        hashes.into_iter()
    }

    /// Authenticate full member metadata and IDs, ignoring checkpoint hints.
    pub fn verify(self) -> Result<VerifiedItem, VerificationError> {
        match self {
            Self::Commit { meta, signatures } => verify_metadata(
                ItemId::Commit(*meta.hash()),
                signatures,
                std::iter::once(Ok(meta)),
            ),
            Self::Fragment {
                range,
                commits,
                signatures,
                ..
            } => Self::verify_fragment(range, signatures, commits.into_iter().map(Ok)),
        }
    }

    /// Verify decoded member metadata in any order, deriving external parents
    /// only after all member hashes are known. Decoder errors abort verification.
    /// Admission must still check canonical placement against boundary history.
    pub fn verify_fragment(
        range: FragmentRange,
        signatures: Vec<SignatureEvidence>,
        commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
    ) -> Result<VerifiedItem, VerificationError> {
        verify_metadata(ItemId::Fragment(range), signatures, commits)
    }
}

/// Derive references leaving the members, independent of input order.
fn external_parents<'a>(commits: impl Iterator<Item = &'a CommitMeta>) -> BTreeSet<CommitHash> {
    let mut members = BTreeSet::new();
    let mut parents = BTreeSet::new();
    for meta in commits {
        members.insert(*meta.hash());
        parents.extend(meta.parents());
    }
    parents.retain(|hash| !members.contains(hash));
    parents
}

fn verify_metadata(
    id: ItemId,
    signatures: Vec<SignatureEvidence>,
    commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
) -> Result<VerifiedItem, VerificationError> {
    let mut members = BTreeMap::new();
    for meta in commits {
        let meta = meta?;
        if members.insert(*meta.hash(), meta).is_some() {
            return Err(VerificationError::InvalidItem);
        }
    }
    let sorted = topological_order(&members)?;
    let external_parents = external_parents(members.values());
    if let ItemId::Fragment(range) = &id {
        validate_fragment(range, &external_parents, &members)?;
    }

    for evidence in &signatures {
        if !members.contains_key(&evidence.commit) {
            return Err(VerificationError::InvalidItem);
        }
        let key =
            ed25519_dalek::VerifyingKey::from_bytes(evidence.author.as_array()).map_err(|_| {
                VerificationError::InvalidSignature(SignatureError::InvalidPublicKey(
                    evidence.author,
                ))
            })?;
        let signature = ed25519_dalek::Signature::from_bytes(&evidence.signature.into());
        key.verify_strict(evidence.commit.as_bytes(), &signature)
            .map_err(|_| {
                VerificationError::InvalidSignature(SignatureError::InvalidSignature {
                    author: evidence.author,
                    commit: evidence.commit,
                })
            })?;
    }

    let mut introduced: BTreeMap<Author, BTreeSet<CommitHash>> = BTreeMap::new();
    for meta in members.values() {
        introduced
            .entry(*meta.author())
            .or_default()
            .insert(*meta.hash());
    }
    let mut signature_boundary = BTreeMap::new();
    for (author, authored) in introduced {
        // All concurrent latest commits matter. Checking these maximal members
        // also establishes coverage of this author's earlier introduced members.
        let required = frontier(&members, authored);
        let signed = frontier(
            &members,
            signatures
                .iter()
                .filter(|evidence| evidence.author == author)
                .map(|evidence| evidence.commit)
                .collect(),
        );
        let covered = ancestry(&members, signed.iter().copied());
        if let Some(commit) = required.difference(&covered).next() {
            return Err(VerificationError::MissingSignature {
                author,
                commit: *commit,
            });
        }
        signature_boundary.insert(author, signed);
    }
    Ok(VerifiedItem {
        id,
        commits: sorted
            .into_iter()
            .map(|hash| members.remove(&hash).unwrap())
            .collect(),
        external_parents,
        signatures,
        signature_boundary,
    })
}

/// Follow only supplied metadata, including cross-author links.
fn ancestry(
    members: &BTreeMap<CommitHash, CommitMeta>,
    tips: impl IntoIterator<Item = CommitHash>,
) -> BTreeSet<CommitHash> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<_> = tips.into_iter().collect();
    while let Some(hash) = stack.pop() {
        if let Some(meta) = members.get(&hash) {
            if found.insert(hash) {
                stack.extend(meta.parents());
            }
        }
    }
    found
}

fn frontier(
    members: &BTreeMap<CommitHash, CommitMeta>,
    mut tips: BTreeSet<CommitHash>,
) -> BTreeSet<CommitHash> {
    let ancestors = ancestry(
        members,
        tips.iter()
            .flat_map(|hash| members[hash].parents().iter().copied()),
    );
    tips.retain(|hash| !ancestors.contains(hash));
    tips
}

fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes()
        .iter()
        .take_while(|byte| **byte == 0)
        .count()
}

/// Check item-local reachability and external-parent depth constraints.
/// Canonical placement in excluded history still requires the destination graph.
fn validate_fragment(
    range: &FragmentRange,
    external_parents: &BTreeSet<CommitHash>,
    members: &BTreeMap<CommitHash, CommitMeta>,
) -> Result<(), VerificationError> {
    let head_depth = depth(&range.head);
    if head_depth == 0
        || !members.contains_key(&range.head)
        || range.boundary.iter().any(|hash| depth(hash) < head_depth)
        || external_parents
            .difference(&range.boundary)
            .any(|hash| depth(hash) >= head_depth)
        || members.keys().any(|hash| {
            range.boundary.contains(hash) || (*hash != range.head && depth(hash) >= head_depth)
        })
    {
        return Err(VerificationError::InvalidItem);
    }
    // Low-depth external parents may belong to omitted boundary history.
    // Admission checks this placement; no external metadata is consulted here.
    if ancestry(members, [range.head]).len() != members.len() {
        return Err(VerificationError::InvalidItem);
    }
    Ok(())
}

/// Kahn's algorithm over available metadata; external parents are not read.
fn topological_order(
    members: &BTreeMap<CommitHash, CommitMeta>,
) -> Result<Vec<CommitHash>, VerificationError> {
    let mut remaining = BTreeMap::new();
    let mut children: BTreeMap<CommitHash, Vec<CommitHash>> = BTreeMap::new();
    let mut ready = BTreeSet::new();
    for (hash, meta) in members {
        let parents: BTreeSet<_> = meta
            .parents()
            .iter()
            .filter(|parent| members.contains_key(parent))
            .copied()
            .collect();
        remaining.insert(*hash, parents.len());
        if parents.is_empty() {
            ready.insert(*hash);
        }
        for parent in parents {
            children.entry(parent).or_default().push(*hash);
        }
    }
    let mut sorted = Vec::with_capacity(members.len());
    while let Some(hash) = ready.pop_first() {
        sorted.push(hash);
        if let Some(descendants) = children.get(&hash) {
            for child in descendants {
                let count = remaining
                    .get_mut(child)
                    .expect("child belongs to supplied metadata");
                *count -= 1;
                if *count == 0 {
                    ready.insert(*child);
                }
            }
        }
    }
    if sorted.len() != members.len() {
        return Err(VerificationError::InvalidItem);
    }
    Ok(sorted)
}

/// A conditional authentication and closure certificate, not proof that
/// dependencies or payloads are available. Canonical fragment placement cannot
/// be proven statelessly; admission checks it against authenticated dependency
/// history before mutation. There is deliberately no unchecked public constructor.
///
/// ```compile_fail
/// use stockfoil::VerifiedItem;
/// let item = VerifiedItem {};
/// ```
#[derive(Debug, Clone)]
pub struct VerifiedItem {
    pub(crate) id: ItemId,
    pub(crate) commits: Vec<CommitMeta>,
    // Exact parent references leaving the supplied members. Admission follows
    // these into boundary history without rewalking supplied member edges.
    external_parents: BTreeSet<CommitHash>,
    pub(crate) signatures: Vec<SignatureEvidence>,
    pub(crate) signature_boundary: BTreeMap<Author, BTreeSet<CommitHash>>,
}

impl VerifiedItem {
    pub fn id(&self) -> &ItemId {
        &self.id
    }
    /// Sorted, distinct admission prerequisites: the union of the range boundary
    /// and external parents for a fragment, or parents for a standalone commit.
    pub fn dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        let boundary = match &self.id {
            ItemId::Fragment(range) => &range.boundary,
            ItemId::Commit(_) => &self.external_parents,
        };
        self.external_parents.union(boundary)
    }
    /// Exact parent references leaving the supplied members, sorted and distinct.
    /// For fragments, admission must prove these lie in boundary history.
    /// For standalone commits, these are the commit's parents.
    pub fn external_parents(&self) -> impl Iterator<Item = &CommitHash> {
        self.external_parents.iter()
    }
    /// Member envelopes in lexicographically smallest parents-first order.
    pub fn commits(&self) -> impl Iterator<Item = &CommitMeta> {
        self.commits.iter()
    }
    /// Signed member targets whose per-author frontiers dominate the author's
    /// latest supplied commits. Boundaries are dependencies, never members.
    pub fn signature_boundary(&self) -> &BTreeMap<Author, BTreeSet<CommitHash>> {
        &self.signature_boundary
    }
}
