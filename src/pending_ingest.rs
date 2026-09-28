//! Received items wait here, outside delivered graph history.
use std::collections::{HashMap, VecDeque};

use crate::{Author, CommitHash, ItemId, VerifiedItem};

#[derive(Clone)]
pub(crate) struct PreparedItem<P> {
    pub item: VerifiedItem,
    // Aligned with item.commits, prepared synchronously by ingest's caller.
    // None means the member was already delivered. No callback is retained.
    pub payloads: Vec<Option<P>>,
}

// ItemId alone is insufficient: different member sets or certificates may claim
// the same range. In particular, a malformed candidate must not suppress a valid
// one. Hashes bind complete metadata; retain all distinct signature evidence.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    id: ItemId,
    members: Vec<CommitHash>,
    signatures: Vec<(Author, CommitHash, [u8; 64])>,
}

impl Key {
    fn new(item: &VerifiedItem) -> Self {
        let mut signatures: Vec<_> = item
            .signatures
            .iter()
            .map(|s| (s.author, s.commit, s.signature.into()))
            .collect();
        signatures.sort_unstable();
        signatures.dedup();
        Self {
            id: item.id().clone(),
            // Verification gives members a canonical topological order.
            members: item.commits().map(|m| *m.hash()).collect(),
            signatures,
        }
    }
}

#[derive(Clone)]
struct Entry<P> {
    key: Key,
    prepared: PreparedItem<P>,
    remaining: usize,
}

#[derive(Clone)]
pub(crate) struct PendingIngest<P> {
    entries: HashMap<usize, Entry<P>>,
    keys: HashMap<Key, usize>,
    waiting: HashMap<CommitHash, Vec<usize>>,
    ready: VecDeque<usize>,
    next_id: usize,
}

impl<P> Default for PendingIngest<P> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            keys: HashMap::new(),
            waiting: HashMap::new(),
            ready: VecDeque::new(),
            next_id: 0,
        }
    }
}

impl<P> PendingIngest<P> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn missing_dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.waiting.keys()
    }

    pub fn contains(&self, item: &VerifiedItem) -> bool {
        self.keys.contains_key(&Key::new(item))
    }

    // Called only after all payload callbacks succeed, with distinct missing
    // dependencies. No partially prepared entry or waiter survives a panic.
    pub fn push(&mut self, prepared: PreparedItem<P>, missing: Vec<CommitHash>) {
        debug_assert!(!missing.is_empty());
        let key = Key::new(&prepared.item);
        debug_assert!(!self.keys.contains_key(&key));
        let id = self.next_id;
        self.next_id += 1;
        self.keys.insert(key.clone(), id);
        self.entries.insert(
            id,
            Entry {
                key,
                prepared,
                remaining: missing.len(),
            },
        );
        for hash in missing {
            self.waiting.entry(hash).or_default().push(id);
        }
    }

    // Each dependency edge is visited once, even if a hash occurs in several
    // received certificates or is later redelivered as a standalone commit.
    pub fn admitted(&mut self, hash: &CommitHash) {
        if let Some(waiters) = self.waiting.remove(hash) {
            for id in waiters {
                let entry = self.entries.get_mut(&id).expect("waiting item exists");
                entry.remaining -= 1;
                if entry.remaining == 0 {
                    self.ready.push_back(id);
                }
            }
        }
    }

    pub fn pop_ready(&mut self) -> Option<PreparedItem<P>> {
        let id = self.ready.pop_front()?;
        let entry = self.entries.remove(&id).expect("ready item exists");
        self.keys.remove(&entry.key);
        Some(entry.prepared)
    }
}
