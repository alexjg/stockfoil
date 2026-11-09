//! Randomized comparison of history queries against naive whole-history models.
use crate::*;
use ed25519_dalek::{Signer, SigningKey};
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::collections::{BTreeSet, HashMap, HashSet};

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
        history
            .iter()
            .filter(
                |h| match view.write_permissions().permission(&self.authors[*h]) {
                    WritePermission::Unbounded => true,
                    WritePermission::Denied => false,
                    WritePermission::Bounded(frontier) => {
                        let available: Vec<_> = frontier
                            .iter()
                            .filter(|f| history.contains(*f))
                            .copied()
                            .collect();
                        self.ancestry(&available).contains(*h)
                    }
                },
            )
            .copied()
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
    let random_permission = |rng: &mut StdRng| match rng.random_range(0..3u8) {
        0 => WritePermission::Denied,
        1 => WritePermission::Unbounded,
        _ => WritePermission::Bounded(
            (0..rng.random_range(0..3usize))
                .map(|_| model.order[rng.random_range(0..model.order.len())])
                .collect(),
        ),
    };
    // Some authors stay unlisted, so the unlisted rule is exercised too.
    let mut permissions = WritePermissions::new().with_unlisted(random_permission(rng));
    for k in keys {
        if rng.random_range(0..4u8) > 0 {
            permissions.insert(author(k), random_permission(rng));
        }
    }
    ViewPoint::from_parts(heads, permissions)
}

