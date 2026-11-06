//! Durable materialization progress. No payload IDs, node indices, or I/O.
use super::view::{MaterializationCursor, ViewPoint, encode_hashes};
use crate::reader::Reader;
use crate::{CommitHash, Error};
use std::collections::BTreeMap;

const MAGIC: &[u8; 5] = b"SFCP\x01";

/// Which view of a ledger a materialization follows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Follow {
    /// Everything delivered, including unsigned local drafts: the working copy.
    #[default]
    Working,
    /// Authenticated history only. Drafts participate once signed.
    Signed,
    /// Fixed heads. A materialization can follow a branch by changing its pin;
    /// a local edit against it commits on these heads and moves the pin.
    Pinned(Vec<CommitHash>),
}

/// Durable progress of one materialization, to save atomically with its state.
///
/// The checkpoint contains the follow rule, the last completed cursor, rejected
/// hashes (and whether their descendants are tainted), and optionally a captured
/// traversal plus the number of records already processed. It contains no
/// payloads, payload IDs, or process-local indices: a history rebuilt from the
/// same stored items in a different arrival order can interpret it.
///
/// A partial traversal captures **both** its target view and its delivery heads.
/// The target alone is insufficient: change traversals report newly delivered
/// commits even outside that view, and new items may arrive before resuming.
/// Resume re-plans this fixed selection in canonical order and skips the saved
/// record count. The count is of source change records, not subsequent payload
/// requests or suppressed events.
///
/// All named heads must exist on resume: persist items in the sedimentree store
/// before materializing them. A checkpoint is metadata, not a copy of history;
/// it cannot make an unsigned draft durable. Parsing checks structure only;
/// the planner must validate heads and progress against the rebuilt history.
///
/// Serialized size is proportional to heads, permissions, and rejected commits.
/// Rejection reasons are not saved. Collection lengths are checked against the
/// input size before allocation, and unknown versions/tags are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checkpoint {
    pub(crate) follow: Follow,
    pub(crate) cursor: MaterializationCursor,
    pub(crate) rejected: BTreeMap<CommitHash, bool>,
    pub(crate) in_progress: Option<InProgress>,
}

/// A fixed traversal, independent of delivery order and later history growth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InProgress {
    pub target: ViewPoint,
    pub delivery_heads: Vec<CommitHash>,
    pub applied: u64,
}

impl Checkpoint {
    /// Empty state, following `follow`, before any delivery.
    pub fn new(follow: Follow) -> Self {
        Self {
            follow,
            ..Self::default()
        }
    }

    pub fn follow(&self) -> &Follow {
        &self.follow
    }

    /// Heads of the last **completed** view, not a claim that a partial state
    /// already reflects the entire target. Check [`is_partial`](Self::is_partial)
    /// before using these to label a checkout.
    pub fn heads(&self) -> &[CommitHash] {
        self.cursor.view_point().heads()
    }

    pub fn is_partial(&self) -> bool {
        self.in_progress.is_some()
    }

    /// Rejected commits and whether each taints its descendants, in hash order.
    pub fn rejected(&self) -> impl ExactSizeIterator<Item = (CommitHash, bool)> + '_ {
        self.rejected
            .iter()
            .map(|(&hash, &cascade)| (hash, cascade))
    }

    /// A deterministic, versioned encoding. Permission maps and rejected sets
    /// are encoded in sorted key order, independent of map insertion order.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        match &self.follow {
            Follow::Working => out.push(0),
            Follow::Signed => out.push(1),
            Follow::Pinned(heads) => {
                out.push(2);
                encode_hashes(&mut out, heads);
            }
        }
        let cursor = self.cursor.to_bytes();
        out.extend((cursor.len() as u64).to_le_bytes());
        out.extend(cursor);
        out.extend((self.rejected.len() as u64).to_le_bytes());
        for (hash, cascade) in &self.rejected {
            out.extend(hash.as_bytes());
            out.push(u8::from(*cascade));
        }
        match &self.in_progress {
            None => out.push(0),
            Some(progress) => {
                out.push(1);
                progress.target.encode_into(&mut out);
                encode_hashes(&mut out, &progress.delivery_heads);
                out.extend(progress.applied.to_le_bytes());
            }
        }
        out
    }

    /// Decode a checkpoint without consulting a history. Invalid/truncated
    /// collections, duplicate keys, trailing bytes, and unknown versions or
    /// tags are errors. Semantic validation happens when the planner resumes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::decode(bytes).map_err(|_| Error::Malformed("materialization checkpoint"))
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        if r.take(MAGIC.len())? != MAGIC {
            return Err(Error::Malformed("checkpoint version"));
        }
        let follow = match r.take(1)?[0] {
            0 => Follow::Working,
            1 => Follow::Signed,
            2 => Follow::Pinned(r.hashes()?),
            _ => return Err(Error::Malformed("checkpoint follow rule")),
        };
        let cursor_len = r.count(1)?;
        let cursor = MaterializationCursor::from_bytes(r.take(cursor_len)?)?;
        let count = r.count(33)?;
        let mut rejected = BTreeMap::new();
        for _ in 0..count {
            let hash = r.hash()?;
            let cascade = read_bool(&mut r)?;
            if rejected.insert(hash, cascade).is_some() {
                return Err(Error::Malformed("duplicate rejected commit"));
            }
        }
        let in_progress = if read_bool(&mut r)? {
            Some(InProgress {
                target: ViewPoint::decode_from(&mut r)?,
                delivery_heads: r.hashes()?,
                applied: r.u64_le()?,
            })
        } else {
            None
        };
        r.finish()?;
        Ok(Self {
            follow,
            cursor,
            rejected,
            in_progress,
        })
    }
}

fn read_bool(r: &mut Reader<'_>) -> Result<bool, Error> {
    match r.take(1)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Malformed("checkpoint boolean")),
    }
}

#[cfg(test)]
mod tests;
