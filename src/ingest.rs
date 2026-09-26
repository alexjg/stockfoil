//! Stateless verification of received history. Dependencies are assumed
//! authenticated; admission establishes that premise and checks canonical fragment
//! placement against their history before mutation. Signatures cover each
//! author's causal frontier among all supplied members.
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
        /// External parents other than boundaries, excluded with boundary history.
        additional_dependencies: BTreeSet<CommitHash>,
        /// Normalized members: the head and its ancestry minus boundaries and
        /// all boundary ancestry. Admission checks this canonical placement.
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
            Self::Fragment {
                range,
                additional_dependencies,
                ..
            } => range
                .boundary
                .union(additional_dependencies)
                .copied()
                .collect(),
        };
        hashes.into_iter()
    }

    pub fn verify(self) -> Result<VerifiedItem, VerificationError> {
        match self {
            Self::Commit { meta, signatures } => {
                let dependencies = meta.parents().iter().copied().collect::<BTreeSet<_>>();
                verify_metadata(
                    ItemId::Commit(*meta.hash()),
                    dependencies,
                    BTreeSet::new(),
                    signatures,
                    std::iter::once(Ok(meta)),
                )
            }
            Self::Fragment {
                range,
                additional_dependencies,
                commits,
                signatures,
            } => Self::verify_fragment(
                range,
                additional_dependencies,
                signatures,
                commits.into_iter().map(Ok),
            ),
        }
    }

    pub fn verify_fragment(
        range: FragmentRange,
        additional_dependencies: BTreeSet<CommitHash>,
        signatures: Vec<SignatureEvidence>,
        commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
    ) -> Result<VerifiedItem, VerificationError> {
        if !range.boundary.is_disjoint(&additional_dependencies) {
            return Err(VerificationError::InvalidItem);
        }
        let dependencies = range.boundary.clone();
        verify_metadata(
            ItemId::Fragment(range),
            dependencies,
            additional_dependencies,
            signatures,
            commits,
        )
    }
}

fn verify_metadata(
    id: ItemId,
    dependencies: BTreeSet<CommitHash>,
    additional_dependencies: BTreeSet<CommitHash>,
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
    let direct_boundary = match &id {
        ItemId::Fragment(range) => validate_fragment(range, &additional_dependencies, &members)?,
        ItemId::Commit(_) => BTreeSet::new(),
    };

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
        dependencies,
        additional_dependencies,
        direct_boundary,
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

/// Check item-local closure and retain boundary references from supplied members.
/// Canonical placement in excluded history still requires the destination graph.
fn validate_fragment(
    range: &FragmentRange,
    additional_dependencies: &BTreeSet<CommitHash>,
    members: &BTreeMap<CommitHash, CommitMeta>,
) -> Result<BTreeSet<CommitHash>, VerificationError> {
    let head_depth = depth(&range.head);
    if head_depth == 0
        || !members.contains_key(&range.head)
        || range.boundary.iter().any(|hash| depth(hash) < head_depth)
        || additional_dependencies
            .iter()
            .any(|hash| depth(hash) >= head_depth)
        || members.keys().any(|hash| {
            range.boundary.contains(hash)
                || additional_dependencies.contains(hash)
                || (*hash != range.head && depth(hash) >= head_depth)
        })
    {
        return Err(VerificationError::InvalidItem);
    }
    let mut direct_boundary = BTreeSet::new();
    let mut external = BTreeSet::new();
    for parent in members.values().flat_map(|meta| meta.parents()) {
        if range.boundary.contains(parent) {
            direct_boundary.insert(*parent);
        } else if !members.contains_key(parent) {
            external.insert(*parent);
        }
    }
    // Boundaries can also be discovered through excluded history, so they need
    // not be referenced directly. Additional dependencies must be exact.
    if &external != additional_dependencies
        || ancestry(members, [range.head]).len() != members.len()
    {
        return Err(VerificationError::InvalidItem);
    }
    Ok(direct_boundary)
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
    // Standalone parents or fragment boundaries, separate from external parents
    // excluded through boundary history.
    dependencies: BTreeSet<CommitHash>,
    additional_dependencies: BTreeSet<CommitHash>,
    // Boundary hashes referenced by member parents, computed during verification.
    // Empty for standalone commits. Admission only needs to discover the stops
    // reached through additional dependencies, without rewalking member edges.
    pub(crate) direct_boundary: BTreeSet<CommitHash>,
    pub(crate) signatures: Vec<SignatureEvidence>,
    pub(crate) signature_boundary: BTreeMap<Author, BTreeSet<CommitHash>>,
}

impl VerifiedItem {
    pub fn id(&self) -> &ItemId {
        &self.id
    }
    /// Sorted, distinct prerequisites whose authentication admission must establish:
    /// standalone parents, or the union of fragment boundaries and additional dependencies.
    pub fn dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.dependencies.union(&self.additional_dependencies)
    }
    /// Exact external member parents other than fragment boundaries, sorted and distinct.
    /// These must lie in boundary history; admission checks that placement.
    /// Empty for standalone commits.
    pub fn additional_dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.additional_dependencies.iter()
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
