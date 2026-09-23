//! Public-API properties for summaries, canonical fragment exports, and borrowed reads.
//! The oracle knows only envelopes, arrival/local flags, and signature events;
//! it never asks the graph for delivery, ancestry, depth, or fragment membership.
mod support;

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
    sync::OnceLock,
};

use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
use stockfoil::{
    Author, BlobHash, Checkpoint, CommitContents, CommitGraph, CommitHash, CommitMeta, Fragment,
    FragmentRange, FragmentReadError, LooseCommit, SedimentreeSummary,
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
    mine_as(parents, target, seed, node, author(&key(1)))
}

fn mine_as(
    parents: Vec<CommitHash>,
    target: usize,
    seed: u64,
    node: usize,
    author: Author,
) -> CommitMeta {
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
                        11 => (0, vec![9]),
                        12 => (0, vec![11]),
                        // No depth-1 checkpoint between depth-2 heads 13 and 8:
                        // 13 must close its own level-1 fragment over a loose chain.
                        13 => (2, vec![12]),
                        14 => (1, vec![13, 0]), // boundary and root reached independently
                        15 => (0, vec![0]),
                        16 => (1, vec![15]),
                        17 => (0, vec![16]),
                        18 => (1, vec![17, 0]), // shortcut behind a boundary, leaving a hole
                        19 => (0, vec![4]),     // fork from an old, now nested checkpoint
                        20 => (1, vec![19, 1]),
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
// Keep full-hash membership alongside the unchanged canonical summary oracle;
// reduction must not infer membership from truncated checkpoint metadata.
fn oracle(
    commits: &[CommitMeta],
    published: &BTreeSet<CommitHash>,
) -> (
    SedimentreeSummary,
    HashMap<CommitHash, BTreeSet<CommitHash>>,
) {
    let known: HashMap<_, _> = commits.iter().map(|meta| (hash(meta), meta)).collect();
    let mut covered = BTreeSet::new();
    let mut fragments = Vec::new();
    let mut memberships = HashMap::new();
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
        covered.extend(members.iter().copied());
        memberships.insert(head, members);
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
    (
        SedimentreeSummary {
            loose_commits,
            fragments,
        },
        memberships,
    )
}

// Independent lexicographically smallest topological order: repeatedly scan
// remaining membership for the smallest commit with no remaining parent. No
// indegrees, adjacency index, ready queue, or production traversal are used.
fn export_order(commits: &[CommitMeta], members: &BTreeSet<CommitHash>) -> Vec<CommitHash> {
    let known: HashMap<_, _> = commits.iter().map(|meta| (hash(meta), meta)).collect();
    let mut remaining = members.clone();
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .copied()
            .filter(|h| known[h].parents().iter().all(|p| !remaining.contains(p)))
            .min()
            .expect("fixture must be acyclic");
        remaining.remove(&next);
        result.push(next);
    }
    result
}

fn assert_meta(actual: &CommitMeta, expected: &CommitMeta) {
    assert_eq!(actual.hash(), expected.hash());
    assert_eq!(actual.parents(), expected.parents());
    assert_eq!(actual.author(), expected.author());
    assert_eq!(actual.payload_hash(), expected.payload_hash());
}

fn minimized_oracle(
    canonical: &SedimentreeSummary,
    memberships: &HashMap<CommitHash, BTreeSet<CommitHash>>,
) -> SedimentreeSummary {
    let mut minimized = canonical.clone();
    minimized.fragments.retain(|fragment| {
        let head = fragment.range.head;
        let head_depth = depth(head);
        assert!(head_depth > 0);
        // Strict depth excludes both the other head and its inclusive >=-depth
        // boundaries, so this is precisely nonzero interior membership. In
        // particular, equal-depth set containment must NOT remove a fragment.
        !memberships
            .iter()
            .any(|(&other_head, members)| head_depth < depth(other_head) && members.contains(&head))
    });
    minimized
}

