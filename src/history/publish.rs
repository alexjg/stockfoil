//! Received bundles: dependency and placement checks, then atomic publication.
use super::{Delivered, History, SignatureEvidence};
use crate::meta::depth;
use crate::verify::Verified;
use crate::{CommitHash, Error, FragmentRange, ItemId, PayloadId};
use std::collections::{BTreeSet, HashSet};

/// The result of admitting a received bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub id: ItemId,
    /// One entry per bundle member, in bundle order.
    pub members: Vec<Delivered>,
    /// Local draft certificates this admission made complete.
    pub drafts_admitted: Vec<ItemId>,
}

pub(crate) enum Readiness {
    /// This dependency is not admitted yet.
    Waiting(CommitHash),
    /// Admissible. `undelivered` members need payload IDs.
    Ready { undelivered: bool },
}

impl History {
    /// Check dependencies and canonical fragment placement. Does not mutate.
    pub(crate) fn readiness(&self, verified: &Verified) -> Result<Readiness, Error> {
        if let Some(missing) = verified
            .dependencies()
            .into_iter()
            .find(|d| !self.is_admitted(d))
        {
            return Ok(Readiness::Waiting(missing));
        }
        if let ItemId::Fragment(range) = &verified.meta.id {
            self.check_placement(range, &verified.hashes, &verified.external)?;
        }
        Ok(Readiness::Ready {
            undelivered: verified
                .hashes
                .iter()
                .any(|h| !self.by_hash.contains_key(h)),
        })
    }

    /// A fragment's members lie above its boundary history, its external parents
    /// inside it, and walking down from those parents stops exactly at the boundary.
    fn check_placement(
        &self,
        range: &FragmentRange,
        members: &[CommitHash],
        external: &BTreeSet<CommitHash>,
    ) -> Result<(), Error> {
        let boundary: Vec<_> = range.boundary.iter().map(|h| self.by_hash[h]).collect();
        let excluded = self.clock_for_heads(&boundary);
        let below = |hash: &CommitHash| {
            self.by_hash
                .get(hash)
                .is_some_and(|&idx| self.contains(&excluded, idx))
        };
        if members.iter().any(below) {
            return Err(Error::InvalidPlacement);
        }
        let mut stack = Vec::with_capacity(external.len());
        for hash in external {
            if !below(hash) {
                return Err(Error::InvalidPlacement);
            }
            stack.push(self.by_hash[hash]);
        }
        let level = depth(&range.head);
        let mut stops = BTreeSet::new();
        let mut visited = HashSet::new();
        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }
            if self.node(idx).depth as usize >= level {
                stops.insert(self.node(idx).hash);
            } else {
                stack.extend(self.parent_nodes(idx));
            }
        }
        if stops != range.boundary {
            return Err(Error::InvalidPlacement);
        }
        Ok(())
    }

    /// Atomically deliver and admit a ready bundle. `payloads` holds one ID per
    /// member (only consulted for members not already delivered).
    pub(crate) fn publish_verified(
        &mut self,
        verified: &Verified,
        payloads: &[PayloadId],
    ) -> Receipt {
        let mut members = Vec::with_capacity(verified.hashes.len());
        let mut indices = Vec::with_capacity(verified.hashes.len());
        for (i, &hash) in verified.hashes.iter().enumerate() {
            let delivered = if let Some(&idx) = self.by_hash.get(&hash) {
                self.mark_admitted(idx);
                indices.push(idx);
                Delivered {
                    hash,
                    payload: self.node(idx).payload,
                    newly_delivered: false,
                }
            } else {
                let parents: Vec<_> = verified.parents(i).collect();
                let author = verified.meta.shape.members[i].author;
                let payload = payloads[i];
                indices.push(self.insert(hash, author, &parents, payload, true));
                Delivered {
                    hash,
                    payload,
                    newly_delivered: true,
                }
            };
            members.push(delivered);
        }
        let evidence = verified
            .meta
            .evidence
            .iter()
            .map(|e| SignatureEvidence {
                author: e.author,
                commit: verified.hashes[e.member],
                signature: e.signature,
            })
            .collect();
        self.add_certificate(verified.meta.id.clone(), evidence, &indices);
        let drafts_admitted = self.admit_ready_drafts();
        self.bump_epoch();
        Receipt {
            id: verified.meta.id.clone(),
            members,
            drafts_admitted,
        }
    }
}
