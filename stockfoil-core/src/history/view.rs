//! Views, participation, and metadata-only change batches for materializers.
//!
//! A [`ViewPoint`] is a set of history heads plus write permissions. A commit
//! *participates* in a view if it is in the heads' ancestry and its author's
//! permission allows it. Change planning compares two participation sets and the
//! materializer's previously delivered history, and enumerates only the chain
//! intervals that differ, so small updates cost about as much as they change.
use super::clock::{self, Clock};
use super::{Heads, History, NodeIdx};
use crate::verified::events;
pub use crate::verified::events::Vis;
use crate::{Author, CommitHash, Error, PayloadId, WritePermission};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub type WritePermissions = HashMap<Author, WritePermission>;

/// History heads and write permissions. Missing authors are denied; bounds
/// outside the view's history grant nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewPoint {
    heads: Vec<CommitHash>,
    write_permissions: WritePermissions,
}

impl ViewPoint {
    pub fn from_parts(heads: Vec<CommitHash>, write_permissions: WritePermissions) -> Self {
        Self {
            heads,
            write_permissions,
        }
    }
    pub fn heads(&self) -> &[CommitHash] {
        &self.heads
    }
    pub fn write_permissions(&self) -> &WritePermissions {
        &self.write_permissions
    }
}

/// A materializer's completed progress: the history it has been told about and
/// the view it materialized. Persist it atomically with the materialized state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaterializationCursor {
    delivery_heads: Vec<CommitHash>,
    view_point: ViewPoint,
}

const CURSOR_MAGIC: &[u8; 5] = b"SFMC\x01";

impl MaterializationCursor {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn delivery_heads(&self) -> &[CommitHash] {
        &self.delivery_heads
    }
    pub fn view_point(&self) -> &ViewPoint {
        &self.view_point
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = CURSOR_MAGIC.to_vec();
        let hashes = |out: &mut Vec<u8>, hashes: &[CommitHash]| {
            out.extend((hashes.len() as u64).to_le_bytes());
            for h in hashes {
                out.extend(h.as_bytes());
            }
        };
        hashes(&mut out, &self.delivery_heads);
        hashes(&mut out, &self.view_point.heads);
        let mut permissions: Vec<_> = self.view_point.write_permissions.iter().collect();
        permissions.sort_by_key(|(a, _)| **a);
        out.extend((permissions.len() as u64).to_le_bytes());
        for (author, permission) in permissions {
            out.extend(author.as_slice());
            match permission {
                WritePermission::Unbounded => out.push(0),
                WritePermission::Denied => out.push(1),
                WritePermission::Bounded(frontier) => {
                    out.push(2);
                    hashes(&mut out, frontier);
                }
            }
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bad = || Error::Malformed("materialization cursor");
        let mut r = crate::reader::Reader::new(bytes);
        if r.take(5).map_err(|_| bad())? != CURSOR_MAGIC {
            return Err(bad());
        }
        let hashes = |r: &mut crate::reader::Reader| -> Result<Vec<CommitHash>, Error> {
            let n = r.u64_le().map_err(|_| bad())?;
            (0..n).map(|_| r.hash().map_err(|_| bad())).collect()
        };
        let delivery_heads = hashes(&mut r)?;
        let heads = hashes(&mut r)?;
        let n = r.u64_le().map_err(|_| bad())?;
        let mut write_permissions = HashMap::new();
        for _ in 0..n {
            let author =
                crate::Author::try_from(r.take(32).map_err(|_| bad())?).map_err(|_| bad())?;
            let permission = match r.take(1).map_err(|_| bad())?[0] {
                0 => WritePermission::Unbounded,
                1 => WritePermission::Denied,
                2 => WritePermission::Bounded(hashes(&mut r)?),
                _ => return Err(bad()),
            };
            write_permissions.insert(author, permission);
        }
        r.finish().map_err(|_| bad())?;
        Ok(Self {
            delivery_heads,
            view_point: ViewPoint::from_parts(heads, write_permissions),
        })
    }
}

/// One metadata change for a materializer. Contains no payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub hash: CommitHash,
    /// The retained logical payload ID.
    pub payload: PayloadId,
    /// Not delivered to this materializer before. Store it even if it does not
    /// participate now: a later view may enable it.
    pub newly_delivered: bool,
    /// Participation before (the cursor's view) and after (the target view).
    pub visibility: Vis,
}

