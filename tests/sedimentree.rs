//! Public-API properties for the non-minimal structural summary.
//! The oracle knows only envelopes, arrival/local flags, and signature events;
//! it never asks the graph for delivery, ancestry, depth, or fragment membership.
mod support;

use std::{
    collections::{BTreeSet, HashMap},
    sync::OnceLock,
};

use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
use stockfoil::{
    Author, BlobHash, Checkpoint, CommitContents, CommitGraph, CommitHash, CommitMeta, Fragment,
    FragmentRange, LooseCommit, SedimentreeSummary,
};
use support::{author, key, sign};

// Raise the case count for longer randomized runs; a failing seed can be rerun
// alone with SEDIMENTREE_TEST_SEED=<seed> SEDIMENTREE_TEST_CASES=1.
fn seeds() -> impl Iterator<Item = u64> {
    let first = std::env::var("SEDIMENTREE_TEST_SEED")
        .map(|value| {
            if let Some(hex) = value.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).expect("invalid test seed")
            } else {
                value.parse().expect("invalid test seed")
            }
        })
        .unwrap_or(23);
    let count = std::env::var("SEDIMENTREE_TEST_CASES")
        .map(|value| value.parse::<u64>().expect("invalid case count"))
        .unwrap_or(8);
    assert!(count > 0, "at least one randomized case is required");
    (0..count).map(move |case| first.wrapping_add(case))
}

fn hash(meta: &CommitMeta) -> CommitHash {
    *meta.hash()
}

fn depth(hash: CommitHash) -> usize {
    hash.as_bytes()
        .iter()
        .take_while(|byte| **byte == 0)
        .count()
}

// Mine real commit hashes, including parent and author fields. Only three
// depth-2 hashes per case; all test schedules share the resulting fixtures.
fn mine(parents: Vec<CommitHash>, target: usize, seed: u64, node: usize) -> CommitMeta {
    let author = author(&key(1));
    for nonce in 0..2_000_000u64 {
        let meta = CommitMeta::new(CommitContents {
            parents: parents.clone(),
            author,
            payload: BlobHash::of_contents(format!("sedimentree:{seed}:{node}:{nonce}").as_bytes()),
        });
        if depth(hash(&meta)) == target {
            return meta;
        }
    }
    panic!("mining exhausted: seed={seed:#x}, node={node}, depth={target}");
}

struct Case {
    seed: u64,
    commits: Vec<CommitMeta>,
}

fn cases() -> &'static [Case] {
    static CASES: OnceLock<Vec<Case>> = OnceLock::new();
    CASES.get_or_init(|| {
        seeds()
            .map(|seed| {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut commits = Vec::new();
                for node in 0..40 {
                    // A guaranteed adversarial backbone, with randomized parent
                    // ordering and later extensions. Sparse hash depths must not
                    // leave nested skips or >=-depth stops to chance.
                    let (target, mut parents): (usize, Vec<usize>) = match node {
                        0 | 1 => (0, vec![]), // unrelated shallow roots
                        2 => (1, vec![]),     // a nonzero root, later an interior checkpoint
                        3 => (0, vec![0]),
                        4 => (1, vec![3]),
                        5 => (1, vec![3, 1]),
                        6 => (1, vec![4, 5, 3]), // diamond plus redundant ancestor edge
                        7 => (0, vec![6, 2]),
                        8 => (2, vec![7]), // contains ALL earlier depth-1 fragments
                        9 => (0, vec![8]),
                        10 => (1, vec![9]), // must stop at a strictly deeper boundary
                        11 => (0, vec![4]), // fork from an old, now nested checkpoint
                        12 => (1, vec![11, 1]),
                        13 => (2, vec![10, 12]), // equal-depth stop at 8; checkpoint 10
                        14 => (1, vec![13, 0]),  // boundary and root reached independently
                        15 => (0, vec![0]),
                        16 => (1, vec![15]),
                        17 => (0, vec![16]),
                        18 => (1, vec![17, 0]), // shortcut behind a boundary, leaving a hole
                        39 => {
                            // Close the randomized portion too, exercising skip
                            // links through its arbitrary forks and merges.
                            let parents: BTreeSet<_> = commits
                                .iter()
                                .flat_map(|meta: &CommitMeta| meta.parents().iter().copied())
                                .collect();
                            let tips = commits
                                .iter()
                                .enumerate()
                                .filter_map(|(idx, meta)| {
                                    (!parents.contains(meta.hash())).then_some(idx)
                                })
                                .collect();
                            (2, tips)
                        }
                        _ => {
                            let count = rng.random_range(0..=4);
                            let mut parents: Vec<_> = (0..node).collect();
                            parents.shuffle(&mut rng);
                            parents.truncate(count);
                            (usize::from(rng.random_range(0..3) == 0), parents)
                        }
                    };
                    parents.shuffle(&mut rng);
                    let parents = parents.into_iter().map(|i| hash(&commits[i])).collect();
                    commits.push(mine(parents, target, seed, node));
                }
                Case { seed, commits }
            })
            .collect()
    })
}

