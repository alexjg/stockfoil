//! Certificates: recording admitted evidence in the export inventory, and
//! assembling certificates for signed local drafts.
use super::export::{Certificate, Key};
use super::{History, NodeIdx, SignatureEvidence};
use crate::evidence::coverage;
use crate::{BundleMeta, Evidence, FragmentRange, FragmentShape, ItemId, MemberShape, ParentRef};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

impl History {
    /// Record a certificate and its evidence. Fragments retire standalone
    /// certificates and smaller fragments that they fully cover.
    pub(super) fn add_certificate(
        &mut self,
        id: ItemId,
        evidence: Vec<SignatureEvidence>,
        members: &[NodeIdx],
    ) {
        for e in &evidence {
            let idx = self.by_hash[&e.commit];
            self.signatures
                .entry(idx)
                .or_default()
                .insert(e.author, e.signature);
        }
        let key = Key::of(&id);
        match &id {
            ItemId::Commit(hash) => {
                if self.node(self.by_hash[hash]).in_fragment || self.inventory.contains_key(&key) {
                    return;
                }
            }
            ItemId::Fragment(range) => {
                if self.inventory.contains_key(&key) {
                    return;
                }
                let covering: HashSet<NodeIdx> = members.iter().copied().collect();
                for &m in members {
                    self.nodes[m.index()].in_fragment = true;
                    let hash = self.node(m).hash;
                    self.inventory.remove(&Key::Commit(hash));
                    if hash == range.head || self.node(m).depth == 0 {
                        continue;
                    }
                    if let Some(inner) = self.inventory.get(&Key::Fragment(hash))
                        && let ItemId::Fragment(inner_range) = &inner.id
                    {
                        let boundary: Vec<_> = inner_range
                            .boundary
                            .iter()
                            .map(|b| self.by_hash[b])
                            .collect();
                        if self
                            .fragment_members(m, &boundary)
                            .iter()
                            .all(|n| covering.contains(n))
                        {
                            self.inventory.remove(&Key::Fragment(hash));
                        }
                    }
                }
            }
        }
        self.inventory
            .insert(key, Arc::new(Certificate { id, evidence }));
    }

    /// Admit every signed draft whose certificate is complete. Drafts are tried in
    /// topological order, repeating until nothing more can be admitted.
    pub(super) fn admit_ready_drafts(&mut self) -> Vec<ItemId> {
        let mut admitted = Vec::new();
        loop {
            let mut progress = false;
            let candidates: Vec<_> = self.signed_drafts.iter().copied().collect();
            for idx in candidates {
                if self.node(idx).admitted {
                    continue;
                }
                if let Some((id, members, evidence)) = self.assemble(idx) {
                    for &m in &members {
                        self.mark_admitted(m);
                    }
                    self.add_certificate(id.clone(), evidence, &members);
                    admitted.push(id);
                    progress = true;
                }
            }
            if !progress {
                return admitted;
            }
        }
    }

    /// Build a certificate for a signed draft: a standalone commit at depth 0,
    /// otherwise its canonical fragment. `None` until signatures cover every
    /// member and every dependency is admitted.
    fn assemble(&self, head: NodeIdx) -> Option<(ItemId, Vec<NodeIdx>, Vec<SignatureEvidence>)> {
        let level = self.node(head).depth as usize;
        let (id, members, boundary) = if level == 0 {
            (
                ItemId::Commit(self.node(head).hash),
                vec![head],
                BTreeSet::new(),
            )
        } else {
            let boundary = self.fragment_boundary(head, level);
            let members =
                self.fragment_members(head, &boundary.iter().copied().collect::<Vec<_>>());
            let range = FragmentRange {
                head: self.node(head).hash,
                boundary: boundary.iter().map(|&b| self.node(b).hash).collect(),
            };
            (ItemId::Fragment(range), members, boundary)
        };
        if boundary.iter().any(|&b| !self.node(b).admitted) {
            return None;
        }
        let position: HashMap<NodeIdx, usize> =
            members.iter().enumerate().map(|(i, &m)| (m, i)).collect();
        let mut shape = Vec::with_capacity(members.len());
        for &m in &members {
            let mut parents = Vec::new();
            for &p in self.parent_nodes(m) {
                parents.push(match position.get(&p) {
                    Some(&i) => ParentRef::Member(i),
                    None if self.node(p).admitted => ParentRef::External(self.node(p).hash),
                    None => return None,
                });
            }
            shape.push(MemberShape {
                author: self.author_of(m),
                parents,
            });
        }
        let mut evidence = Vec::new();
        for (i, &m) in members.iter().enumerate() {
            for (author, signature) in self.signatures.get(&m).into_iter().flatten() {
                evidence.push(Evidence {
                    author: *author,
                    member: i,
                    signature: *signature,
                });
            }
        }
        let meta = BundleMeta {
            id,
            shape: FragmentShape { members: shape },
            evidence,
        };
        let Ok(rows) = coverage(&meta) else {
            return self.assemble_standalone(head);
        };
        let evidence = rows
            .into_iter()
            .map(|row| {
                let e = &meta.evidence[row];
                SignatureEvidence {
                    author: e.author,
                    commit: self.node(members[e.member]).hash,
                    signature: e.signature,
                }
            })
            .collect();
        Some((meta.id, members, evidence))
    }

    /// The fallback when a fragment cannot be certified because some admitted
    /// member's evidence lies outside it (for example, a commit admitted because
    /// its author signed a descendant). If the head's author signed it and every
    /// parent is admitted, it is certified on its own; a standalone commit may
    /// have any depth.
    fn assemble_standalone(
        &self,
        head: NodeIdx,
    ) -> Option<(ItemId, Vec<NodeIdx>, Vec<SignatureEvidence>)> {
        let node = self.node(head);
        let author = self.author_of(head);
        let signature = *self.signatures.get(&head)?.get(&author)?;
        if self
            .parent_nodes(head)
            .iter()
            .any(|&p| !self.node(p).admitted)
        {
            return None;
        }
        let evidence = vec![SignatureEvidence {
            author,
            commit: node.hash,
            signature,
        }];
        Some((ItemId::Commit(node.hash), vec![head], evidence))
    }
}