#[test]
fn stored_topology_matches_hash_graph() {
    for seed in 0..8 {
        let (history, model, _) = random_history(seed, 48);
        let mut end = 0;
        for (i, node) in history.topology.nodes().iter().enumerate() {
            assert_eq!(node.parents_start, end);
            end += node.parents_len as usize;
            let parents = &history.topology.parent_ids()[node.parents_start..end];
            assert!(parents.iter().all(|p| p.index() < i));
            let hashes: HashSet<_> = parents.iter().map(|p| history.node(*p).hash).collect();
            assert_eq!(hashes, model.parents[&node.hash].iter().copied().collect());
            assert_eq!(node.hash, model.order[i]);
            assert_eq!(history.topology.by_hash().delivered(&node.hash).index(), i);
            assert_eq!(node.payload, i as PayloadId);
            assert_eq!(
                history.topology.chain_authors()[node.chain as usize],
                model.authors[&node.hash]
            );
        }
        assert_eq!(end, history.topology.parent_ids().len());
        assert_eq!(
            history.topology.chain_tips().len(),
            history.topology.chain_nodes().len()
        );
        for (chain, row) in history.topology.chain_nodes().iter().enumerate() {
            let expected: Vec<_> = history
                .topology
                .nodes()
                .iter()
                .enumerate()
                .filter(|(_, node)| node.chain as usize == chain)
                .map(|(i, _)| super::NodeIdx::new(i as u32))
                .collect();
            assert_eq!(row, &expected);
            assert_eq!(row.last(), Some(&history.topology.chain_tips()[chain]));
            let owner = history.topology.chain_authors()[chain];
            let owned: Vec<_> = history
                .topology
                .chain_authors()
                .iter()
                .enumerate()
                .filter(|(_, author)| **author == owner)
                .map(|(c, _)| c as u32)
                .collect();
            assert_eq!(history.topology.author_chains().get(&owner), Some(&owned));
        }
    }
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

/// Deliberately naive: recompute readiness from whole-history ancestry, not
/// the implementation's chains or frontiers. Also checks ordering through
/// unchanged ancestors that are absent from `order`.
fn assert_canonical(model: &Model, order: &[CommitHash]) {
    let ancestors: HashMap<_, _> = order
        .iter()
        .map(|&hash| {
            let mut past = model.ancestry(&[hash]);
            past.remove(&hash);
            (hash, past)
        })
        .collect();
    let mut pending: BTreeSet<_> = order.iter().copied().collect();
    for &hash in order {
        let expected = pending
            .iter()
            .find(|h| ancestors[h].iter().all(|a| !pending.contains(a)))
            .copied()
            .expect("a DAG has a ready commit");
        assert_eq!(hash, expected, "smallest hash among ancestry-ready records");
        assert!(pending.remove(&hash));
    }
    assert!(pending.is_empty());
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
                    ChangeBatch::More => {}
                    ChangeBatch::Complete(next) => break next,
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
            assert_canonical(&model, &records.iter().map(|c| c.hash).collect::<Vec<_>>());
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
fn ancestry_queries_and_differences_match_the_model() {
    for seed in 0..20 {
        let (history, model, keys) = random_history(seed, 100);
        let mut rng = StdRng::seed_from_u64(seed + 3000);
        for _ in 0..20 {
            let before = random_view(&mut rng, &model, &keys);
            let after = random_view(&mut rng, &model, &keys);
            let b = history
                .ancestry(before.heads(), before.write_permissions())
                .unwrap();
            let a = history
                .ancestry(after.heads(), after.write_permissions())
                .unwrap();
            let before_set = model.ancestry(before.heads());
            let after_set = model.ancestry(after.heads());
            let active = model.participation(&after);
            for hash in &model.order {
                assert_eq!(
                    a.contains(&history, hash).unwrap(),
                    after_set.contains(hash)
                );
                assert_eq!(
                    a.participates(&history, hash).unwrap(),
                    active.contains(hash)
                );
                let ancestor_or_head = after
                    .heads()
                    .iter()
                    .any(|head| hash == head || history.is_ancestor(hash, head));
                assert_eq!(a.contains(&history, hash).unwrap(), ancestor_or_head);
            }
            let difference: Vec<_> = a.difference(&b, &history).unwrap().collect();
            assert_eq!(
                difference.iter().copied().collect::<HashSet<_>>(),
                after_set.difference(&before_set).copied().collect()
            );
            assert_canonical(&model, &difference);
            let since: Vec<_> = history.commits_since(before.heads()).unwrap().collect();
            assert_eq!(
                since.iter().copied().collect::<HashSet<_>>(),
                model
                    .order
                    .iter()
                    .filter(|h| !before_set.contains(h))
                    .copied()
                    .collect()
            );
            assert_canonical(&model, &since);
            let full = history
                .ancestry(
                    &history.heads(Heads::Delivered).collect::<Vec<_>>(),
                    &WritePermissions::new(),
                )
                .unwrap();
            assert_eq!(
                since,
                full.difference(&b, &history).unwrap().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn views_require_delivered_heads() {
    let (history, _, _) = random_history(1, 5);
    let unknown = CommitHash::from([3; 32]);
    let view = ViewPoint::from_parts(vec![unknown], WritePermissions::new());
    assert_eq!(
        history
            .changes(&MaterializationCursor::new(), &view)
            .unwrap_err(),
        Error::UnknownCommit(unknown)
    );
}

#[test]
fn zero_and_unlimited_batches_preserve_the_captured_history() {
    let (mut history, model, keys) = random_history(902, 6);
    // Missing permissions deny everything, but new deliveries must still appear.
    let target = history.view_point(&WritePermissions::new());
    let mut changes = history
        .changes(&MaterializationCursor::new(), &target)
        .unwrap();
    let mut records = vec![];
    assert_eq!(
        history.next_changes(&mut changes, &mut records, 0).unwrap(),
        ChangeBatch::More
    );
    assert_eq!(changes.remaining(), model.order.len());
    assert!(records.is_empty());

    let mut builder = history
        .begin_commit(author(&keys[0]), vec![model.order[5]])
        .unwrap();
    builder.update(b"delivered after the traversal was captured");
    let prepared = builder.finish();
    let later = prepared.hash;
    history.publish_commit(prepared, 99).unwrap();

    assert_eq!(
        history.next_changes(&mut changes, &mut records, 1).unwrap(),
        ChangeBatch::More
    );
    let ChangeBatch::Complete(cursor) = history
        .next_changes(&mut changes, &mut records, usize::MAX)
        .unwrap()
    else {
        panic!("unlimited batch must finish")
    };
    let order: Vec<_> = records.iter().map(|e| e.hash).collect();
    assert_eq!(order.len(), model.order.len());
    assert_eq!(
        order.iter().copied().collect::<BTreeSet<_>>(),
        model.order.iter().copied().collect::<BTreeSet<_>>()
    );
    assert_canonical(&model, &order);
    assert!(
        records
            .iter()
            .all(|e| e.newly_delivered && e.visibility == Vis::Neither)
    );
    let count = records.len();
    assert_eq!(
        history.next_changes(&mut changes, &mut records, 0).unwrap(),
        ChangeBatch::Complete(cursor.clone())
    );
    assert_eq!(records.len(), count);

    let target = history.view_point(&WritePermissions::new());
    let mut changes = history.changes(&cursor, &target).unwrap();
    records.clear();
    assert!(matches!(
        history
            .next_changes(&mut changes, &mut records, usize::MAX)
            .unwrap(),
        ChangeBatch::Complete(_)
    ));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].hash, later);
}
