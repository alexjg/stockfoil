//! Certificates: recording admitted evidence in the export inventory, and
//! assembling certificates for signed local drafts.
use super::export::{Certificate, Key};
use super::{History, NodeIdx, SignatureEvidence};
use crate::evidence::coverage;
use crate::meta::Evidence;
use crate::{BundleMeta, FragmentRange, ItemId, MemberShape, ParentRef};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

    /// Record a certificate and its evidence. An item whose members all lie in
    /// admitted fragments adds nothing; otherwise a fragment retires standalone
    /// certificates and smaller fragments that it fully covers.
pub(super) fn add_certificate(
    history: &mut History,
        id: ItemId,
        evidence: Vec<SignatureEvidence>,
        members: &[NodeIdx],
    ) {
        for e in &evidence {
            let idx = history.by_hash[&e.commit];
            history.signatures
                .entry(idx)
                .or_default()
                .insert(e.author, e.signature);
        }
        let key = Key::of(&id);
        if history.inventory.contains_key(&key) || members.iter().all(|&m| history.node(m).in_fragment) {
            return;
        }
        if let ItemId::Fragment(range) = &id {
            let covering: HashSet<NodeIdx> = members.iter().copied().collect();
            for &m in members {
                history.nodes[m.index()].in_fragment = true;
                let hash = history.node(m).hash;
                history.inventory.remove(&Key::Commit(hash));
                if hash == range.head || history.node(m).depth == 0 {
                    continue;
                }
                if let Some(inner) = history.inventory.get(&Key::Fragment(hash))
                    && let ItemId::Fragment(inner_range) = &inner.id
                {
                    let boundary: Vec<_> = inner_range
                        .boundary
                        .iter()
                        .map(|b| history.by_hash[b])
                        .collect();
                    if history
                        .fragment_members(m, &boundary)
                        .iter()
                        .all(|n| covering.contains(n))
                    {
                        history.inventory.remove(&Key::Fragment(hash));
                    }
                }
            }
        }
        history.inventory
            .insert(key, Arc::new(Certificate { id, evidence }));
    }

    /// Admit every signed draft whose certificate is complete. Drafts are tried in
    /// topological order, repeating until nothing more can be admitted.
pub(super) fn admit_ready_drafts(history: &mut History) -> Vec<ItemId> {
        let mut admitted = Vec::new();
        loop {
            let mut progress = false;
            let candidates: Vec<_> = history.signed_drafts.iter().copied().collect();
            for idx in candidates {
                if history.node(idx).admitted {
                    continue;
                }
                if let Some((id, members, evidence)) = assemble(history, idx) {
                    for &m in &members {
                        history.mark_admitted(m);
                    }
                    add_certificate(history, id.clone(), evidence, &members);
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
fn assemble(history: &History, head: NodeIdx) -> Option<(ItemId, Vec<NodeIdx>, Vec<SignatureEvidence>)> {
        let level = history.node(head).depth as usize;
        let (id, members, boundary) = if level == 0 {
            (
                ItemId::Commit(history.node(head).hash),
                vec![head],
                BTreeSet::new(),
            )
        } else {
            let boundary = history.fragment_boundary(head, level);
            let members =
                history.fragment_members(head, &boundary.iter().copied().collect::<Vec<_>>());
            let range = FragmentRange {
                head: history.node(head).hash,
                boundary: boundary.iter().map(|&b| history.node(b).hash).collect(),
            };
            (ItemId::Fragment(range), members, boundary)
        };
        if boundary.iter().any(|&b| !history.node(b).admitted) {
            return None;
        }
        let position: HashMap<NodeIdx, usize> =
            members.iter().enumerate().map(|(i, &m)| (m, i)).collect();
        let mut shape = Vec::with_capacity(members.len());
        for &m in &members {
            let mut parents = Vec::new();
            for &p in history.parent_nodes(m) {
                parents.push(match position.get(&p) {
                    Some(&i) => ParentRef::Member(i),
                    None if history.node(p).admitted => ParentRef::External(history.node(p).hash),
                    None => return None,
                });
            }
            shape.push(MemberShape {
                author: history.author_of(m),
                parents,
            });
        }
        let mut evidence = Vec::new();
        for (i, &m) in members.iter().enumerate() {
            for (author, signature) in history.signatures.get(&m).into_iter().flatten() {
                evidence.push(Evidence {
                    author: *author,
                    member: i,
                    signature: *signature,
                });
            }
        }
        let meta = BundleMeta {
            id,
            members: shape,
            evidence,
        };
        let Ok(rows) = coverage(&meta) else {
            return assemble_standalone(history, head);
        };
        let evidence = rows
            .into_iter()
            .map(|row| {
                let e = &meta.evidence[row];
                SignatureEvidence {
                    author: e.author,
                    commit: history.node(members[e.member]).hash,
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
    history: &History,
        head: NodeIdx,
    ) -> Option<(ItemId, Vec<NodeIdx>, Vec<SignatureEvidence>)> {
        let node = history.node(head);
        let author = history.author_of(head);
        let signature = *history.signatures.get(&head)?.get(&author)?;
        if history
            .parent_nodes(head)
            .iter()
            .any(|&p| !history.node(p).admitted)
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
