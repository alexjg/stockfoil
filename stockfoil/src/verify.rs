//! Incremental bundle verification. Canonical member bytes arrive in order and
//! are hashed as they stream past; nothing is buffered. Only this module creates
//! [`Verified`], and history admits nothing else from received data.
use crate::evidence::{Coverage, reach, verify_signature};
use crate::meta::{CommitHasher, depth};
use crate::{BundleMeta, CommitHash, Error, ItemId, Limits, ParentRef};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Work units charged for one signature check, relative to one hashed byte.
pub(crate) const SIGNATURE_COST: usize = 16 * 1024;

/// An authenticated bundle: identities reconstructed from canonical bytes,
/// every signature checked, and every member covered by its author's signature.
/// Placement in history is checked separately at admission.
#[derive(Debug)]
pub(crate) struct Verified {
    pub meta: Arc<BundleMeta>,
    pub hashes: Vec<CommitHash>,
    /// Parent references leaving the bundle, sorted and distinct.
    pub external: BTreeSet<CommitHash>,
    /// The rows of `meta.evidence` that cover the bundle (see [`coverage`]).
    pub evidence: Vec<usize>,
}

impl Verified {
    /// Commits that must already be admitted: boundaries and external parents.
    pub fn dependencies(&self) -> BTreeSet<CommitHash> {
        let mut deps = self.external.clone();
        if let ItemId::Fragment(range) = &self.meta.id {
            deps.extend(&range.boundary);
        }
        deps
    }

    pub fn parents(&self, member: usize) -> impl ExactSizeIterator<Item = CommitHash> + '_ {
        self.meta.members[member].parents.iter().map(|p| match *p {
            ParentRef::Member(i) => self.hashes[i],
            ParentRef::External(h) => h,
        })
    }
}

enum Phase {
    Members,
    Signatures(usize),
    Coverage(Coverage),
    Done,
}

pub(crate) struct Verifier {
    meta: Arc<BundleMeta>,
    limits: Limits,
    phase: Phase,
    payload: blake3::Hasher,
    canonical_bytes: u64,
    hashes: Vec<CommitHash>,
    by_hash: BTreeMap<CommitHash, usize>,
    result: Option<Arc<Verified>>,
}

fn invalid<T>(why: &'static str) -> Result<T, Error> {
    Err(Error::InvalidItem(why))
}

impl Verifier {
    /// Check everything that does not need member contents.
    pub fn new(meta: Arc<BundleMeta>, limits: &Limits) -> Result<Self, Error> {
        let n = meta.members.len();
        if n > limits.max_members {
            return Err(Error::Limit("members"));
        }
        if meta.evidence.len() > limits.max_signatures {
            return Err(Error::Limit("signatures"));
        }
        match &meta.id {
            ItemId::Commit(_) if n != 1 => return invalid("a commit has exactly one member"),
            ItemId::Fragment(range) => {
                let head_depth = depth(&range.head);
                if n == 0 || head_depth == 0 {
                    return invalid("a fragment head must have nonzero depth");
                }
                if range.boundary.len() > limits.max_boundaries {
                    return Err(Error::Limit("boundaries"));
                }
                if range.boundary.iter().any(|b| depth(b) < head_depth) {
                    return invalid("boundary shallower than head");
                }
            }
            _ => {}
        }
        let mut parents = 0usize;
        for (i, member) in meta.members.iter().enumerate() {
            parents = parents.saturating_add(member.parents.len());
            if parents > limits.max_parents {
                return Err(Error::Limit("parents"));
            }
            if member
                .parents
                .iter()
                .any(|p| matches!(p, ParentRef::Member(j) if *j >= i))
            {
                return invalid("member parent must be an earlier member");
            }
        }
        if meta.evidence.iter().any(|e| e.member >= n) {
            return invalid("signature target out of range");
        }
        Ok(Self {
            meta,
            limits: limits.clone(),
            phase: Phase::Members,
            payload: blake3::Hasher::new(),
            canonical_bytes: 0,
            hashes: Vec::with_capacity(n),
            by_hash: BTreeMap::new(),
            result: None,
        })
    }

    pub fn meta(&self) -> &Arc<BundleMeta> {
        &self.meta
    }

    /// Members whose contents have ended so far.
    pub fn members_ended(&self) -> usize {
        self.hashes.len()
    }

    pub fn result(&self) -> Option<&Arc<Verified>> {
        self.result.as_ref()
    }

