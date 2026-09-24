//! Stateless verification of received history. Fragment boundaries are assumed
//! authenticated; admission establishes that premise. Signatures cover each
//! author's causal frontier among the members introduced by the item.
//! Payload bytes and application-specific decoding remain separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Author, CommitHash, CommitMeta, FragmentRange, Signature, SignatureError, VerificationError,
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
        commits: Vec<CommitMeta>,
        signatures: Vec<SignatureEvidence>,
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

    /// Untrusted prerequisite claims. Verification never looks these up.
    pub fn dependencies(&self) -> impl Iterator<Item = CommitHash> + '_ {
        let hashes: BTreeSet<_> = match self {
            Self::Commit { meta, .. } => meta.parents().iter().copied().collect(),
            Self::Fragment { range, .. } => range.boundary.clone(),
        };
        hashes.into_iter()
    }

    /// Verify already-decoded contents. A standalone commit is valid at any depth;
    /// its own certificate does not certify a fragment headed at that commit.
    /// The result assumes authenticated dependencies; admission checks that premise.
    pub fn verify(self) -> Result<VerifiedItem, VerificationError> {
        match self {
            Self::Commit { meta, signatures } => {
                let dependencies = meta.parents().iter().copied().collect::<BTreeSet<_>>();
                verify_metadata(
                    ItemId::Commit(*meta.hash()),
                    dependencies,
                    signatures,
                    std::iter::once(Ok(meta)),
                )
            }
            Self::Fragment {
                range,
                commits,
                signatures,
            } => Self::verify_fragment(range, signatures, commits.into_iter().map(Ok)),
        }
    }

    /// Drive an application-provided decoder one envelope at a time. A decoder
    /// can reconstruct omitted hashes and compressed parent references using
    /// `CommitMeta::new` as it proceeds. Map malformed compressed data to
    /// `VerificationError::InvalidItem`; a decoding error aborts verification.
    ///
    /// No graph or dependency provider is consulted. Metadata is retained for
    /// structural checks and admission, but payload bytes need not be retained.
    pub fn verify_fragment(
        range: FragmentRange,
        signatures: Vec<SignatureEvidence>,
        commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
    ) -> Result<VerifiedItem, VerificationError> {
        let dependencies = range.boundary.clone();
        verify_metadata(ItemId::Fragment(range), dependencies, signatures, commits)
    }
}

fn verify_metadata(
    id: ItemId,
    dependencies: BTreeSet<CommitHash>,
    signatures: Vec<SignatureEvidence>,
    commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
) -> Result<VerifiedItem, VerificationError> {
    let mut members = BTreeMap::new();
    for meta in commits {
        let meta = meta?;
        // CommitMeta's private fields and canonical constructor bind its hash to
        // its contents. A decoder cannot supply an unchecked cached hash.
        if members.insert(*meta.hash(), meta).is_some() {
            return Err(VerificationError::InvalidItem);
        }
    }
    let sorted = topological_order(&members)?;
    let boundary = if let ItemId::Fragment(range) = &id {
        validate_fragment(range, &members)?;
        range.boundary.clone()
    } else {
        BTreeSet::new()
    };

    // Check every signature, even redundant evidence. Targets must be members;
    // neither dependency claims nor decoder state extend signature coverage.
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
        if !boundary.contains(meta.hash()) {
            introduced
                .entry(*meta.author())
                .or_default()
                .insert(*meta.hash());
        }
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
        dependencies: dependencies.into_iter().collect(),
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

/// Plain canonical traversal, independent of the graph's fragment index.
fn validate_fragment(
    range: &FragmentRange,
    members: &BTreeMap<CommitHash, CommitMeta>,
) -> Result<(), VerificationError> {
    let head_depth = depth(&range.head);
    if head_depth == 0 {
        return Err(VerificationError::InvalidItem);
    }
    let mut pending = vec![range.head];
    let mut visited = BTreeSet::new();
    let mut boundary = BTreeSet::new();
    while let Some(hash) = pending.pop() {
        if !visited.insert(hash) {
            continue;
        }
        let meta = members.get(&hash).ok_or(VerificationError::InvalidItem)?;
        if hash != range.head && depth(&hash) >= head_depth {
            boundary.insert(hash);
        } else {
            pending.extend(meta.parents().iter().copied());
        }
    }
    if boundary != range.boundary || visited.len() != members.len() {
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

/// A conditional certificate, not proof that dependencies or payloads are available.
/// There is deliberately no unchecked public constructor.
///
/// ```compile_fail
/// use stockfoil::VerifiedItem;
/// let item = VerifiedItem {};
/// ```
#[derive(Debug, Clone)]
pub struct VerifiedItem {
    pub(crate) id: ItemId,
    pub(crate) commits: Vec<CommitMeta>,
    pub(crate) dependencies: Vec<CommitHash>,
    pub(crate) signatures: Vec<SignatureEvidence>,
    pub(crate) signature_boundary: BTreeMap<Author, BTreeSet<CommitHash>>,
}

impl VerifiedItem {
    pub fn id(&self) -> &ItemId {
        &self.id
    }
    /// Distinct prerequisites whose authentication admission must establish.
    pub fn dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.dependencies.iter()
    }
    /// Member envelopes in lexicographically smallest parents-first order.
    pub fn commits(&self) -> impl Iterator<Item = &CommitMeta> {
        self.commits.iter()
    }
    /// Signed targets whose per-author frontiers dominate the author's latest
    /// introduced commits. Already-authenticated boundaries add no obligation.
    pub fn signature_boundary(&self) -> &BTreeMap<Author, BTreeSet<CommitHash>> {
        &self.signature_boundary
    }
}
