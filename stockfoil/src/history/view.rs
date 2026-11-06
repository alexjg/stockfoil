//! Views, participation, and metadata-only change batches for materializers.
//!
//! A [`ViewPoint`] is a set of history heads plus write permissions. A commit
//! *participates* in a view if it is in the heads' ancestry and its author's
//! permission allows it. Change planning compares two participation sets and the
//! materializer's previously delivered history, and enumerates only the chain
//! intervals that differ, so small updates cost about as much as they change.
use super::clock::Clock;
use super::traversal::Traversal;
use super::{Heads, History, NodeIdx};
use crate::{Author, CommitHash, Error, PayloadId, WritePermission};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Per-author write permissions, plus the rule for authors not listed.
///
/// Unlisted authors are [`Denied`](WritePermission::Denied) unless
/// [`with_unlisted`](Self::with_unlisted) says otherwise, for example
/// [`Unbounded`](WritePermission::Unbounded) when another layer decides who may
/// write. A listed author's own rule always takes precedence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WritePermissions {
    authors: HashMap<Author, WritePermission>,
    unlisted: WritePermission,
}

impl WritePermissions {
    /// No listed authors; unlisted authors are denied.
    pub fn new() -> Self {
        Self::default()
    }
    /// Set the rule for authors without their own entry.
    pub fn with_unlisted(mut self, permission: WritePermission) -> Self {
        self.unlisted = permission;
        self
    }
    /// The rule for authors without their own entry.
    pub fn unlisted(&self) -> &WritePermission {
        &self.unlisted
    }
    pub fn set_unlisted(&mut self, permission: WritePermission) {
        self.unlisted = permission;
    }
    /// The rule that applies to `author`, listed or not.
    pub fn permission(&self, author: &Author) -> &WritePermission {
        self.authors.get(author).unwrap_or(&self.unlisted)
    }
    /// Give `author` its own rule, returning any rule it replaces.
    pub fn insert(
        &mut self,
        author: Author,
        permission: WritePermission,
    ) -> Option<WritePermission> {
        self.authors.insert(author, permission)
    }
    /// Remove `author`'s own rule, so the unlisted rule applies to it.
    pub fn remove(&mut self, author: &Author) -> Option<WritePermission> {
        self.authors.remove(author)
    }
    /// Listed authors and their rules, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&Author, &WritePermission)> {
        self.authors.iter()
    }
}

impl FromIterator<(Author, WritePermission)> for WritePermissions {
    fn from_iter<I: IntoIterator<Item = (Author, WritePermission)>>(iter: I) -> Self {
        Self {
            authors: iter.into_iter().collect(),
            unlisted: WritePermission::Denied,
        }
    }
}

impl<const N: usize> From<[(Author, WritePermission); N]> for WritePermissions {
    fn from(authors: [(Author, WritePermission); N]) -> Self {
        authors.into_iter().collect()
    }
}

/// History heads and write permissions. Bounds outside the view's history
/// grant nothing.
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

    pub(super) fn encode_into(&self, out: &mut Vec<u8>) {
        fn encode_permission(out: &mut Vec<u8>, permission: &WritePermission) {
            match permission {
                WritePermission::Unbounded => out.push(0),
                WritePermission::Denied => out.push(1),
                WritePermission::Bounded(frontier) => {
                    out.push(2);
                    encode_hashes(out, frontier);
                }
            }
        }
        encode_hashes(out, &self.heads);
        let mut permissions: Vec<_> = self.write_permissions.iter().collect();
        permissions.sort_unstable_by_key(|(author, _)| **author);
        out.extend((permissions.len() as u64).to_le_bytes());
        for (author, permission) in permissions {
            out.extend(author.as_slice());
            encode_permission(out, permission);
        }
        encode_permission(out, self.write_permissions.unlisted());
    }

    pub(super) fn decode_from(r: &mut crate::reader::Reader<'_>) -> Result<Self, Error> {
        fn decode_permission(r: &mut crate::reader::Reader<'_>) -> Result<WritePermission, Error> {
            Ok(match r.take(1)?[0] {
                0 => WritePermission::Unbounded,
                1 => WritePermission::Denied,
                2 => WritePermission::Bounded(r.hashes()?),
                _ => return Err(Error::Malformed("view permission")),
            })
        }
        let heads = r.hashes()?;
        let count = r.count(33)?; // author + permission tag, at minimum
        let mut write_permissions = WritePermissions::new();
        for _ in 0..count {
            let author =
                Author::try_from(r.take(32)?).map_err(|_| Error::Malformed("view author"))?;
            let permission = decode_permission(r)?;
            if write_permissions.insert(author, permission).is_some() {
                return Err(Error::Malformed("duplicate view author"));
            }
        }
        write_permissions.set_unlisted(decode_permission(r)?);
        Ok(Self {
            heads,
            write_permissions,
        })
    }
}

