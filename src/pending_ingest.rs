//! Received items wait here, outside delivered graph history.
use alloc::{collections::VecDeque, vec::Vec};
use crate::collections::{HashMap, HashSet};

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
    borrowed: Vec<CommitHash>,
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
            borrowed: item.borrowed().iter().copied().collect(),
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
    // Items waiting for a dependency to be admitted.
    waiting: HashMap<CommitHash, Vec<usize>>,
    // Items waiting for a borrowed member to be authenticated by another item
    // (or admitted). Members are never dependencies, so the maps are disjoint.
    waiting_auth: HashMap<CommitHash, Vec<usize>>,
    // Unadmitted commits authenticated by some verified item we have queued,
    // even one later rejected for placement: authorship is a stateless fact
    // about that item's signatures and hash-bound metadata. Admitted commits
    // are removed; the graph answers for them. Grows with rejected items.
    authenticated: HashSet<CommitHash>,
    ready: VecDeque<usize>,
    next_id: usize,
}

impl<P> Default for PendingIngest<P> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            keys: HashMap::new(),
            waiting: HashMap::new(),
            waiting_auth: HashMap::new(),
            authenticated: HashSet::new(),
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

    pub fn missing_authentication(&self) -> impl Iterator<Item = &CommitHash> {
        self.waiting_auth.keys()
    }

    pub fn is_authenticated(&self, hash: &CommitHash) -> bool {
        self.authenticated.contains(hash)
    }

    pub fn has_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    pub fn contains(&self, item: &VerifiedItem) -> bool {
        self.keys.contains_key(&Key::new(item))
    }

    // Called only after all payload callbacks succeed, with distinct missing
    // dependencies and distinct unauthenticated borrowed members, at least one
    // of which is non-empty. No partially prepared entry or waiter survives a
    // panic. The caller records the item's own authentication separately.
    pub fn push(
        &mut self,
        prepared: PreparedItem<P>,
        missing: Vec<CommitHash>,
        missing_auth: Vec<CommitHash>,
    ) {
        debug_assert!(!missing.is_empty() || !missing_auth.is_empty());
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
                remaining: missing.len() + missing_auth.len(),
            },
        );
        for hash in missing {
            self.waiting.entry(hash).or_default().push(id);
        }
        for hash in missing_auth {
            self.waiting_auth.entry(hash).or_default().push(id);
        }
    }

    fn wake(&mut self, waiters: Option<Vec<usize>>) {
        for id in waiters.into_iter().flatten() {
            let entry = self.entries.get_mut(&id).expect("waiting item exists");
            entry.remaining -= 1;
            if entry.remaining == 0 {
                self.ready.push_back(id);
            }
        }
    }

    // Record that a verified item authenticates an unadmitted commit. Wakes
    // items borrowing it whether or not the authenticating item is admitted.
    pub fn authenticate(&mut self, hash: CommitHash) {
        if self.authenticated.insert(hash) {
            let waiters = self.waiting_auth.remove(&hash);
            self.wake(waiters);
        }
    }

    // Each dependency edge is visited once, even if a hash occurs in several
    // received certificates or is later redelivered as a standalone commit.
    // Admission also authenticates, so it satisfies both kinds of wait.
    pub fn admitted(&mut self, hash: &CommitHash) {
        self.authenticated.remove(hash);
        let waiters = self.waiting.remove(hash);
        self.wake(waiters);
        let waiters = self.waiting_auth.remove(hash);
        self.wake(waiters);
    }

    pub fn pop_ready(&mut self) -> Option<PreparedItem<P>> {
        let id = self.ready.pop_front()?;
        let entry = self.entries.remove(&id).expect("ready item exists");
        self.keys.remove(&entry.key);
        Some(entry.prepared)
    }
}