    /// Hash canonical bytes of the current member.
    pub fn update(&mut self, canonical: &[u8]) -> Result<(), Error> {
        if !matches!(self.phase, Phase::Members) || self.hashes.len() == self.meta.len() {
            return Err(Error::Protocol("canonical bytes after the last member"));
        }
        self.canonical_bytes = self
            .canonical_bytes
            .checked_add(canonical.len() as u64)
            .filter(|&total| total <= self.limits.max_canonical_bytes)
            .ok_or(Error::Limit("canonical bytes"))?;
        self.payload.update(canonical);
        Ok(())
    }

    /// Close the current member and reconstruct its commit hash.
    pub fn end_member(&mut self) -> Result<(), Error> {
        let index = self.hashes.len();
        if !matches!(self.phase, Phase::Members) || index == self.meta.len() {
            return Err(Error::Protocol("more member boundaries than members"));
        }
        let payload: [u8; 32] = self.payload.finalize().into();
        self.payload.reset();
        let member = &self.meta.members[index];
        let mut hasher = CommitHasher::new(member.parents.len());
        for parent in &member.parents {
            hasher.parent(match parent {
                ParentRef::Member(i) => &self.hashes[*i],
                ParentRef::External(h) => h,
            });
        }
        let hash = hasher.finish(&member.author, &payload);
        if self.by_hash.insert(hash, index).is_some() {
            return invalid("duplicate member");
        }
        if let ItemId::Fragment(range) = &self.meta.id
            && (range.boundary.contains(&hash)
                || (hash != range.head && depth(&hash) >= depth(&range.head)))
        {
            return invalid("member at or below a boundary level");
        }
        self.hashes.push(hash);
        Ok(())
    }

    /// After every member has ended: check closure and evidence. Returns the
    /// work used; call again until [`Self::result`] is available.
    pub fn finish(&mut self, budget: usize) -> Result<usize, Error> {
        match self.phase {
            Phase::Members => {
                if self.hashes.len() != self.meta.len() {
                    return Err(Error::Protocol("decoder finished before every member"));
                }
                self.check_structure()?;
                self.phase = Phase::Signatures(0);
                Ok(self.meta.len())
            }
            Phase::Signatures(start) => {
                let count = (budget / SIGNATURE_COST).max(1);
                let end = (start + count).min(self.meta.evidence.len());
                for e in &self.meta.evidence[start..end] {
                    verify_signature(&e.author, &self.hashes[e.member], &e.signature).map_err(
                        |error| match error {
                            Error::InvalidSignature { author, .. } => Error::InvalidSignature {
                                author,
                                member: e.member,
                            },
                            other => other,
                        },
                    )?;
                }
                self.phase = if end == self.meta.evidence.len() {
                    Phase::Coverage(Coverage::new(&self.meta))
                } else {
                    Phase::Signatures(end)
                };
                Ok((end - start) * SIGNATURE_COST)
            }
            Phase::Coverage(ref mut coverage) => {
                let work = coverage
                    .step(&self.meta, budget)
                    .map_err(|(author, member)| Error::MissingSignature { author, member })?;
                if let Some(rows) = coverage.rows() {
                    self.complete(rows);
                }
                Ok(work)
            }
            Phase::Done => Ok(0),
        }
    }

    fn check_structure(&mut self) -> Result<(), Error> {
        let head = self.meta.head();
        let Some(&head_index) = self.by_hash.get(&head) else {
            return invalid("head is not a member");
        };
        let head_depth = depth(&head);
        let fragment = match &self.meta.id {
            ItemId::Fragment(range) => Some(&range.boundary),
            ItemId::Commit(_) => None,
        };
        for member in &self.meta.members {
            for parent in &member.parents {
                if let ParentRef::External(h) = parent {
                    if self.by_hash.contains_key(h) {
                        return invalid("external parent is a member");
                    }
                    if let Some(boundary) = fragment
                        && !boundary.contains(h)
                        && depth(h) >= head_depth
                    {
                        return invalid("deep external parent is not a boundary");
                    }
                }
            }
        }
        // Every member must be an ancestor of (or equal to) the head.
        if reach(&self.meta, [head_index], |_| ()) != self.meta.len() {
            return invalid("member unreachable from head");
        }
        Ok(())
    }

    fn complete(&mut self, evidence: Vec<usize>) {
        let external = self
            .meta
            .members
            .iter()
            .flat_map(|m| &m.parents)
            .filter_map(|p| match p {
                ParentRef::External(h) => Some(*h),
                ParentRef::Member(_) => None,
            })
            .collect();
        self.result = Some(Arc::new(Verified {
            meta: Arc::clone(&self.meta),
            hashes: std::mem::take(&mut self.hashes),
            external,
            evidence,
        }));
        self.by_hash = BTreeMap::new();
        self.phase = Phase::Done;
    }
}
