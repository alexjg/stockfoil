//! Randomized comparison of history queries against naive whole-history models.
use ed25519_dalek::{Signer, SigningKey};
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::collections::{BTreeSet, HashMap, HashSet};
use stockfoil_core::*;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn author(key: &SigningKey) -> Author {
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

fn sign(key: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(key.sign(hash.as_bytes()).to_bytes())
}

struct Model {
    order: Vec<CommitHash>,
    parents: HashMap<CommitHash, Vec<CommitHash>>,
    authors: HashMap<CommitHash, Author>,
}

impl Model {
    fn ancestry(&self, heads: &[CommitHash]) -> HashSet<CommitHash> {
        let mut seen = HashSet::new();
        let mut stack: Vec<_> = heads
            .iter()
            .filter(|h| self.parents.contains_key(*h))
            .copied()
            .collect();
        while let Some(h) = stack.pop() {
            if seen.insert(h) {
                stack.extend(&self.parents[&h]);
            }
        }
        seen
    }

    fn participation(&self, view: &ViewPoint) -> HashSet<CommitHash> {
        let history = self.ancestry(view.heads());
        let mut allowed = HashMap::new();
        for (author, permission) in view.write_permissions() {
            let set = match permission {
                WritePermission::Unbounded => None,
                WritePermission::Denied => Some(HashSet::new()),
                WritePermission::Bounded(frontier) => {
                    let available: Vec<_> = frontier
                        .iter()
                        .filter(|h| history.contains(*h))
                        .copied()
                        .collect();
                    Some(self.ancestry(&available))
                }
            };
            allowed.insert(*author, set);
        }
        history
            .into_iter()
            .filter(|h| match allowed.get(&self.authors[h]) {
                Some(None) => true,
                Some(Some(set)) => set.contains(h),
                None => false,
            })
            .collect()
    }

    fn heads(&self, of: impl Fn(&CommitHash) -> bool) -> BTreeSet<CommitHash> {
        let members: Vec<_> = self.order.iter().filter(|h| of(h)).copied().collect();
        let mut heads: BTreeSet<_> = members.iter().copied().collect();
        for h in &members {
            for p in &self.parents[h] {
                heads.remove(p);
            }
        }
        heads
    }
}

fn random_history(seed: u64, size: usize) -> (History, Model, Vec<ed25519_dalek::SigningKey>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let keys: Vec<_> = (1..=3).map(key).collect();
    let mut history = History::new();
    let mut model = Model {
        order: vec![],
        parents: HashMap::new(),
        authors: HashMap::new(),
    };
    for i in 0..size {
        let k = &keys[rng.random_range(0..keys.len())];
        let mut parents = vec![];
        if !model.order.is_empty() {
            for _ in 0..rng.random_range(0..=3usize) {
                // Bias toward recent commits, like real histories.
                let back = rng.random_range(0..model.order.len().min(8));
                parents.push(model.order[model.order.len() - 1 - back]);
            }
        }
        let mut builder = history.begin_commit(author(k), parents.clone()).unwrap();
        builder.update(format!("{seed}-{i}").as_bytes());
        let prepared = builder.finish();
        let hash = prepared.hash;
        history.publish_commit(prepared, i as PayloadId).unwrap();
        model.order.push(hash);
        model.parents.insert(hash, parents);
        model.authors.insert(hash, author(k));
        if rng.random_bool(0.7) {
            history.sign(author(k), hash, sign(k, &hash)).unwrap();
        }
    }
    (history, model, keys)
}

fn random_view(rng: &mut StdRng, model: &Model, keys: &[ed25519_dalek::SigningKey]) -> ViewPoint {
    let heads: Vec<_> = (0..rng.random_range(0..4usize))
        .map(|_| model.order[rng.random_range(0..model.order.len())])
        .collect();
    let mut permissions = HashMap::new();
    for k in keys {
        let permission = match rng.random_range(0..4u8) {
            0 => continue,
            1 => WritePermission::Denied,
            2 => WritePermission::Unbounded,
            _ => WritePermission::Bounded(
                (0..rng.random_range(0..3usize))
                    .map(|_| model.order[rng.random_range(0..model.order.len())])
                    .collect(),
            ),
        };
        permissions.insert(author(k), permission);
    }
    ViewPoint::from_parts(heads, permissions)
}

#[test]
fn topology_queries_match_the_model() {
    for seed in 0..20 {
        let (history, model, _) = random_history(seed, 120);
        let mut rng = StdRng::seed_from_u64(seed + 1000);
        let delivered: BTreeSet<_> = history.heads(Heads::Delivered).collect();
        assert_eq!(delivered, model.heads(|_| true));
        let admitted: BTreeSet<_> = history.heads(Heads::Admitted).collect();
        assert_eq!(admitted, model.heads(|h| history.is_admitted(h)));
        // Admission is parent-closed.
        for h in &model.order {
            if history.is_admitted(h) {
                assert!(model.parents[h].iter().all(|p| history.is_admitted(p)));
            }
        }
        for _ in 0..300 {
            let a = model.order[rng.random_range(0..model.order.len())];
            let b = model.order[rng.random_range(0..model.order.len())];
            let expected = a != b && model.ancestry(&[b]).contains(&a);
            assert_eq!(history.is_ancestor(&a, &b), expected, "seed {seed}");
        }
    }
}

fn assert_topological(model: &Model, order: &[CommitHash]) {
    let position: HashMap<_, _> = order.iter().enumerate().map(|(i, h)| (*h, i)).collect();
    for (i, h) in order.iter().enumerate() {
        for a in model.ancestry(&[*h]) {
            if let Some(&j) = position.get(&a) {
                assert!(j <= i, "ancestor emitted after descendant");
            }
        }
    }
}

#[test]
fn change_batches_match_the_model() {
    for seed in 0..20 {
        let (history, model, keys) = random_history(seed, 100);
        let mut rng = StdRng::seed_from_u64(seed + 2000);
        // A sequence of traversals from one evolving cursor, plus fresh ones.
        let mut cursor = MaterializationCursor::new();
        for step in 0..15 {
            if rng.random_bool(0.2) {
                cursor = MaterializationCursor::new();
            }
            let target = random_view(&mut rng, &model, &keys);
            let delivered_before = model.ancestry(cursor.delivery_heads());
            let before = model.participation(cursor.view_point());
            let after = model.participation(&target);
            let mut changes = history.changes(&cursor, &target).unwrap();
            let mut records = vec![];
            let batch = rng.random_range(1..10usize);
            let next = loop {
                match history
                    .next_changes(&mut changes, &mut records, batch)
                    .unwrap()
                {
                    Batch::More => {}
                    Batch::Complete(next) => break next,
                }
            };
            let expected: BTreeSet<_> = model
                .order
                .iter()
                .filter_map(|h| {
                    let newly = !delivered_before.contains(h);
                    let vis = match (before.contains(h), after.contains(h)) {
                        (true, false) => Vis::Before,
                        (false, true) => Vis::After,
                        (true, true) => Vis::Both,
                        (false, false) => Vis::Neither,
                    };
                    (newly || matches!(vis, Vis::Before | Vis::After))
                        .then(|| (*h, newly, format!("{vis:?}")))
                })
                .collect();
            let got: BTreeSet<_> = records
                .iter()
                .map(|c| (c.hash, c.newly_delivered, format!("{:?}", c.visibility)))
                .collect();
            assert_eq!(got, expected, "seed {seed} step {step}");
            assert_eq!(got.len(), records.len(), "each commit at most once");
            assert_topological(&model, &records.iter().map(|c| c.hash).collect::<Vec<_>>());
            for c in &records {
                assert_eq!(c.payload, history.lookup(&c.hash).unwrap().payload);
            }
            assert_eq!(next.view_point(), &target);
            // The cursor round-trips through its durable encoding.
            assert_eq!(
                MaterializationCursor::from_bytes(&next.to_bytes()).unwrap(),
                next
            );
            cursor = next;
        }
    }
}

#[test]
fn views_require_delivered_heads() {
    let (history, _, _) = random_history(1, 5);
    let unknown = CommitHash::from([3; 32]);
    let view = ViewPoint::from_parts(vec![unknown], HashMap::new());
    assert_eq!(
        history
            .changes(&MaterializationCursor::new(), &view)
            .unwrap_err(),
        Error::UnknownCommit(unknown)
    );
}
