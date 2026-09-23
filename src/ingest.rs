//! The trust boundary for received sedimentree history.
//!
//! Verification is stateless and uses only supplied metadata. Signature targets
//! and coverage paths are confined to actual item members; fragment boundaries
//! are assumed to be authenticated, a premise established during admission.
//! Metadata authentication binds payload hashes, not the actual payload bytes.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Author, CommitContents, CommitHash, CommitMeta, FragmentRange, Signature, SignatureError,
    VerificationError,
};

/// Signature evidence supplied with an untrusted item. Merely constructing this
/// value does not establish signature validity or author coverage.
#[derive(Debug, Clone)]
pub struct SignatureEvidence {
    pub author: Author,
    pub commit: CommitHash,
    pub signature: Signature,
}

/// An exchangeable item's identity, not a verification prerequisite.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemId {
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
    /// The claimed identity, not yet checked against the supplied metadata.
    pub fn id(&self) -> ItemId {
        match self {
            Self::Commit { meta, .. } => ItemId::Commit(*meta.hash()),
            Self::Fragment { range, .. } => ItemId::Fragment(range.clone()),
        }
    }

    /// Enumerate distinct, untrusted prerequisite claims deterministically.
    /// Fragment boundaries are inclusive members assumed authenticated.
    pub fn dependencies(&self) -> impl Iterator<Item = CommitHash> + '_ {
        let hashes: BTreeSet<_> = match self {
            Self::Commit { meta, .. } => meta.parents().iter().copied().collect(),
            Self::Fragment { range, .. } => range.boundary.clone(),
        };
        hashes.into_iter()
    }

    /// Verify identity, canonical structure, and author-signature coverage using
    /// only supplied metadata. The result is conditional on authentication of
    /// the declared dependencies; admission must establish that premise.
    pub fn verify(self) -> Result<VerifiedItem, VerificationError> {
        let dependencies: Vec<_> = self.dependencies().collect();
        let (id, commits, signatures) = match self {
            Self::Commit { meta, signatures } => {
                (ItemId::Commit(*meta.hash()), vec![meta], signatures)
            }
            Self::Fragment {
                range,
                commits,
                signatures,
            } => (ItemId::Fragment(range), commits, signatures),
        };

        // Hash-check every envelope and reject duplicate records rather than
        // silently overwriting them.
        let mut actual = BTreeMap::new();
        for meta in &commits {
            let reconstructed = CommitMeta::new(CommitContents {
                parents: meta.parents().to_vec(),
                author: *meta.author(),
                payload: *meta.payload_hash(),
            });
            if reconstructed.hash() != meta.hash() || actual.insert(*meta.hash(), meta).is_some() {
                return Err(VerificationError::InvalidItem);
            }
        }
        let sorted = topological_order(&actual)?;

        let mut covered = BTreeSet::new();
        match &id {
            ItemId::Commit(hash) => {
                if depth(hash) != 0 {
                    return Err(VerificationError::InvalidItem);
                }
            }
            ItemId::Fragment(range) => {
                validate_fragment(range, &actual)?;
                // Boundaries are authenticated by inductive assumption, not by
                // lookup. Admission must establish this certificate's premise.
                covered.extend(range.boundary.iter().copied());
            }
        }

        // Every supplied signature must be valid, even if some other signature
        // already provides complete coverage. A dependency alone is not an
        // allowed target: it must be an actual item member.
        for evidence in &signatures {
            if !actual.contains_key(&evidence.commit) {
                return Err(VerificationError::InvalidItem);
            }
            let key = ed25519_dalek::VerifyingKey::from_bytes(evidence.author.as_array()).map_err(
                |_| {
                    VerificationError::InvalidSignature(SignatureError::InvalidPublicKey(
                        evidence.author,
                    ))
                },
            )?;
            let signature = ed25519_dalek::Signature::from_bytes(&evidence.signature.into());
            key.verify_strict(evidence.commit.as_bytes(), &signature)
                .map_err(|_| {
                    VerificationError::InvalidSignature(SignatureError::InvalidSignature {
                        author: evidence.author,
                        commit: evidence.commit,
                    })
                })?;
        }
        covered.extend(signature_coverage(&actual, &signatures));

        for hash in &sorted {
            let meta = actual[hash];
            if !covered.contains(hash) {
                return Err(VerificationError::MissingSignature {
                    author: *meta.author(),
                    commit: *hash,
                });
            }
        }
        let commits = sorted
            .into_iter()
            .map(|hash| actual[&hash].clone())
            .collect();
        Ok(VerifiedItem {
            id,
            commits,
            dependencies,
            signatures,
        })
    }
}

/// Author coverage within one item, after validating its signatures.
fn signature_coverage(
    members: &BTreeMap<CommitHash, &CommitMeta>,
    signatures: &[SignatureEvidence],
) -> BTreeSet<CommitHash> {
    let mut covered = BTreeSet::new();
    for evidence in signatures {
        let mut pending = vec![evidence.commit];
        let mut visited = BTreeSet::new();
        while let Some(hash) = pending.pop() {
            if !visited.insert(hash) {
                continue;
            }
            let Some(meta) = members.get(&hash) else {
                continue;
            };
            if meta.author() == &evidence.author {
                covered.insert(hash);
            }
            pending.extend(meta.parents().iter().copied());
        }
    }
    covered
}

/// Compare the complete envelope, preserving parent order (and multiplicity).
pub(crate) fn same_meta(left: &CommitMeta, right: &CommitMeta) -> bool {
    left.hash() == right.hash()
        && left.parents() == right.parents()
        && left.author() == right.author()
        && left.payload_hash() == right.payload_hash()
}

fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes()
        .iter()
        .take_while(|byte| **byte == 0)
        .count()
}

/// Plain maximum-depth traversal, independent of any graph fragment index.
fn validate_fragment(
    range: &FragmentRange,
    members: &BTreeMap<CommitHash, &CommitMeta>,
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

/// Kahn's algorithm over only available metadata; external parents are not read.
fn topological_order(
    members: &BTreeMap<CommitHash, &CommitMeta>,
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

/// A checked, conditional certificate relative to explicit dependencies.
/// Does not establish that dependencies are authenticated or admitted, nor that
/// payload bytes are available or valid.
/// There is deliberately no public unchecked constructor or conversion.
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
}