pub(super) fn encode_hashes(out: &mut Vec<u8>, hashes: &[CommitHash]) {
    out.extend((hashes.len() as u64).to_le_bytes());
    for hash in hashes {
        out.extend(hash.as_bytes());
    }
}

/// Participation of one commit in two views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    Before,
    After,
    Both,
    Neither,
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
    pub(crate) fn from_parts(delivery_heads: Vec<CommitHash>, view_point: ViewPoint) -> Self {
        Self {
            delivery_heads,
            view_point,
        }
    }
    pub fn delivery_heads(&self) -> &[CommitHash] {
        &self.delivery_heads
    }
    pub fn view_point(&self) -> &ViewPoint {
        &self.view_point
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = CURSOR_MAGIC.to_vec();
        encode_hashes(&mut out, &self.delivery_heads);
        self.view_point.encode_into(&mut out);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bad = || Error::Malformed("materialization cursor");
        let mut r = crate::reader::Reader::new(bytes);
        if r.take(5).map_err(|_| bad())? != CURSOR_MAGIC {
            return Err(bad());
        }
        let delivery_heads = r.hashes().map_err(|_| bad())?;
        let view_point = ViewPoint::decode_from(&mut r).map_err(|_| bad())?;
        r.finish().map_err(|_| bad())?;
        Ok(Self {
            delivery_heads,
            view_point,
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
///
/// Records are ancestors-first, with the smallest hash first among records
/// whose selected ancestors have all been emitted. This order depends only on
/// the selected commits and their ancestry, not on local delivery order or
/// batch size. Unchanged commits between two selected commits still impose
/// ordering. The traversal retains intervals and a frontier per touched chain,
/// rather than a list of every selected commit.
#[derive(Debug)]
pub struct Changes {
    history: u64,
    traversal: Traversal,
    delivered_before: Arc<Clock>,
    before: Clock,
    after: Clock,
    delivery_heads: Vec<CommitHash>,
    target: ViewPoint,
}

impl Changes {
    /// Records remaining in this traversal, without expanding its intervals.
    pub fn remaining(&self) -> usize {
        self.traversal.remaining()
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

#[cfg(any(test, feature = "fuzzing"))]
pub(super) fn plan_changes(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> Result<Changes, Error> {
    let delivery_heads: Vec<_> = history.heads.iter().copied().collect();
    plan_changes_at(history, cursor, target, &delivery_heads)
}

/// Re-plan a captured traversal after a restart without absorbing later
/// deliveries. Canonical order plus these delivery heads fixes the record set.
pub(crate) fn plan_changes_at(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
    delivery_heads: &[CommitHash],
) -> Result<Changes, Error> {
    let delivered_before = history.history_clock(&cursor.delivery_heads)?;
    let before = participation(history, &cursor.view_point)?;
    let after = participation(history, target)?;
    let delivered_now = history.history_clock(delivery_heads)?;
    for hash in cursor
        .delivery_heads
        .iter()
        .chain(&cursor.view_point.heads)
        .chain(&target.heads)
    {
        let idx = history.by_hash[hash]; // validated by the clock queries above
        if !history.contains(&delivered_now, idx) {
            return Err(Error::InvalidItem(
                "captured delivery history excludes a view or cursor head",
            ));
        }
    }
    Ok(Changes {
        history: history.id,
        traversal: Traversal::new(
            history,
            &[
                (&delivered_now, &delivered_before),
                (&before, &after),
                (&after, &before),
            ],
        ),
        delivered_before,
        before,
        after,
        delivery_heads: delivery_heads.to_vec(),
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
    for _ in 0..max.min(changes.remaining()) {
        let idx = changes
            .traversal
            .next(history)
            .expect("an acyclic frontier has a ready record");
        let node = history.node(idx);
        out.push(Change {
            hash: node.hash,
            payload: node.payload,
            newly_delivered: !history.contains(&changes.delivered_before, idx),
            visibility: visibility(history, &changes.before, &changes.after, idx),
        });
    }
    if changes.remaining() > 0 {
        return Ok(Batch::More);
    }
    Ok(Batch::Complete(MaterializationCursor {
        delivery_heads: changes.delivery_heads.clone(),
        view_point: changes.target.clone(),
    }))
}

fn visibility(history: &History, before: &Clock, after: &Clock, idx: NodeIdx) -> Vis {
    match (history.contains(before, idx), history.contains(after, idx)) {
        (true, false) => Vis::Before,
        (false, true) => Vis::After,
        (true, true) => Vis::Both,
        (false, false) => Vis::Neither,
    }
}

/// Whether already delivered commits change participation (new deliveries
/// alone do not require resetting a state without visibility support).
pub(crate) fn visibility_changed(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> Result<bool, Error> {
    let delivered = history.history_clock(cursor.delivery_heads())?;
    let before = participation(history, cursor.view_point())?;
    let after = participation(history, target)?;
    let old_after = after.mask(|chain, _| delivered.position(chain));
    Ok(before != old_after)
}

/// The view's ancestry, with each chain capped by its author's permission.
fn participation(history: &History, view: &ViewPoint) -> Result<Clock, Error> {
    let reached = history.history_clock(&view.heads)?;
    Ok(participation_in(history, &reached, &view.write_permissions))
}

/// Apply permissions to an already captured causal past. Shared with
/// `Ancestry` so both APIs use exactly the same frontier semantics.
pub(super) fn participation_in(
    history: &History,
    reached: &Clock,
    permissions: &WritePermissions,
) -> Clock {
    // Keyed by author, or `None` for the shared unlisted frontier.
    let mut bounded: HashMap<Option<Author>, Arc<Clock>> = HashMap::new();
    reached.mask(|chain, tip| {
        let author = history.chain_authors[chain as usize];
        let listed = permissions.authors.get(&author);
        match listed.unwrap_or(&permissions.unlisted) {
            WritePermission::Unbounded => Some(tip),
            WritePermission::Bounded(frontier) => bounded
                .entry(listed.map(|_| author))
                .or_insert_with(|| {
                    let available: Vec<_> = frontier
                        .iter()
                        .filter_map(|h| history.by_hash.get(h).copied())
                        .filter(|&idx| history.contains(reached, idx))
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    history.clock_for_heads(&available)
                })
                .position(chain),
            WritePermission::Denied => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn traversal_retains_intervals_not_one_entry_per_record() {
        let author = Author::from(&SigningKey::from_bytes(&[1; 32]));
        let mut history = History::new();
        let mut hashes = Vec::new();
        for i in 0..10_000u64 {
            let mut builder = history
                .begin_commit(author, hashes.last().copied().into_iter().collect())
                .unwrap();
            builder.update(&i.to_le_bytes());
            let prepared = builder.finish();
            hashes.push(prepared.hash);
            history.publish_commit(prepared, i).unwrap();
        }
        let view = history.view_point(&WritePermissions::from([(
            author,
            WritePermission::Bounded(vec![hashes[99]]),
        )]));
        let mut cold = history
            .changes(&MaterializationCursor::new(), &view)
            .unwrap();
        assert_eq!(cold.remaining(), 10_000);
        assert_eq!(cold.traversal.retained_counts(), (1, 1, 1, 0));
        let mut batch = Vec::new();
        let cursor = loop {
            batch.clear();
            if let Batch::Complete(cursor) =
                history.next_changes(&mut cold, &mut batch, 128).unwrap()
            {
                break cursor;
            }
        };
        assert_eq!(cold.remaining(), 0);
        assert_eq!(cold.traversal.retained_counts(), (0, 0, 0, 0));

        for i in 10_000..10_030u64 {
            let mut builder = history
                .begin_commit(author, vec![*hashes.last().unwrap()])
                .unwrap();
            builder.update(&i.to_le_bytes());
            let prepared = builder.finish();
            hashes.push(prepared.hash);
            history.publish_commit(prepared, i).unwrap();
        }
        // Two disjoint intervals on the same chain: retract 90 earlier writes
        // and deliver 30 new inactive writes, skipping 9,900 unchanged commits.
        let view = history.view_point(&WritePermissions::from([(
            author,
            WritePermission::Bounded(vec![hashes[9]]),
        )]));
        let mut changes = history.changes(&cursor, &view).unwrap();
        assert_eq!(changes.remaining(), 120);
        assert_eq!(changes.traversal.retained_counts(), (1, 2, 1, 0));
        batch.clear();
        assert!(matches!(
            history.next_changes(&mut changes, &mut batch, 200).unwrap(),
            Batch::Complete(_)
        ));
        let expected: Vec<_> = hashes[10..100]
            .iter()
            .chain(&hashes[10_000..])
            .copied()
            .collect();
        assert_eq!(batch.iter().map(|c| c.hash).collect::<Vec<_>>(), expected);
    }
}