fn summaries(graph: &CommitGraph<CommitHash>) -> (SedimentreeSummary, SedimentreeSummary) {
    let view = graph.as_sedimentree();
    (view.summary(), view.minimized_summary())
}

fn check(graph: &CommitGraph<CommitHash>, model: &Model, case: &Case, event: &str) {
    let published = model.published(&case.commits);
    let (expected, memberships) = oracle(&case.commits, &published);
    let (canonical, minimized) = summaries(graph);
    let known: HashMap<_, _> = case.commits.iter().map(|meta| (hash(meta), meta)).collect();
    for meta in &case.commits {
        match graph.publishable_commit(meta.hash()) {
            Some((actual, payload)) => {
                assert!(
                    published.contains(meta.hash()),
                    "seed={:#x}, {event}",
                    case.seed
                );
                assert_meta(actual, meta);
                assert_eq!(payload, meta.hash(), "seed={:#x}, {event}", case.seed);
            }
            None => assert!(
                !published.contains(meta.hash()),
                "seed={:#x}, {event}",
                case.seed
            ),
        }
    }
    // Use oracle ranges, not ranges obtained from the implementation. Every
    // signature/arrival/clone/merge check also verifies every public export.
    for fragment in &expected.fragments {
        let mut actual = Vec::new();
        graph
            .traverse_fragment(&fragment.range, |meta, payload| {
                assert_meta(meta, known[meta.hash()]);
                assert_eq!(payload, meta.hash(), "seed={:#x}, {event}", case.seed);
                let (read_meta, read_payload) = graph.publishable_commit(meta.hash()).unwrap();
                assert_meta(read_meta, meta);
                assert!(std::ptr::eq(payload, read_payload));
                actual.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap_or_else(|err| panic!("seed={:#x}, {event}: {err:?}", case.seed));
        assert_eq!(
            actual,
            export_order(&case.commits, &memberships[&fragment.range.head]),
            "seed={:#x}, {event}, head={:?}",
            case.seed,
            fragment.range.head,
        );
    }
    assert_eq!(
        canonical,
        expected,
        "seed={:#x}, {event}, arrived={}, published={}",
        case.seed,
        model.arrived.len(),
        published.len(),
    );
    assert_eq!(
        minimized,
        minimized_oracle(&expected, &memberships),
        "seed={:#x}, {event}, minimized, arrived={}, published={}",
        case.seed,
        model.arrived.len(),
        published.len(),
    );
    assert_eq!(
        minimized.loose_commits, canonical.loose_commits,
        "seed={:#x}, {event}, minimization must preserve loose commits",
        case.seed,
    );
}

#[test]
fn backbone_retains_equal_depth_containment_and_closes_a_checkpoint_free_loose_chain() {
    for case in cases() {
        let mut graph = CommitGraph::new();
        let mut model = Model::default();
        for node in 0..=13 {
            let meta = &case.commits[node];
            graph.ingest_commit(meta.clone(), hash(meta));
            model.ingest(meta, false);
            check(&graph, &model, case, &format!("backbone node={node}"));
            let (canonical, minimized) = summaries(&graph);
            if node == 6 {
                let smaller = hash(&case.commits[4]);
                let larger = hash(&case.commits[6]);
                let (_, memberships) = oracle(&case.commits, &model.published(&case.commits));
                assert_eq!(depth(smaller), depth(larger));
                assert!(
                    memberships[&smaller].is_subset(&memberships[&larger]),
                    "seed={:#x}, backbone must witness equal-depth containment",
                    case.seed,
                );
                // The redundant edge to 3 reaches 0 behind boundary 4. Thus
                // fragment 4 is wholly contained in 6 but neither is removable.
                for summary in [&canonical, &minimized] {
                    for head in [smaller, larger] {
                        assert!(
                            summary.fragments.iter().any(|f| f.range.head == head),
                            "seed={:#x}, equal-depth contained fragments must survive",
                            case.seed,
                        );
                    }
                }
            }
            if node == 8 {
                let nested = hash(&case.commits[6]);
                assert!(canonical.fragments.iter().any(|f| f.range.head == nested));
                assert!(
                    !minimized.fragments.iter().any(|f| f.range.head == nested),
                    "seed={:#x}, a strictly shallower interior head must be removed",
                    case.seed,
                );
            }
            if node == 13 {
                let head = hash(&case.commits[13]);
                let boundary = hash(&case.commits[8]);
                assert_eq!(depth(head), 2);
                assert_eq!(depth(boundary), 2);
                let (_, memberships) = oracle(&case.commits, &model.published(&case.commits));
                assert_eq!(
                    memberships[&head],
                    [8, 9, 11, 12, 13].map(|i| hash(&case.commits[i])).into(),
                    "seed={:#x}, depth-2 head must close a checkpoint-free loose chain",
                    case.seed,
                );
                for i in [9, 11, 12] {
                    assert_eq!(depth(hash(&case.commits[i])), 0);
                }
                // Observe the same-head level-1 closure only through the public
                // maximum-depth fragment, without inspecting internal records.
                for summary in [&canonical, &minimized] {
                    let fragment = summary
                        .fragments
                        .iter()
                        .find(|f| f.range.head == head)
                        .unwrap();
                    assert_eq!(fragment.range.boundary, [boundary].into());
                    assert!(fragment.checkpoints.is_empty());
                    assert!(summary.fragments.iter().any(|f| f.range.head == boundary));
                }
            }
        }
    }
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
                graph.ingest_commit(meta.clone(), hash(meta));
                model.ingest(meta, false);
                let event = format!("schedule={schedule}, step={step}, node={node}");
                check(&graph, &model, case, &event);
                // Both queued and delivered duplicates are exercised, including
                // parent arrivals that release multiple queued descendants.
                let snapshot = summaries(&graph);
                graph.ingest_commit(meta.clone(), hash(meta));
                check(&graph, &model, case, &format!("{event}, duplicate"));
                assert_eq!(summaries(&graph), snapshot, "{event}");
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
                        graphs[replica]
                            .ingest_commit(case.commits[node].clone(), hash(&case.commits[node]));
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
            let snapshots = graphs.each_ref().map(summaries);
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
                    summaries(&graphs[replica]),
                    snapshots[replica],
                    "seed={:#x}, round={round}, clone must be independent",
                    case.seed
                );
            }
        }
    }
}

