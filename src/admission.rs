//! Admission of received items: the queue, batch planning, and placement.
//!
//! Received items wait here, outside delivered graph history, until they can
//! be admitted. This module decides *what* to admit and in which order; the
//! graph only applies its decisions. It reads the graph through [`History`].
//!
//! Most items are admitted one at a time, once their dependencies and any
//! borrowed members are admitted. A fragment can also borrow authentication
//! from items which are themselves still queued, possibly waiting on the
//! borrower in turn: for example when the lender's boundary is one of the
//! borrower's members. Such items are admitted together as a *batch*, the
//! largest set of queued items which supply each other's missing dependencies
//! and authentication. Proofs therefore only ever come from items admitted no
//! later than the items relying on them, so every cover can authenticate
//! everything it advertises.
//!
//! Placement is validated before anything in a batch is inserted, against the
//! graph plus the batch's own metadata. Directly received items use the same
//! check with no extra metadata.

use alloc::{
    collections::{BTreeSet, VecDeque},
    vec::Vec,
};

use crate::{
    Author, CommitHash, CommitMeta, IngestError, IngestRejection, ItemId, VerifiedItem,
    collections::{HashMap, HashSet},
};

/// Read access to delivered graph history.
pub(crate) trait History {
    /// Delivered and authenticated, as opposed to an unsigned local draft.
    fn is_admitted(&self, hash: &CommitHash) -> bool;
    /// Metadata of a delivered commit, admitted or not.
    fn meta(&self, hash: &CommitHash) -> Option<&CommitMeta>;
    /// Membership in the inclusive ancestry of delivered heads.
    fn ancestry(&self, heads: &[CommitHash]) -> impl Fn(&CommitHash) -> bool + '_;
}

/// Undelivered metadata supplied by the other members of a batch.
pub(crate) type Overlay<'a> = HashMap<CommitHash, &'a CommitMeta>;

fn depth(hash: &CommitHash) -> usize {
    hash.as_bytes().iter().take_while(|b| **b == 0).count()
}

