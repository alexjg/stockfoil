//! Replicas exchanging export plans converge, whatever the order of edits and syncs.
mod support;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::collections::{BTreeSet, HashMap};
use stockfoil::*;
use support::*;

#[test]
fn sync_between_random_replicas_converges() {
    for seed in 0..5u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let keys: Vec<_> = (1..=3).map(key).collect();
        let mut replicas = [
            document_for(&keys[0]),
            document_for(&keys[1]),
            document_for(&keys[2]),
        ];
        let mut cursors: HashMap<(usize, usize), ExportCursor> = HashMap::new();
        for step in 0..60 {
            let r = rng.random_range(0..3usize);
            if rng.random_bool(0.6) {
                let k = &keys[r];
                let heads: Vec<_> = replicas[r].history().heads(Heads::Admitted).collect();
                let payload = if rng.random_bool(0.1) {
                    payload_at_depth(k, &heads, 1, &format!("{seed}-{step}"))
                } else {
                    format!("{seed}-{step}").into_bytes()
                };
                signed(&mut replicas[r], k, heads, &payload);
            } else {
                let to = (r + rng.random_range(1..3usize)) % 3;
                let cursor = cursors.remove(&(r, to)).unwrap_or_default();
                let (a, b) = pair(&mut replicas, r, to);
                cursors.insert((r, to), sync(a, b, &cursor));
            }
        }
        for _ in 0..3 {
            for from in 0..3 {
                for to in 0..3 {
                    if from != to {
                        let cursor = cursors.remove(&(from, to)).unwrap_or_default();
                        let (a, b) = pair(&mut replicas, from, to);
                        cursors.insert((from, to), sync(a, b, &cursor));
                    }
                }
            }
        }
        let sets: Vec<BTreeSet<_>> = replicas
            .iter()
            .map(|g| g.history().traverse().collect())
            .collect();
        assert_eq!(sets[0], sets[1]);
        assert_eq!(sets[1], sets[2]);
        for g in &replicas {
            assert!(g.history().traverse().all(|h| g.history().is_admitted(&h)));
        }
    }
}

fn pair(r: &mut [TestDocument; 3], a: usize, b: usize) -> (&mut TestDocument, &mut TestDocument) {
    assert_ne!(a, b);
    let (lo, hi) = r.split_at_mut(a.max(b));
    if a < b {
        (&mut lo[a], &mut hi[0])
    } else {
        (&mut hi[0], &mut lo[b])
    }
}