fn add_signature(
    graph: &mut CommitGraph<CommitHash>,
    model: &mut Model,
    signer: u8,
    meta: &CommitMeta,
) {
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
                graph.ingest_local_commit(meta.clone(), hash(meta));
            } else {
                graph.ingest_commit(meta.clone(), hash(meta));
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
        let withheld = summaries(&graph);
        add_signature(&mut graph, &mut model, 2, &case.commits[10]);
        check(&graph, &model, case, "wrong author signature");
        assert_eq!(summaries(&graph), withheld, "seed={:#x}", case.seed);
        // A signature targeting a not-yet-delivered descendant cannot cover
        // anything yet, but must become usable when its causal queue drains.
        add_signature(&mut graph, &mut model, 1, &case.commits[13]);
        check(&graph, &model, case, "signature before target arrival");
        assert_eq!(summaries(&graph), withheld, "seed={:#x}", case.seed);

        let mut order: Vec<_> = (11..case.commits.len()).collect();
        order.shuffle(&mut rng);
        for (step, node) in order.into_iter().enumerate() {
            let local = rng.random_bool(0.6);
            let meta = &case.commits[node];
            if local {
                graph.ingest_local_commit(meta.clone(), hash(meta));
            } else {
                graph.ingest_commit(meta.clone(), hash(meta));
            }
            model.ingest(meta, local);
            let event = format!("publication step={step}, node={node}, local={local}");
            check(&graph, &model, case, &event);
            graph.ingest_commit(meta.clone(), hash(meta));
            check(
                &graph,
                &model,
                case,
                &format!("{event}, received duplicate"),
            );
            graph.ingest_local_commit(meta.clone(), hash(meta));
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

fn case_range(case: &Case, node: usize) -> FragmentRange {
    let all = case.commits.iter().map(hash).collect();
    oracle(&case.commits, &all)
        .0
        .fragments
        .into_iter()
        .find(|fragment| fragment.range.head == hash(&case.commits[node]))
        .unwrap()
        .range
}

fn assert_read_error<P: Clone>(
    graph: &CommitGraph<P>,
    range: &FragmentRange,
    expected: FragmentReadError,
) {
    // Returning Break must not let an invalid range evade preflight validation.
    for stop in [false, true] {
        let mut callbacks = 0;
        let result = graph.traverse_fragment(range, |_, _| {
            callbacks += 1;
            if stop {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(callbacks, 0, "invalid ranges must not expose a prefix");
        assert_eq!(result.as_ref().unwrap_err(), &expected);
    }
}

#[test]
fn fragment_reads_validate_heads_and_exact_max_level_boundaries_before_callbacks() {
    let case = &cases()[0];
    let mut graph = CommitGraph::new();
    let range = case_range(case, 6);
    let head = range.head;
    assert!(graph.publishable_commit(&head).is_none());
    assert_read_error(&graph, &range, FragmentReadError::MissingCommit(head));

    graph.ingest_commit(case.commits[6].clone(), head);
    assert!(graph.publishable_commit(&head).is_none());
    assert_read_error(&graph, &range, FragmentReadError::MissingCommit(head));
    for meta in &case.commits {
        graph.ingest_commit(meta.clone(), hash(meta));
    }
    let shallow = hash(&case.commits[0]);
    assert_read_error(
        &graph,
        &FragmentRange {
            head: shallow,
            boundary: BTreeSet::new(),
        },
        FragmentReadError::NotFragment(shallow),
    );
    assert!(graph.publishable_commit(&shallow).is_some());

    let unknown = CommitHash::from([0xff; 32]);
    assert!(case.commits.iter().all(|meta| hash(meta) != unknown));
    assert!(graph.publishable_commit(&unknown).is_none());
    assert_read_error(
        &graph,
        &FragmentRange {
            head: unknown,
            boundary: BTreeSet::new(),
        },
        FragmentReadError::MissingCommit(unknown),
    );
    let mut missing = range.clone();
    missing.boundary.remove(&hash(&case.commits[4]));
    assert_read_error(&graph, &missing, FragmentReadError::BoundaryMismatch(head));
    for extra in [
        unknown,
        head,
        hash(&case.commits[2]),
        hash(&case.commits[3]),
    ] {
        let mut extra_boundary = range.clone();
        extra_boundary.boundary.insert(extra);
        assert_read_error(
            &graph,
            &extra_boundary,
            FragmentReadError::BoundaryMismatch(head),
        );
    }
    // Head 8 has depth 2, whose canonical boundary is empty. Its level-1
    // closure stops at 6 and 2 instead, and is not an accepted public range.
    let max_range = case_range(case, 8);
    assert!(max_range.boundary.is_empty());
    let lower_level = FragmentRange {
        head: max_range.head,
        boundary: [2, 6].map(|i| hash(&case.commits[i])).into(),
    };
    assert_read_error(
        &graph,
        &lower_level,
        FragmentReadError::BoundaryMismatch(max_range.head),
    );
    graph
        .traverse_fragment(&range, |_, _| ControlFlow::Continue(()))
        .unwrap();
    graph
        .traverse_fragment(&max_range, |_, _| ControlFlow::Continue(()))
        .unwrap();
}

#[test]
fn boundary_bypass_edges_order_parents_before_lower_hash_boundaries_and_break_is_global() {
    for case in cases() {
        let mut graph = CommitGraph::new();
        for meta in &case.commits {
            graph.ingest_commit(meta.clone(), hash(meta));
        }
        let range = case_range(case, 6);
        let mut actual = Vec::new();
        graph
            .traverse_fragment(&range, |meta, _| {
                actual.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        let parent = hash(&case.commits[3]);
        for node in [4, 5] {
            let boundary = hash(&case.commits[node]);
            assert!(range.boundary.contains(&boundary));
            assert!(
                boundary < parent,
                "fixture must defeat hash-only boundary scheduling"
            );
            assert!(
                actual.iter().position(|h| *h == parent).unwrap()
                    < actual.iter().position(|h| *h == boundary).unwrap()
            );
        }
        // Parent 1 of boundary 5 is outside the range: it is neither exported
        // nor a prerequisite, unlike bypass-reachable parent 3.
        assert!(!actual.contains(&hash(&case.commits[1])));
        assert_eq!(actual.len(), 5);
        for stop_after in 1..=actual.len() {
            let mut prefix = Vec::new();
            graph
                .traverse_fragment(&range, |meta, _| {
                    prefix.push(hash(meta));
                    if prefix.len() == stop_after {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                })
                .unwrap();
            assert_eq!(prefix, actual[..stop_after], "seed={:#x}", case.seed);
        }
    }
}

#[test]
fn local_fragment_reads_wait_for_signatures_before_or_after_arrival() {
    let case = &cases()[0];
    let range = case_range(case, 8);
    for signature_first in [false, true] {
        let mut graph = CommitGraph::new();
        if signature_first {
            sign(&mut graph, &key(1), range.head);
        }
        // Even a correctly signed queued head cannot be read.
        graph.ingest_local_commit(case.commits[8].clone(), range.head);
        assert!(graph.publishable_commit(&range.head).is_none());
        assert_read_error(&graph, &range, FragmentReadError::MissingCommit(range.head));
        for meta in &case.commits[..8] {
            graph.ingest_commit(meta.clone(), hash(meta));
        }
        if !signature_first {
            assert!(graph.publishable_commit(&range.head).is_none());
            assert_read_error(
                &graph,
                &range,
                FragmentReadError::NotPublishable(range.head),
            );
            // A received duplicate cannot launder local provenance.
            graph.ingest_commit(case.commits[8].clone(), range.head);
            sign(&mut graph, &key(2), range.head);
            assert_read_error(
                &graph,
                &range,
                FragmentReadError::NotPublishable(range.head),
            );
            sign(&mut graph, &key(1), range.head);
        }
        let (meta, payload) = graph.publishable_commit(&range.head).unwrap();
        assert_meta(meta, &case.commits[8]);
        assert_eq!(*payload, range.head);
        let mut seen = Vec::new();
        graph
            .traverse_fragment(&range, |meta, _| {
                seen.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        let all = case.commits.iter().map(hash).collect();
        let (_, memberships) = oracle(&case.commits, &all);
        assert_eq!(seen, export_order(&case.commits, &memberships[&range.head]));
    }
}

#[test]
fn signed_descendant_stays_withheld_behind_another_authors_local_ancestor() {
    let case = &cases()[0];
    let ancestor = &case.commits[8];
    let descendant = mine_as(vec![hash(ancestor)], 1, case.seed, 40, author(&key(2)));
    let range = FragmentRange {
        head: hash(&descendant),
        boundary: [hash(ancestor)].into(),
    };
    for signature_first in [false, true] {
        let mut graph = CommitGraph::new();
        for meta in &case.commits[..8] {
            graph.ingest_commit(meta.clone(), hash(meta));
        }
        graph.ingest_local_commit(ancestor.clone(), hash(ancestor));
        if signature_first {
            sign(&mut graph, &key(2), range.head);
        }
        graph.ingest_local_commit(descendant.clone(), hash(&descendant));
        if !signature_first {
            sign(&mut graph, &key(2), range.head);
        }
        assert!(graph.publishable_commit(ancestor.hash()).is_none());
        assert!(graph.publishable_commit(&range.head).is_none());
        assert_read_error(
            &graph,
            &range,
            FragmentReadError::NotPublishable(range.head),
        );
        sign(&mut graph, &key(1), hash(ancestor));
        let mut seen = Vec::new();
        graph
            .traverse_fragment(&range, |meta, payload| {
                assert_eq!(payload, meta.hash());
                seen.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(seen, [hash(ancestor), range.head]);
        assert_meta(
            graph.publishable_commit(&range.head).unwrap().0,
            &descendant,
        );
    }
}

#[test]
fn repeated_parent_edges_export_each_member_once() {
    let case = &cases()[0];
    let parents = [4, 4, 3, 3, 0, 0].map(|i| hash(&case.commits[i])).to_vec();
    let head = mine(parents, 1, case.seed, 41);
    let mut commits = case.commits[..5].to_vec();
    commits.push(head.clone());
    let all = commits.iter().map(hash).collect();
    let (summary, memberships) = oracle(&commits, &all);
    let range = &summary
        .fragments
        .iter()
        .find(|f| f.range.head == hash(&head))
        .unwrap()
        .range;
    assert_eq!(memberships[&hash(&head)].len(), 4);
    for reverse in [false, true] {
        let mut graph = CommitGraph::new();
        let mut order: Vec<_> = commits.iter().collect();
        if reverse {
            order.reverse();
        }
        for meta in order {
            graph.ingest_commit(meta.clone(), hash(meta));
        }
        let mut seen = Vec::new();
        graph
            .traverse_fragment(range, |meta, payload| {
                assert_eq!(payload, meta.hash());
                seen.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(seen, export_order(&commits, &memberships[&hash(&head)]));
    }
}

#[test]
fn fragment_and_covered_commit_reads_borrow_without_cloning_payloads() {
    struct NoClone(CommitHash);
    impl Clone for NoClone {
        fn clone(&self) -> Self {
            panic!("read APIs must not clone payloads")
        }
    }
    let case = &cases()[0];
    let mut graph = CommitGraph::new();
    // Reverse order exercises queue drainage with these payloads as well.
    for meta in case.commits[..7].iter().rev() {
        graph.ingest_commit(meta.clone(), NoClone(hash(meta)));
    }
    let read = |graph: &CommitGraph<NoClone>| {
        let range = case_range(case, 6);
        let all = case.commits.iter().map(hash).collect();
        let (_, memberships) = oracle(&case.commits, &all);
        let mut seen = Vec::new();
        let mut borrowed = Vec::new();
        graph
            .traverse_fragment(&range, |meta, payload| {
                borrowed.push((meta, payload));
                let expected = case
                    .commits
                    .iter()
                    .find(|m| m.hash() == meta.hash())
                    .unwrap();
                assert_meta(meta, expected);
                assert_eq!(payload.0, hash(meta));
                let (read_meta, read_payload) = graph.publishable_commit(meta.hash()).unwrap();
                assert_meta(read_meta, meta);
                assert!(std::ptr::eq(payload, read_payload));
                seen.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(seen, export_order(&case.commits, &memberships[&range.head]));
        // References can be retained after the callback, tied to the graph borrow.
        for (meta, payload) in borrowed {
            let (stored_meta, stored_payload) = graph.publishable_commit(meta.hash()).unwrap();
            assert!(std::ptr::eq(meta, stored_meta));
            assert!(std::ptr::eq(payload, stored_payload));
        }
        for meta in &case.commits[..7] {
            let (actual, payload) = graph.publishable_commit(meta.hash()).unwrap();
            assert_meta(actual, meta);
            assert_eq!(payload.0, hash(meta));
        }
    };
    read(&graph);
    for meta in &case.commits[7..=8] {
        graph.ingest_commit(meta.clone(), NoClone(hash(meta)));
    }
    // The fragment at 6 is now nested inside 8 and omitted by minimization;
    // neither its range nor any covered commit loses read access.
    assert!(
        !graph
            .as_sedimentree()
            .minimized_summary()
            .fragments
            .iter()
            .any(|f| f.range.head == hash(&case.commits[6]))
    );
    read(&graph);
}