#[derive(Clone, Default)]
struct Model {
    arrived: BTreeSet<CommitHash>,
    local: BTreeSet<CommitHash>,
    signatures: BTreeSet<(Author, CommitHash)>,
}

impl Model {
    fn ingest(&mut self, meta: &CommitMeta, local: bool) {
        // Duplicate received envelopes do not promote existing local work.
        if self.arrived.insert(hash(meta)) && local {
            self.local.insert(hash(meta));
        }
    }

    fn published(&self, commits: &[CommitMeta]) -> BTreeSet<CommitHash> {
        // Fixed point of received envelopes with all parents delivered: no use
        // of traversal APIs, insertion indices, or the production pending queue.
        let mut delivered = BTreeSet::new();
        loop {
            let before = delivered.len();
            for meta in commits {
                if self.arrived.contains(meta.hash())
                    && meta.parents().iter().all(|p| delivered.contains(p))
                {
                    delivered.insert(hash(meta));
                }
            }
            if delivered.len() == before {
                break;
            }
        }
        let known: HashMap<_, _> = commits.iter().map(|meta| (hash(meta), meta)).collect();
        let mut authenticated = BTreeSet::new();
        for (signer, tip) in &self.signatures {
            if !delivered.contains(tip) {
                continue;
            }
            let mut seen = BTreeSet::new();
            let mut stack = vec![*tip];
            while let Some(h) = stack.pop() {
                if seen.insert(h) {
                    let meta = known[&h];
                    if meta.author() == signer {
                        authenticated.insert(h);
                    }
                    stack.extend(meta.parents());
                }
            }
        }
        let mut published = BTreeSet::new();
        loop {
            let before = published.len();
            for meta in commits {
                let h = hash(meta);
                if delivered.contains(&h)
                    && (!self.local.contains(&h) || authenticated.contains(&h))
                    && meta.parents().iter().all(|p| published.contains(p))
                {
                    published.insert(h);
                }
            }
            if published.len() == before {
                return published;
            }
        }
    }
}

// Intentionally a plain parent walk for EVERY publishable nonzero head. There
// is no fragment reuse, covering index, subset reduction, or production helper.
fn oracle(commits: &[CommitMeta], published: &BTreeSet<CommitHash>) -> SedimentreeSummary {
    let known: HashMap<_, _> = commits.iter().map(|meta| (hash(meta), meta)).collect();
    let mut covered = BTreeSet::new();
    let mut fragments = Vec::new();
    for &head in published {
        let head_depth = depth(head);
        if head_depth == 0 {
            continue;
        }
        let mut members = BTreeSet::new();
        let mut boundary = BTreeSet::new();
        let mut checkpoints = BTreeSet::new();
        let mut stack = vec![head];
        while let Some(h) = stack.pop() {
            assert!(published.contains(&h), "oracle input must be parent-closed");
            if !members.insert(h) {
                continue;
            }
            if h != head {
                if depth(h) >= head_depth {
                    boundary.insert(h);
                    continue;
                }
                if depth(h) != 0 {
                    checkpoints.insert(Checkpoint(h.as_bytes()[..12].try_into().unwrap()));
                }
            }
            // Shallow roots naturally end the walk without becoming boundaries.
            stack.extend(known[&h].parents());
        }
        covered.extend(members);
        fragments.push(Fragment {
            range: FragmentRange { head, boundary },
            checkpoints,
        });
    }
    let mut loose_commits: Vec<_> = published
        .difference(&covered)
        .map(|h| LooseCommit {
            commit: *h,
            parents: known[h].parents().iter().copied().collect(),
        })
        .collect();
    loose_commits.sort_by_key(|commit| commit.commit);
    fragments.sort_by(|a, b| a.range.cmp(&b.range));
    SedimentreeSummary {
        loose_commits,
        fragments,
    }
}