/// A captured change traversal. Later deliveries belong to a later traversal.
#[derive(Debug)]
pub struct Changes {
    history: u64,
    pending: Vec<NodeIdx>,
    next: usize,
    delivered_before: Arc<Clock>,
    before: Clock,
    after: Clock,
    delivery_heads: Vec<CommitHash>,
    target: ViewPoint,
}

impl Changes {
    /// Records remaining in this traversal.
    pub fn remaining(&self) -> usize {
        self.pending.len() - self.next
    }
}

/// Whether a change traversal has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Batch {
    More,
    /// Every record has been emitted. Persist this cursor together with the
    /// application effects of the whole traversal.
    Complete(MaterializationCursor),
}

pub(super) fn view(history: &History, heads: Heads, permissions: &WritePermissions) -> ViewPoint {
    ViewPoint::from_parts(history.heads(heads).collect(), permissions.clone())
}

pub(super) fn plan_changes(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> Result<Changes, Error> {
    let delivered_before = history.history_clock(&cursor.delivery_heads)?;
    let before = participation(history, &cursor.view_point)?;
    let after = participation(history, target)?;
    let delivered_now = Clock::from_tips(&history.chain_tips);
    let mut pending = Vec::new();
    clock::difference(
        &history.chain_nodes,
        &delivered_now,
        &delivered_before,
        &mut pending,
    );
    clock::symmetric_difference(&history.chain_nodes, &before, &after, &mut pending);
    crate::verified::candidates::normalize(&mut pending);
    Ok(Changes {
        history: history.id,
        pending,
        next: 0,
        delivered_before,
        before,
        after,
        delivery_heads: history.heads.iter().copied().collect(),
        target: target.clone(),
    })
}

pub(super) fn emit_next_changes(
    history: &History,
    changes: &mut Changes,
    out: &mut Vec<Change>,
    max: usize,
) -> Result<Batch, Error> {
    if changes.history != history.id {
        return Err(Error::WrongHistory);
    }
    let end = events::batch_end(changes.next, changes.pending.len(), max);
    for &idx in &changes.pending[changes.next..end] {
        let node = history.node(idx);
        out.push(Change {
            hash: node.hash,
            payload: node.payload,
            newly_delivered: !history.contains(&changes.delivered_before, idx),
            visibility: visibility(history, &changes.before, &changes.after, idx),
        });
    }
    changes.next = end;
    if end < changes.pending.len() {
        return Ok(Batch::More);
    }
    Ok(Batch::Complete(MaterializationCursor {
        delivery_heads: changes.delivery_heads.clone(),
        view_point: changes.target.clone(),
    }))
}

fn visibility(history: &History, before: &Clock, after: &Clock, idx: NodeIdx) -> Vis {
    events::classify(history.contains(before, idx), history.contains(after, idx))
}

/// The view's ancestry, with each chain capped by its author's permission.
fn participation(history: &History, view: &ViewPoint) -> Result<Clock, Error> {
    let reached = history.history_clock(&view.heads)?;
    let mut bounded: HashMap<Author, Arc<Clock>> = HashMap::new();
    Ok(reached.mask(|chain, tip| {
        let author = history.chain_authors[chain as usize];
        match view.write_permissions.get(&author) {
            Some(WritePermission::Unbounded) => Some(tip),
            Some(WritePermission::Bounded(frontier)) => bounded
                .entry(author)
                .or_insert_with(|| {
                    let available: Vec<_> = frontier
                        .iter()
                        .filter_map(|h| history.by_hash.get(h).copied())
                        .filter(|&idx| history.contains(&reached, idx))
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    history.clock_for_heads(&available)
                })
                .position(chain),
            Some(WritePermission::Denied) | None => None,
        }
    }))
}