/// Check canonical fragment placement against delivered history plus any
/// overlay metadata. Stateless verification already proved member
/// reachability and depth and derived exact external parents; placement in
/// boundary history and the boundary stops reached through omitted metadata
/// depend on history outside the item.
pub(crate) fn validate_placement(
    item: &VerifiedItem,
    history: &impl History,
    overlay: &Overlay<'_>,
) -> Result<(), IngestError> {
    let ItemId::Fragment(range) = item.id() else {
        return Ok(());
    };
    let invalid = IngestError::InvalidFragment;
    let lookup = |hash: &CommitHash| overlay.get(hash).copied().or_else(|| history.meta(hash));

    // Inclusive boundary history: walk overlay metadata down to delivered
    // commits, then use the graph's ancestry for everything below them.
    let mut walked = HashSet::new();
    let mut delivered_heads = Vec::new();
    let mut stack: Vec<_> = range.boundary.iter().copied().collect();
    while let Some(hash) = stack.pop() {
        if let Some(meta) = overlay.get(&hash) {
            if walked.insert(hash) {
                stack.extend(meta.parents());
            }
        } else if history.meta(&hash).is_some() {
            delivered_heads.push(hash);
        } else {
            // Callers ensure dependencies are delivered or in the overlay.
            return Err(invalid);
        }
    }
    let in_delivered = history.ancestry(&delivered_heads);
    let in_excluded = |hash: &CommitHash| walked.contains(hash) || in_delivered(hash);

    if item.commits().any(|meta| in_excluded(meta.hash())) {
        return Err(invalid);
    }
    let mut stack = Vec::new();
    for hash in item.external_parents() {
        if !in_excluded(hash) {
            return Err(invalid);
        }
        stack.push(*hash);
    }
    let head_depth = depth(&range.head);
    let mut boundary = BTreeSet::new();
    let mut visited = HashSet::new();
    while let Some(hash) = stack.pop() {
        if !visited.insert(hash) {
            continue;
        }
        if depth(&hash) >= head_depth {
            boundary.insert(hash);
        } else {
            stack.extend(lookup(&hash).ok_or(invalid.clone())?.parents());
        }
    }
    if boundary != range.boundary {
        return Err(invalid);
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct PreparedItem<P> {
    pub item: VerifiedItem,
    // Aligned with item.commits, prepared synchronously by ingest's caller.
    // None means the member was already delivered. No callback is retained.
    pub payloads: Vec<Option<P>>,
}

/// What the graph should do next.
pub(crate) enum Step<P> {
    /// Admit these items in order. Each item's dependencies are admitted or
    /// members of an earlier item; placement has been validated. Items in a
    /// batch may rely on each other for authentication, so the graph must
    /// admit all of them before observing the result.
    Admit(Vec<PreparedItem<P>>),
    /// This item failed placement and has been removed from the queue.
    Reject(IngestRejection),
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
    // Dependencies and borrowed members which were unadmitted when queued.
    deps: Vec<CommitHash>,
    auth: Vec<CommitHash>,
    // Outstanding waits of either kind, for single-item admission.
    remaining: usize,
}

/// The received queue.
#[derive(Clone)]
pub(crate) struct Queue<P> {
    entries: HashMap<usize, Entry<P>>,
    keys: HashMap<Key, usize>,
    // Single-item admission: entries waiting for each hash to be admitted.
    // Members are never dependencies, so the two maps are disjoint per entry.
    waiting: HashMap<CommitHash, Vec<usize>>,
    waiting_auth: HashMap<CommitHash, Vec<usize>>,
    ready: VecDeque<usize>,
    // Batch planning: which queued entries contain, or authenticate, a commit.
    members: HashMap<CommitHash, Vec<usize>>,
    proofs: HashMap<CommitHash, Vec<usize>>,
    // Entries queued with borrowed members: only these can need a batch.
    borrowers: BTreeSet<usize>,
    // Whether anything changed since batch planning last found nothing.
    stale: bool,
    next_id: usize,
}

impl<P> Default for Queue<P> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            keys: HashMap::new(),
            waiting: HashMap::new(),
            waiting_auth: HashMap::new(),
            ready: VecDeque::new(),
            members: HashMap::new(),
            proofs: HashMap::new(),
            borrowers: BTreeSet::new(),
            stale: false,
            next_id: 0,
        }
    }
}

fn unlink(index: &mut HashMap<CommitHash, Vec<usize>>, hash: &CommitHash, id: usize) {
    if let Some(ids) = index.get_mut(hash) {
        ids.retain(|&other| other != id);
        if ids.is_empty() {
            index.remove(hash);
        }
    }
}