fn check(graph: &CommitGraph<()>, model: &Model, case: &Case, event: &str) {
    let published = model.published(&case.commits);
    assert_eq!(
        graph.as_sedimentree().summary(),
        oracle(&case.commits, &published),
        "seed={:#x}, {event}, arrived={}, published={}",
        case.seed,
        model.arrived.len(),
        published.len(),
    );
}

#[test]
fn every_arrival_matches_the_walk_in_causal_reverse_and_shuffled_orders() {
    for case in cases() {
        let mut rng = StdRng::seed_from_u64(case.seed ^ 0xa771_a15);
        // Causal, reverse, and three independent permutations of the same DAG.
        for schedule in 0..5 {
            let mut order: Vec<_> = (0..case.commits.len()).collect();
            match schedule {
                0 => (),
                1 => order.reverse(),
                _ => order.shuffle(&mut rng),
            }
            let mut graph = CommitGraph::new();
            let mut model = Model::default();
            check(&graph, &model, case, "empty");
            for (step, node) in order.into_iter().enumerate() {
                let meta = &case.commits[node];
                graph.ingest_commit(meta.clone(), ());
                model.ingest(meta, false);
                let event = format!("schedule={schedule}, step={step}, node={node}");
                check(&graph, &model, case, &event);
                // Both queued and delivered duplicates are exercised, including
                // parent arrivals that release multiple queued descendants.
                let snapshot = graph.as_sedimentree().summary();
                graph.ingest_commit(meta.clone(), ());
                check(&graph, &model, case, &format!("{event}, duplicate"));
                assert_eq!(graph.as_sedimentree().summary(), snapshot, "{event}");
            }
            let all = case.commits.iter().map(hash).collect();
            assert_eq!(model.published(&case.commits), all, "seed={:#x}", case.seed);
        }
    }
}

#[test]
fn partitioned_arrivals_clone_and_merge_converge_in_both_directions() {
    for case in cases() {
        let mut rng = StdRng::seed_from_u64(case.seed ^ 0xc10e_0e42);
        for round in 0..3 {
            let mut graphs = [CommitGraph::new(), CommitGraph::new()];
            let mut models = [Model::default(), Model::default()];
            let mut order: Vec<_> = (0..case.commits.len()).collect();
            order.shuffle(&mut rng);
            for (step, node) in order.into_iter().enumerate() {
                // Every envelope reaches at least one replica; some overlap.
                let mask = rng.random_range(1..=3);
                for replica in 0..2 {
                    if mask & (1 << replica) != 0 {
                        graphs[replica].ingest_commit(case.commits[node].clone(), ());
                        models[replica].ingest(&case.commits[node], false);
                        check(
                            &graphs[replica],
                            &models[replica],
                            case,
                            &format!("round={round}, step={step}, node={node}, replica={replica}"),
                        );
                    }
                }
            }
            let snapshots = graphs.each_ref().map(|g| g.as_sedimentree().summary());
            let mut left = graphs[0].clone();
            let mut right = graphs[1].clone();
            check(
                &left,
                &models[0],
                case,
                &format!("round={round}, clone left"),
            );
            check(
                &right,
                &models[1],
                case,
                &format!("round={round}, clone right"),
            );
            // All inputs here are received, so merge exchanges even detached
            // queued envelopes. Model their union, not the graph's delivery API.
            let union = Model {
                arrived: models[0]
                    .arrived
                    .union(&models[1].arrived)
                    .copied()
                    .collect(),
                ..Model::default()
            };
            left.merge(&graphs[1]);
            right.merge(&graphs[0]);
            check(&left, &union, case, &format!("round={round}, left merge"));
            check(&right, &union, case, &format!("round={round}, right merge"));
            left.merge(&right);
            right.merge(&left);
            check(
                &left,
                &union,
                case,
                &format!("round={round}, repeated left merge"),
            );
            check(
                &right,
                &union,
                case,
                &format!("round={round}, repeated right merge"),
            );
            for replica in 0..2 {
                check(
                    &graphs[replica],
                    &models[replica],
                    case,
                    &format!("round={round}, original replica={replica}"),
                );
                assert_eq!(
                    graphs[replica].as_sedimentree().summary(),
                    snapshots[replica],
                    "seed={:#x}, round={round}, clone must be independent",
                    case.seed
                );
            }
        }
    }
}

