//! Export inventory: one certificate per admitted bundle or assembled draft,
//! minus those a later fragment fully covers. An export cursor records which
//! inventory items a destination has acknowledged.
use super::{History, NodeIdx, SignatureEvidence};
use crate::meta::Evidence;
use crate::{
    BundleMeta, CommitHash, Error, FormatId, FragmentRange, ItemId, MemberShape, ParentRef,
    PayloadId,
};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Key {
    Commit(CommitHash),
    /// A fragment's boundary is fixed by its head.
    Fragment(CommitHash),
}

impl Key {
    pub fn of(id: &ItemId) -> Self {
        match id {
            ItemId::Commit(h) => Self::Commit(*h),
            ItemId::Fragment(r) => Self::Fragment(r.head),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Certificate {
    pub id: ItemId,
    pub evidence: Vec<SignatureEvidence>,
}

/// The inventory items a destination has acknowledged. Persist it with the
/// destination's state; it is independent of node indices and admission order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportCursor {
    items: BTreeSet<ItemId>,
}

const CURSOR_MAGIC: &[u8; 5] = b"SFXC\x01";

impl ExportCursor {
    pub fn new() -> Self {
        Self::default()
    }
    /// Restore the inventory a destination has durably acknowledged.
    pub fn from_items(items: impl IntoIterator<Item = ItemId>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn items(&self) -> impl Iterator<Item = &ItemId> {
        self.items.iter()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = CURSOR_MAGIC.to_vec();
        out.extend((self.items.len() as u64).to_le_bytes());
        for item in &self.items {
            match item {
                ItemId::Commit(h) => {
                    out.push(0);
                    out.extend(h.as_bytes());
                }
                ItemId::Fragment(r) => {
                    out.push(1);
                    out.extend(r.head.as_bytes());
                    out.extend((r.boundary.len() as u64).to_le_bytes());
                    for b in &r.boundary {
                        out.extend(b.as_bytes());
                    }
                }
            }
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bad = Error::Malformed("export cursor");
        let mut r = crate::reader::Reader::new(bytes);
        if r.take(5).map_err(|_| bad.clone())? != CURSOR_MAGIC {
            return Err(bad);
        }
        let count = r.u64_le().map_err(|_| bad.clone())?;
        let mut items = BTreeSet::new();
        for _ in 0..count {
            let item = match r.take(1).map_err(|_| bad.clone())?[0] {
                0 => ItemId::Commit(r.hash().map_err(|_| bad.clone())?),
                1 => {
                    let head = r.hash().map_err(|_| bad.clone())?;
                    let n = r.u64_le().map_err(|_| bad.clone())?;
                    let mut boundary = BTreeSet::new();
                    for _ in 0..n {
                        boundary.insert(r.hash().map_err(|_| bad.clone())?);
                    }
                    ItemId::Fragment(FragmentRange { head, boundary })
                }
                _ => return Err(bad),
            };
            items.insert(item);
        }
        r.finish().map_err(|_| bad)?;
        Ok(Self { items })
    }
}

/// Items to send a destination, captured at planning time. Later admissions
/// belong to a later plan. Items are ordered by their heads' delivery order.
#[derive(Debug)]
pub struct ExportPlan {
    history: u64,
    added: Vec<Arc<Certificate>>,
    retired: Vec<ItemId>,
    kept: BTreeSet<ItemId>,
    acknowledged: Vec<bool>,
}

impl ExportPlan {
    /// Number of items to send.
    pub fn len(&self) -> usize {
        self.added.len()
    }
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
    }
    pub fn id(&self, index: usize) -> Option<&ItemId> {
        self.added.get(index).map(|c| &c.id)
    }
    /// Items the destination holds that are no longer in the inventory because a
    /// fragment covers them. These are not deletion commands.
    pub fn retired(&self) -> &[ItemId] {
        &self.retired
    }
    /// Record that the destination durably accepted item `index`.
    pub fn acknowledge(&mut self, index: usize) -> Result<(), Error> {
        *self
            .acknowledged
            .get_mut(index)
            .ok_or(Error::OutOfRange("export item"))? = true;
        Ok(())
    }
    /// The cursor after the acknowledged items. Unacknowledged items will be
    /// planned again next time.
    pub fn cursor(&self) -> ExportCursor {
        let mut items = self.kept.clone();
        for (certificate, acked) in self.added.iter().zip(&self.acknowledged) {
            if *acked {
                items.insert(certificate.id.clone());
            }
        }
        ExportCursor { items }
    }
}

/// Everything an encoder needs for one exported item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportItem {
    pub meta: Arc<BundleMeta>,
    /// Hash and retained payload ID of each member, aligned with `meta`.
    pub members: Vec<(CommitHash, PayloadId)>,
}

impl ExportItem {
    /// Serialize the wire v2 header for this item under `format`.
    pub fn encode_header(&self, format: &FormatId) -> Vec<u8> {
        crate::wire::encode_header(&self.meta, format)
    }
}

pub(super) fn plan_export(history: &History, cursor: &ExportCursor) -> ExportPlan {
    let mut added = Vec::new();
    let mut kept = BTreeSet::new();
    let mut retired = Vec::new();
    for certificate in history.inventory.values() {
        if cursor.items.contains(&certificate.id) {
            kept.insert(certificate.id.clone());
        } else {
            added.push(Arc::clone(certificate));
        }
    }
    for id in &cursor.items {
        if !kept.contains(id) {
            retired.push(id.clone());
        }
    }
    // Heads in delivery order put most dependencies first. A receiver must
    // still tolerate waiting: a dependency's covering item can come later.
    added.sort_by_key(|c| history.by_hash[&c.id.head()]);
    ExportPlan {
        history: history.id,
        acknowledged: vec![false; added.len()],
        added,
        retired,
        kept,
    }
}

pub(super) fn build_export_item(
    history: &History,
    plan: &ExportPlan,
    index: usize,
) -> Result<ExportItem, Error> {
    if plan.history != history.id {
        return Err(Error::WrongHistory);
    }
    let certificate = plan
        .added
        .get(index)
        .ok_or(Error::OutOfRange("export item"))?;
    let members = match &certificate.id {
        ItemId::Commit(h) => vec![history.by_hash[h]],
        ItemId::Fragment(range) => {
            let head = history.by_hash[&range.head];
            let boundary: Vec<_> = range.boundary.iter().map(|b| history.by_hash[b]).collect();
            canonical_order(history, &history.fragment_members(head, &boundary))
        }
    };
    let position: HashMap<NodeIdx, usize> =
        members.iter().enumerate().map(|(i, &m)| (m, i)).collect();
    let shape = members
        .iter()
        .map(|&m| MemberShape {
            author: history.author_of(m),
            parents: history
                .parent_nodes(m)
                .iter()
                .map(|p| match position.get(p) {
                    Some(&i) => ParentRef::Member(i),
                    None => ParentRef::External(history.node(*p).hash),
                })
                .collect(),
        })
        .collect();
    let evidence = certificate
        .evidence
        .iter()
        .map(|e| Evidence {
            author: e.author,
            member: position[&history.by_hash[&e.commit]],
            signature: e.signature,
        })
        .collect();
    Ok(ExportItem {
        meta: Arc::new(BundleMeta {
            id: certificate.id.clone(),
            members: shape,
            evidence,
        }),
        members: members
            .iter()
            .map(|&m| (history.node(m).hash, history.node(m).payload))
            .collect(),
    })
}

/// Parents first; among ready members the smallest hash goes first. This
/// order depends only on the members, not on local delivery order.
fn canonical_order(history: &History, members: &[NodeIdx]) -> Vec<NodeIdx> {
    let set: std::collections::HashSet<_> = members.iter().copied().collect();
    let mut remaining = HashMap::new();
    let mut children: HashMap<NodeIdx, Vec<NodeIdx>> = HashMap::new();
    let mut ready = BTreeSet::new();
    for &m in members {
        let parents: BTreeSet<_> = history
            .parent_nodes(m)
            .iter()
            .copied()
            .filter(|p| set.contains(p))
            .collect();
        remaining.insert(m, parents.len());
        if parents.is_empty() {
            ready.insert((history.node(m).hash, m));
        }
        for p in parents {
            children.entry(p).or_default().push(m);
        }
    }
    let mut order = Vec::with_capacity(members.len());
    while let Some((_, m)) = ready.pop_first() {
        order.push(m);
        for &c in children.get(&m).into_iter().flatten() {
            let count = remaining.get_mut(&c).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert((history.node(c).hash, c));
            }
        }
    }
    order
}