impl<P> Queue<P> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn missing_dependencies(&self) -> impl Iterator<Item = &CommitHash> {
        self.waiting.keys()
    }

    pub fn contains(&self, item: &VerifiedItem) -> bool {
        self.keys.contains_key(&Key::new(item))
    }

    // Called only after all payload callbacks succeed, with distinct unadmitted
    // dependencies and borrowed members, at least one of which is non-empty.
    // No partially prepared entry or waiter survives a panic.
    pub fn push(
        &mut self,
        prepared: PreparedItem<P>,
        deps: Vec<CommitHash>,
        auth: Vec<CommitHash>,
    ) {
        debug_assert!(!deps.is_empty() || !auth.is_empty());
        let key = Key::new(&prepared.item);
        debug_assert!(!self.keys.contains_key(&key));
        let id = self.next_id;
        self.next_id += 1;
        self.keys.insert(key.clone(), id);
        for hash in &deps {
            self.waiting.entry(*hash).or_default().push(id);
        }
        for hash in &auth {
            self.waiting_auth.entry(*hash).or_default().push(id);
        }
        for meta in prepared.item.commits() {
            self.members.entry(*meta.hash()).or_default().push(id);
        }
        for hash in prepared.item.authenticated() {
            self.proofs.entry(*hash).or_default().push(id);
        }
        if !auth.is_empty() {
            self.borrowers.insert(id);
        }
        self.entries.insert(
            id,
            Entry {
                key,
                prepared,
                remaining: deps.len() + auth.len(),
                deps,
                auth,
            },
        );
        self.stale = true;
    }

    // Each wait edge is visited once, even if a hash occurs in several items
    // or is later redelivered as a standalone commit.
    pub fn admitted(&mut self, hash: &CommitHash) {
        for map in [&mut self.waiting, &mut self.waiting_auth] {
            for id in map.remove(hash).into_iter().flatten() {
                let entry = self.entries.get_mut(&id).expect("waiting item exists");
                entry.remaining -= 1;
                if entry.remaining == 0 {
                    self.ready.push_back(id);
                }
            }
        }
        self.stale = true;
    }

    fn remove(&mut self, id: usize) -> Entry<P> {
        let entry = self.entries.remove(&id).expect("queued item exists");
        self.keys.remove(&entry.key);
        for hash in &entry.deps {
            unlink(&mut self.waiting, hash, id);
        }
        for hash in &entry.auth {
            unlink(&mut self.waiting_auth, hash, id);
        }
        for meta in entry.prepared.item.commits() {
            unlink(&mut self.members, meta.hash(), id);
        }
        for hash in entry.prepared.item.authenticated() {
            unlink(&mut self.proofs, hash, id);
        }
        self.borrowers.remove(&id);
        self.ready.retain(|&other| other != id);
        entry
    }

    /// Decide the next admission or rejection, if any.
    pub fn next(&mut self, history: &impl History) -> Option<Step<P>> {
        if let Some(id) = self.ready.pop_front() {
            let entry = self.remove(id);
            return Some(
                match validate_placement(&entry.prepared.item, history, &Overlay::new()) {
                    Ok(()) => Step::Admit(alloc::vec![entry.prepared]),
                    Err(error) => Step::Reject(IngestRejection {
                        item: entry.prepared.item.id().clone(),
                        error,
                    }),
                },
            );
        }
        if !self.stale {
            return None;
        }
        let step = self.plan_batch(history);
        // Nothing can change until the queue or admitted history does. A
        // rejection may leave a smaller batch, so plan again next time.
        self.stale = matches!(step, Some(Step::Reject(_)));
        step
    }

    // Whether every outstanding wait is admitted or provided within `set`.
    fn supported(&self, id: usize, set: &BTreeSet<usize>, history: &impl History) -> bool {
        let entry = &self.entries[&id];
        let provided = |index: &HashMap<CommitHash, Vec<usize>>, hash: &CommitHash| {
            history.is_admitted(hash)
                || index
                    .get(hash)
                    .is_some_and(|ids| ids.iter().any(|id| set.contains(id)))
        };
        entry.deps.iter().all(|hash| provided(&self.members, hash))
            && entry.auth.iter().all(|hash| provided(&self.proofs, hash))
    }

    // The largest set of queued items which supply each other's unadmitted
    // dependencies and borrowed members. Without borrowing, dependency edges
    // follow the acyclic commit graph, so single-item admission already finds
    // everything; batches are only needed around borrowers.
    fn plan_batch(&mut self, history: &impl History) -> Option<Step<P>> {
        let seeds: Vec<_> = self
            .borrowers
            .iter()
            .copied()
            .filter(|id| {
                self.entries[id]
                    .auth
                    .iter()
                    .any(|hash| !history.is_admitted(hash))
            })
            .collect();
        // Every queued item which could contribute, transitively.
        let mut set: BTreeSet<usize> = seeds.iter().copied().collect();
        let mut stack = seeds;
        while let Some(id) = stack.pop() {
            let entry = &self.entries[&id];
            let deps = entry.deps.iter().map(|hash| (&self.members, hash));
            let auth = entry.auth.iter().map(|hash| (&self.proofs, hash));
            for (index, hash) in deps.chain(auth) {
                if history.is_admitted(hash) {
                    continue;
                }
                for &provider in index.get(hash).into_iter().flatten() {
                    if set.insert(provider) {
                        stack.push(provider);
                    }
                }
            }
        }
        // Greatest fixed point: discard items lacking support until stable.
        loop {
            let unsupported: Vec<_> = set
                .iter()
                .copied()
                .filter(|&id| !self.supported(id, &set, history))
                .collect();
            if unsupported.is_empty() {
                break;
            }
            for id in unsupported {
                set.remove(&id);
            }
        }
        if set.is_empty() {
            return None;
        }

        // Validate everything before anything is inserted.
        let mut overlay = Overlay::new();
        for id in &set {
            for meta in self.entries[id].prepared.item.commits() {
                if history.meta(meta.hash()).is_none() {
                    overlay.insert(*meta.hash(), meta);
                }
            }
        }
        let invalid = set.iter().find_map(|&id| {
            validate_placement(&self.entries[&id].prepared.item, history, &overlay)
                .err()
                .map(|error| (id, error))
        });
        drop(overlay);
        if let Some((id, error)) = invalid {
            let entry = self.remove(id);
            return Some(Step::Reject(IngestRejection {
                item: entry.prepared.item.id().clone(),
                error,
            }));
        }

        // Admit in dependency order. Commit ancestry is acyclic, so an order
        // always exists; admitting only part of a batch would let an item
        // outlive the proof it relied on.
        let mut placed = HashSet::new();
        let mut order = Vec::with_capacity(set.len());
        while let Some(id) = set.iter().copied().find(|id| {
            self.entries[id]
                .deps
                .iter()
                .all(|hash| history.is_admitted(hash) || placed.contains(hash))
        }) {
            set.remove(&id);
            placed.extend(self.entries[&id].prepared.item.commits().map(|m| *m.hash()));
            order.push(id);
        }
        if !set.is_empty() {
            debug_assert!(false, "a supported batch must have a dependency order");
            return None;
        }
        Some(Step::Admit(
            order
                .into_iter()
                .map(|id| self.remove(id).prepared)
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlobHash, CommitContents, FragmentRange, Signature, SignatureEvidence, UnverifiedItem,
    };
    use alloc::collections::BTreeMap;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::{RngExt, SeedableRng, rngs::StdRng};

    // Delivered history held in a map: every commit is admitted.
    struct MapHistory(BTreeMap<CommitHash, CommitMeta>);

    fn ancestry(
        metas: &BTreeMap<CommitHash, CommitMeta>,
        heads: impl IntoIterator<Item = CommitHash>,
    ) -> BTreeSet<CommitHash> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<_> = heads.into_iter().collect();
        while let Some(hash) = stack.pop() {
            if seen.insert(hash) {
                stack.extend(metas[&hash].parents());
            }
        }
        seen
    }

    impl History for MapHistory {
        fn is_admitted(&self, hash: &CommitHash) -> bool {
            self.0.contains_key(hash)
        }
        fn meta(&self, hash: &CommitHash) -> Option<&CommitMeta> {
            self.0.get(hash)
        }
        fn ancestry(&self, heads: &[CommitHash]) -> impl Fn(&CommitHash) -> bool + '_ {
            let found = ancestry(&self.0, heads.iter().copied());
            move |hash| found.contains(hash)
        }
    }

    fn mined(key: &SigningKey, parents: Vec<CommitHash>, depth: usize) -> CommitMeta {
        let author = crate::Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
        (0u64..)
            .map(|nonce| {
                CommitMeta::new(CommitContents {
                    author,
                    parents: parents.clone(),
                    payload: BlobHash::of_contents(&nonce.to_le_bytes()),
                })
            })
            .find(|meta| super::depth(meta.hash()) == depth)
            .unwrap()
    }

    // The canonical range for a head, computed independently over all history.
    fn canonical(
        metas: &BTreeMap<CommitHash, CommitMeta>,
        head: CommitHash,
    ) -> (BTreeSet<CommitHash>, BTreeSet<CommitHash>) {
        let level = super::depth(&head);
        let mut boundary = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = alloc::vec![head];
        while let Some(hash) = stack.pop() {
            if !seen.insert(hash) {
                continue;
            }
            if hash != head && super::depth(&hash) >= level {
                boundary.insert(hash);
            } else {
                stack.extend(metas[&hash].parents());
            }
        }
        let excluded = ancestry(metas, boundary.iter().copied());
        let members = ancestry(metas, [head])
            .into_iter()
            .filter(|hash| !excluded.contains(hash))
            .collect();
        (boundary, members)
    }

    // The same verdict whether boundary history is delivered or supplied by
    // other members of a batch, for canonical ranges and for ranges claiming
    // a boundary which the parent walk does not reach.
    #[test]
    fn placement_agrees_across_delivered_and_overlay_history() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let author = crate::Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
        let mut checked = [0usize; 2];
        for seed in 0..24u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut metas = BTreeMap::new();
            let mut order = Vec::new();
            for index in 0..14 {
                let mut parents = Vec::new();
                for _ in 0..rng.random_range(if index == 0 { 0..=0 } else { 1..=2 }) {
                    let parent = order[rng.random_range(0..order.len())];
                    if !parents.contains(&parent) {
                        parents.push(parent);
                    }
                }
                let depth = match rng.random_range(0..12) {
                    0 => 2,
                    1..=4 => 1,
                    _ => 0,
                };
                let meta = mined(&key, parents, depth);
                order.push(*meta.hash());
                metas.insert(*meta.hash(), meta);
            }
            for &head in order.iter().filter(|hash| super::depth(hash) > 0) {
                let (boundary, members) = canonical(&metas, head);
                let excluded = ancestry(&metas, boundary.iter().copied());
                // Unreached deeper ancestors make plausible but wrong boundaries.
                let mut claims = alloc::vec![(boundary.clone(), true)];
                for extra in excluded.iter().filter(|hash| {
                    !boundary.contains(*hash) && super::depth(hash) >= super::depth(&head)
                }) {
                    let mut wrong = boundary.clone();
                    wrong.insert(*extra);
                    claims.push((wrong, false));
                }
                for (claim, valid) in claims {
                    let item = UnverifiedItem::Fragment {
                        range: FragmentRange {
                            head,
                            boundary: claim,
                        },
                        commits: members.iter().map(|hash| metas[hash].clone()).collect(),
                        signatures: alloc::vec![SignatureEvidence {
                            author,
                            commit: head,
                            signature: Signature::from(key.sign(head.as_bytes()).to_bytes()),
                        }],
                        borrowed: BTreeSet::new(),
                    }
                    .verify()
                    .expect("stateless checks cannot see boundary history");
                    // Deliver random ancestor-closed parts of the remaining
                    // history; the rest arrives as batch metadata.
                    for _ in 0..4 {
                        let picks: Vec<_> = order
                            .iter()
                            .copied()
                            .filter(|hash| !members.contains(hash) && rng.random_bool(0.5))
                            .collect();
                        // Commits with no member in their ancestry form an
                        // ancestor-closed set, as delivered history must be.
                        let delivered: BTreeSet<_> = ancestry(&metas, picks)
                            .into_iter()
                            .filter(|hash| ancestry(&metas, [*hash]).is_disjoint(&members))
                            .collect();
                        let history = MapHistory(
                            delivered
                                .iter()
                                .map(|hash| (*hash, metas[hash].clone()))
                                .collect(),
                        );
                        let overlay: Overlay<'_> = metas
                            .iter()
                            .filter(|(hash, _)| {
                                !delivered.contains(*hash) && !members.contains(*hash)
                            })
                            .map(|(hash, meta)| (*hash, meta))
                            .collect();
                        assert_eq!(
                            validate_placement(&item, &history, &overlay).is_ok(),
                            valid,
                            "seed={seed} head={head:?} delivered={}",
                            delivered.len()
                        );
                        checked[usize::from(valid)] += 1;
                    }
                }
            }
        }
        assert!(checked.iter().all(|&count| count > 20), "{checked:?}");
    }
}