fn add_signature(graph: &mut CommitGraph<()>, model: &mut Model, signer: u8, meta: &CommitMeta) {
    let key = key(signer);
    sign(graph, &key, hash(meta));
    model.signatures.insert((author(&key), hash(meta)));
}

#[test]
fn randomized_local_publication_and_signature_timing_match_the_walk() {
    for case in cases() {
        let mut rng = StdRng::seed_from_u64(case.seed ^ 0x519a_7e42);
        let mut graph = CommitGraph::new();
        let mut model = Model::default();
        // Explicit witness: 7 is a published depth-0 tail. The delivered but
        // unsigned depth-2 fragment at 8 must NOT remove it from loose commits;
        // received descendants 9 and 10 must also remain unpublished.
        for node in 0..=10 {
            let local = node == 8;
            let meta = &case.commits[node];
            if local {
                graph.ingest_local_commit(meta.clone(), ());
            } else {
                graph.ingest_commit(meta.clone(), ());
            }
            model.ingest(meta, local);
            check(
                &graph,
                &model,
                case,
                &format!("publication prefix node={node}"),
            );
        }
        assert!(
            graph
                .as_sedimentree()
                .summary()
                .loose_commits
                .iter()
                .any(|commit| commit.commit == hash(&case.commits[7])),
            "seed={:#x}",
            case.seed
        );
        let withheld = graph.as_sedimentree().summary();
        add_signature(&mut graph, &mut model, 2, &case.commits[10]);
        check(&graph, &model, case, "wrong author signature");
        assert_eq!(
            graph.as_sedimentree().summary(),
            withheld,
            "seed={:#x}",
            case.seed
        );
        // A signature targeting a not-yet-delivered descendant cannot cover
        // anything yet, but must become usable when its causal queue drains.
        add_signature(&mut graph, &mut model, 1, &case.commits[13]);
        check(&graph, &model, case, "signature before target arrival");
        assert_eq!(
            graph.as_sedimentree().summary(),
            withheld,
            "seed={:#x}",
            case.seed
        );

        let mut order: Vec<_> = (11..case.commits.len()).collect();
        order.shuffle(&mut rng);
        for (step, node) in order.into_iter().enumerate() {
            let local = rng.random_bool(0.6);
            let meta = &case.commits[node];
            if local {
                graph.ingest_local_commit(meta.clone(), ());
            } else {
                graph.ingest_commit(meta.clone(), ());
            }
            model.ingest(meta, local);
            let event = format!("publication step={step}, node={node}, local={local}");
            check(&graph, &model, case, &event);
            graph.ingest_commit(meta.clone(), ());
            check(
                &graph,
                &model,
                case,
                &format!("{event}, received duplicate"),
            );
            graph.ingest_local_commit(meta.clone(), ());
            check(&graph, &model, case, &format!("{event}, local duplicate"));
            if rng.random_bool(0.5) {
                let target = rng.random_range(0..case.commits.len());
                let signer = if rng.random_bool(0.75) { 1 } else { 2 };
                add_signature(&mut graph, &mut model, signer, &case.commits[target]);
                check(
                    &graph,
                    &model,
                    case,
                    &format!("{event}, signature signer={signer}, target={target}"),
                );
            }
        }
        let frozen = graph.clone();
        let frozen_model = model.clone();
        let mut signing_order: Vec<_> = (0..case.commits.len()).collect();
        signing_order.shuffle(&mut rng);
        for node in signing_order {
            add_signature(&mut graph, &mut model, 1, &case.commits[node]);
            check(&graph, &model, case, &format!("final signing node={node}"));
            check(
                &frozen,
                &frozen_model,
                case,
                &format!("frozen clone after signing node={node}"),
            );
        }
        assert_eq!(
            model.published(&case.commits).len(),
            case.commits.len(),
            "seed={:#x}, signatures must eventually publish all history",
            case.seed
        );
        let mut received = CommitGraph::new();
        received.merge(&graph);
        check(
            &received,
            &model,
            case,
            "fully published history exported by merge",
        );
    }
}
