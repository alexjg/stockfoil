//! Public-API properties for summaries, canonical fragment exports, and borrowed reads.
//! The oracle knows only accepted verified members, local edits, and signature
//! events; it never asks the graph for delivery, ancestry, depth, or membership.
//! Out-of-order transport verifies statelessly on arrival; certificates and
//! payloads then wait in the graph's received queue, separately from local edits.
mod support;

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
    sync::OnceLock,
};

use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
use stockfoil::{
    Author, BlobHash, Checkpoint, CommitContents, CommitGraph, CommitHash, CommitMeta, ExportCursor,
    ExportError, Fragment, FragmentRange, FragmentReadError, IngestError, ItemId,
    LooseCommit, SedimentreeSummary, SignatureEvidence, UnverifiedItem,
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
        .unwrap_or(2);
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

fn evidence(meta: &CommitMeta) -> SignatureEvidence {
    let key = (1..=255)
        .map(key)
        .find(|key| &author(key) == meta.author())
        .expect("fixture signing key");
    SignatureEvidence {
        author: author(&key),
        commit: hash(meta),
        signature: support::signature(&key, hash(meta)),
    }
}

fn envelope(meta: &CommitMeta) -> UnverifiedItem {
    UnverifiedItem::Commit {
        meta: meta.clone(),
        signatures: vec![evidence(meta)],
    }
}

// Nonzero heads travel as canonical certificates with normalized payloads,
// not as commit items. Sign only payload members, including mixed-author ones.
fn fixture_item(commits: &[CommitMeta], meta: &CommitMeta) -> UnverifiedItem {
    if depth(hash(meta)) == 0 {
        return envelope(meta);
    }
    let all = commits.iter().map(hash).collect();
    let (summary, memberships) = oracle(commits, &all);
    let range = summary
        .fragments
        .into_iter()
        .find(|f| f.range.head == hash(meta))
        .unwrap()
        .range;
    let members: Vec<_> = commits
        .iter()
        .filter(|m| memberships[&range.head].contains(m.hash()))
        .cloned()
        .collect();
    let signatures = members.iter().map(evidence).collect();
    UnverifiedItem::Fragment {
        range,
        checkpoints: BTreeSet::new(),
        commits: members,
        signatures,
    }
}

// The receiver only verifies and supplies payloads synchronously. Its metadata
// archive translates reported certificate IDs to oracle members; it neither
// retains payloads nor retries/schedules pending work on the graph's behalf.
struct Receiver<P> {
    commits: Vec<CommitMeta>,
    payload: fn(CommitHash) -> P,
    receipts: HashMap<ItemId, Vec<CommitMeta>>,
}

impl<P: Clone> Receiver<P> {
    fn new(commits: &[CommitMeta], payload: fn(CommitHash) -> P) -> Self {
        Self {
            commits: commits.to_vec(),
            payload,
            receipts: HashMap::new(),
        }
    }

    fn receive(
        &mut self,
        graph: &mut CommitGraph<P>,
        meta: &CommitMeta,
        payload: P,
    ) -> Vec<CommitMeta> {
        let verified = fixture_item(&self.commits, meta).verify().unwrap();
        self.receipts
            .insert(verified.id().clone(), verified.commits().cloned().collect());
        let mut payloads: HashMap<_, _> = verified
            .commits()
            .filter(|m| hash(m) != hash(meta))
            .map(|m| (hash(m), (self.payload)(hash(m))))
            .collect();
        payloads.insert(hash(meta), payload);
        let report = graph
            .ingest(verified, |meta| payloads.remove(meta.hash()).unwrap())
            .expect("canonical fixture must be accepted");
        assert_eq!(report.pending, graph.pending_items());
        assert!(
            report.rejected.is_empty(),
            "canonical fixture rejected: {:?}",
            report.rejected
        );
        report
            .admitted
            .iter()
            .flat_map(|id| {
                self.receipts
                    .get(id)
                    .expect("reported received certificate")
                    .iter()
                    .cloned()
            })
            .collect()
    }
}

#[derive(Clone, Default)]
struct Model {
    arrived: BTreeSet<CommitHash>,
    local: BTreeSet<CommitHash>,
    verified: BTreeSet<CommitHash>,
    signatures: BTreeSet<(Author, CommitHash)>,
}

impl Model {
    fn ingest(&mut self, meta: &CommitMeta, local: bool) {
        // Only call this after admission, never merely on network arrival.
        if self.arrived.insert(hash(meta)) && local {
            self.local.insert(hash(meta));
        }
        if !local {
            // Every test receipt carries its author's real signature. This
            // evidence can also authenticate earlier same-author local edits.
            self.verified.insert(hash(meta));
            self.signatures.insert((*meta.author(), hash(meta)));
        }
    }

    fn accepted(&mut self, commits: Vec<CommitMeta>) {
        for meta in commits {
            self.ingest(&meta, false);
        }
    }

    fn published(&self, commits: &[CommitMeta]) -> BTreeSet<CommitHash> {
        // Fixed point of admitted receipts and local edits with all parents
        // delivered: no traversal API, insertion index, or production queue.
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
        let mut published = BTreeSet::new();
        loop {
            let before = published.len();
            for meta in commits {
                let h = hash(meta);
                if !delivered.contains(&h) || published.contains(&h) {
                    continue;
                }
                if self.verified.contains(&h)
                    && meta.parents().iter().all(|p| published.contains(p))
                {
                    published.insert(h);
                    continue;
                }
                if !self.local.contains(&h) || !self.signatures.contains(&(*meta.author(), h)) {
                    continue;
                }
                let (members, dependencies) = if depth(h) == 0 {
                    (
                        BTreeSet::from([h]),
                        meta.parents().iter().copied().collect(),
                    )
                } else {
                    let (members, boundary) = normalized_walk(&known, h);
                    let mut dependencies = boundary;
                    dependencies.extend(
                        members
                            .iter()
                            .flat_map(|h| known[h].parents().iter().copied())
                            .filter(|p| !members.contains(p)),
                    );
                    (members, dependencies)
                };
                if !dependencies.is_subset(&published) {
                    continue;
                }
                let mut covered = dependencies;
                for (signer, target) in &self.signatures {
                    let mut seen = BTreeSet::new();
                    let mut stack = vec![*target];
                    while let Some(next) = stack.pop() {
                        if !members.contains(&next) || !seen.insert(next) {
                            continue;
                        }
                        if known[&next].author() == signer {
                            covered.insert(next);
                        }
                        stack.extend(known[&next].parents());
                    }
                }
                if members.is_subset(&covered) {
                    published.extend(members);
                }
            }
            if published.len() == before {
                return published;
            }
        }
    }
}

// Independent inclusive history, using only fixture parent metadata.
fn history(
    known: &HashMap<CommitHash, &CommitMeta>,
    heads: impl IntoIterator<Item = CommitHash>,
) -> BTreeSet<CommitHash> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<_> = heads.into_iter().collect();
    while let Some(h) = stack.pop() {
        if seen.insert(h) {
            stack.extend(known[&h].parents().iter().copied());
        }
    }
    seen
}

// Discover raw >=-depth stops first, retaining even redundant ancestor stops.
// Then normalize by subtracting their full histories from the head's history.
fn normalized_walk(
    known: &HashMap<CommitHash, &CommitMeta>,
    head: CommitHash,
) -> (BTreeSet<CommitHash>, BTreeSet<CommitHash>) {
    let mut seen = BTreeSet::new();
    let mut boundary = BTreeSet::new();
    let mut stack = vec![head];
    while let Some(h) = stack.pop() {
        if !seen.insert(h) {
            continue;
        }
        if h != head && depth(h) >= depth(head) {
            boundary.insert(h);
        } else {
            stack.extend(known[&h].parents().iter().copied());
        }
    }
    let excluded = history(known, boundary.iter().copied());
    let members = history(known, [head])
        .difference(&excluded)
        .copied()
        .collect();
    (members, boundary)
}

// Plain parent walks for EVERY publishable nonzero head: no fragment reuse,
// graph index, or production helper. Keep full hashes for subset reduction;
// truncated checkpoint metadata must never stand in for payload membership.
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
        assert!(
            history(&known, [head]).is_subset(published),
            "oracle input must be parent-closed"
        );
        let (members, boundary) = normalized_walk(&known, head);
        let checkpoints = members
            .iter()
            .filter(|&&h| h != head && depth(h) != 0)
            .map(|h| Checkpoint(h.as_bytes()[..12].try_into().unwrap()))
            .collect();
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
        // Head containment alone is insufficient after boundary normalization.
        // Retain equal-depth overlaps and any shallower fragment with a payload
        // or explicit boundary that the deeper candidate cannot account for.
        !canonical.fragments.iter().any(|other| {
            let members = &memberships[&other.range.head];
            head_depth < depth(other.range.head)
                && members.contains(&head)
                && memberships[&head].is_subset(members)
                && fragment
                    .range
                    .boundary
                    .iter()
                    .all(|b| members.contains(b) || other.range.boundary.contains(b))
        })
    });
    minimized
}

fn cover_ids<'a>(
    loose: &'a [LooseCommit],
    fragments: &'a [Fragment],
) -> impl Iterator<Item = ItemId> + 'a {
    loose
        .iter()
        .map(|c| ItemId::Commit(c.commit))
        .chain(fragments.iter().map(|f| ItemId::Fragment(f.range.clone())))
}

// Use only when the fixture establishes certificates for every surviving range.
fn assert_structural_cover(graph: &CommitGraph<CommitHash>, expected: SedimentreeSummary) {
    let cover = graph.minimal_cover();
    assert_eq!(cover.loose_commits, expected.loose_commits);
    assert_eq!(cover.fragments, expected.fragments);
}

// The cover must be parent-closed, verifiable, and mergeable into an empty peer.
fn check_direct_inventory(graph: &CommitGraph<CommitHash>, model: &Model, case: &Case) {
    let published = model.published(&case.commits);
    let (canonical, memberships) = oracle(&case.commits, &published);
    let inventory = graph.minimal_cover();
    let (changes, _) = graph.export_changes(&ExportCursor::default());
    assert!(changes.retired.is_empty());
    let mut additions: HashMap<_, _> = changes.added.into_iter()
        .map(|item| (item.id(), item))
        .collect();
    assert_eq!(additions.len(), inventory.loose_commits.len() + inventory.fragments.len());
    let mut covered = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut items = Vec::new();
    for id in cover_ids(&inventory.loose_commits, &inventory.fragments) {
        let expected = match &id {
            ItemId::Commit(h) => {
                assert_eq!(depth(*h), 0);
                BTreeSet::from([*h])
            }
            ItemId::Fragment(range) => memberships[&range.head].clone(),
        };
        let exported = graph.export(&id).unwrap();
        let added = additions.remove(&id).expect("same direct and incremental inventory");
        assert_eq!(exported.id(), added.id());
        if let ItemId::Fragment(range) = &id {
            let summary = inventory.fragments.iter().find(|f| &f.range == range).unwrap();
            let oracle = canonical.fragments.iter().find(|f| &f.range == range).unwrap();
            for item in [&exported, &added] {
                let UnverifiedItem::Fragment { checkpoints, .. } = item else {
                    panic!("fragment identity must have fragment contents")
                };
                assert_eq!(checkpoints, &summary.checkpoints);
                assert_eq!(checkpoints, &oracle.checkpoints);
                // BTreeSet iteration is strictly sorted and deduplicated.
                assert!(checkpoints.iter().zip(checkpoints.iter().skip(1)).all(|(a, b)| a < b));
            }
        }
        // Verification ignores reconciliation hints and authenticates full members.
        let verified = exported.verify().unwrap();
        assert_eq!(verified.id(), &id);
        let actual: Vec<_> = verified.commits().map(hash).collect();
        assert_eq!(actual, export_order(&case.commits, &expected));
        dependencies.extend(verified.dependencies().copied());
        covered.extend(actual);
        items.push(verified);
    }
    assert!(additions.is_empty());
    assert_eq!(
        covered, published,
        "the cover must relay every admitted node"
    );
    assert!(dependencies.is_subset(&covered));
    for meta in case.commits.iter().filter(|m| covered.contains(m.hash())) {
        assert!(meta.parents().iter().all(|p| covered.contains(p)));
    }
    let (canonical, memberships) = oracle(&case.commits, &covered);
    let minimized = minimized_oracle(&canonical, &memberships);
    // Structural ranges include interior checkpoints without their own export
    // certificates. Their canonical payloads can extend outside the item that
    // introduced them, so transport need not equal this structural minimum.
    assert_eq!(summaries(graph), (canonical, minimized));
    // Relay the actual inventory in arbitrary reverse order, without a
    // transport-side readiness scheduler. The graph must drain the closed cover.
    let certificates: HashMap<_, Vec<_>> = items
        .iter()
        .map(|item| (item.id().clone(), item.commits().map(hash).collect()))
        .collect();
    let mut destination = CommitGraph::new();
    let mut admitted = BTreeSet::new();
    for item in items.into_iter().rev() {
        let report = destination.ingest(item, hash).unwrap();
        assert!(report.rejected.is_empty());
        assert_eq!(report.pending, destination.pending_items());
        for id in report.admitted {
            admitted.extend(certificates[&id].iter().copied());
        }
    }
    assert_eq!(destination.pending_items(), 0);
    assert!(destination.missing_dependencies().next().is_none());
    assert_eq!(admitted, published);
    assert_eq!(destination.minimal_cover(), inventory);
    let mut relay = CommitGraph::new();
    relay.merge(&destination);
    assert_eq!(relay.minimal_cover(), inventory);
    let mut received = BTreeSet::new();
    relay.traverse(|meta, _| {
        assert!(relay.has_commit(meta.hash()));
        received.insert(hash(meta));
        ControlFlow::Continue(())
    });
    assert_eq!(
        received, covered,
        "seed={:#x}, closed cover merges from empty",
        case.seed
    );
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
    // signature/arrival/clone/merge check also verifies every borrowed fragment read.
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
fn backbone_normalizes_bypasses_and_closes_a_checkpoint_free_loose_chain() {
    for case in cases() {
        let mut graph = CommitGraph::new();
        let mut model = Model::default();
        for node in 0..=13 {
            let meta = &case.commits[node];
            support::receive_commit(&mut graph, meta.clone(), hash(meta));
            model.ingest(meta, false);
            check(&graph, &model, case, &format!("backbone node={node}"));
            let (canonical, minimized) = summaries(&graph);
            if node == 6 {
                let smaller = hash(&case.commits[4]);
                let larger = hash(&case.commits[6]);
                let (_, memberships) = oracle(&case.commits, &model.published(&case.commits));
                assert_eq!(depth(smaller), depth(larger));
                assert_eq!(memberships[&larger], BTreeSet::from([larger]));
                assert!(memberships[&smaller].is_disjoint(&memberships[&larger]));
                let sibling = hash(&case.commits[5]);
                assert!(!memberships[&smaller].is_disjoint(&memberships[&sibling]));
                assert!(minimized.fragments.iter().any(|f| f.range.head == sibling));
                // Equal-depth sibling payloads overlap, but both survive.
                // The redundant edge to 3 reaches 0 behind boundary 4, but
                // normalization removes both. Boundary 4 remains its own item.
                for summary in [&canonical, &minimized] {
                    for head in [smaller, larger] {
                        assert!(
                            summary.fragments.iter().any(|f| f.range.head == head),
                            "seed={:#x}, boundary fragments must survive separately",
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
                    [9, 11, 12, 13].map(|i| hash(&case.commits[i])).into(),
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
fn long_same_depth_checkpoint_chain_preserves_every_boundary_fragment() {
    const LENGTH: usize = 128;
    let seed = 0xc4a1_0128;
    let mut commits = Vec::with_capacity(LENGTH);
    for node in 0..LENGTH {
        let parents = commits.last().map(hash).into_iter().collect();
        commits.push(mine(parents, 1, seed, node));
    }
    let case = Case { seed, commits };
    let mut graph = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits {
        assert_eq!(depth(hash(meta)), 1);
        support::receive_commit(&mut graph, meta.clone(), hash(meta));
        model.ingest(meta, false);
    }

    // Many disjoint singleton covers exercise candidate indexing without mining
    // expensive deeper checkpoints or running the full oracle after each arrival.
    // Equal-depth ancestors are dependencies, never containment candidates.
    let mut expected = SedimentreeSummary {
        loose_commits: Vec::new(),
        fragments: case
            .commits
            .iter()
            .map(|meta| Fragment {
                range: FragmentRange {
                    head: hash(meta),
                    boundary: meta.parents().iter().copied().collect(),
                },
                checkpoints: BTreeSet::new(),
            })
            .collect(),
    };
    expected.fragments.sort_by(|a, b| a.range.cmp(&b.range));
    let (canonical, minimized) = summaries(&graph);
    assert_eq!(canonical, expected);
    assert_eq!(minimized, expected);
    assert_structural_cover(&graph, expected);

    for fragment in &canonical.fragments {
        let range = &fragment.range;
        let mut members = Vec::new();
        graph
            .traverse_fragment(range, |meta, payload| {
                assert_eq!(payload, meta.hash());
                members.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(members, [range.head]);
        let exported = graph.export(&ItemId::Fragment(range.clone())).unwrap();
        assert_eq!(
            exported
                .clone()
                .verify()
                .unwrap()
                .external_parents()
                .copied()
                .collect::<BTreeSet<_>>(),
            range.boundary
        );
        let UnverifiedItem::Fragment {
            range: exported_range,
            commits,
            checkpoints,
            ..
        } = exported
        else {
            panic!("checkpoint must export as a fragment")
        };
        assert_eq!(&exported_range, range);
        assert!(checkpoints.is_empty());
        assert_eq!(commits.iter().map(hash).collect::<Vec<_>>(), [range.head]);
    }
    // Verify the actual inventory and let an empty peer buffer unordered items,
    // then relay again. Every previous checkpoint must travel as its own item
    // rather than being swallowed by its child's payload.
    check_direct_inventory(&graph, &model, &case);
}

fn deeper_head_containment_case() -> Case {
    let seed = 0xdecaf;
    let root = mine(vec![], 0, seed, 0);
    let checkpoint = mine(vec![hash(&root)], 1, seed, 1);
    let boundary = mine(vec![hash(&checkpoint)], 2, seed, 2);
    let with_boundary = mine(vec![hash(&checkpoint)], 1, seed, 3);
    let with_root = mine(vec![hash(&root)], 1, seed, 4);
    let head = mine(
        vec![hash(&boundary), hash(&with_boundary), hash(&with_root)],
        2,
        seed,
        5,
    );
    Case {
        seed,
        commits: vec![root, checkpoint, boundary, with_boundary, with_root, head],
    }
}

#[test]
fn deeper_head_containment_does_not_hide_uncovered_payloads_or_boundaries() {
    let case = deeper_head_containment_case();
    let h = |i: usize| hash(&case.commits[i]);
    let all = case.commits.iter().map(hash).collect();
    let (canonical, memberships) = oracle(&case.commits, &all);
    assert_eq!(memberships[&h(5)], [h(3), h(4), h(5)].into());
    let fragment = canonical
        .fragments
        .iter()
        .find(|f| f.range.head == h(5))
        .unwrap();
    assert_eq!(fragment.range.boundary, [h(2)].into());
    assert_eq!(
        fragment.checkpoints,
        [3, 4]
            .map(|i| Checkpoint(h(i).as_bytes()[..12].try_into().unwrap()))
            .into()
    );
    // Both shallower heads are in the deeper payload. One has an uncovered
    // explicit boundary; the other has an uncovered payload ancestor.
    assert_eq!(memberships[&h(3)], [h(3)].into());
    assert_eq!(memberships[&h(4)], [h(0), h(4)].into());
    let expected = minimized_oracle(&canonical, &memberships);
    assert_eq!(
        expected
            .fragments
            .iter()
            .map(|f| f.range.head)
            .collect::<BTreeSet<_>>(),
        [h(2), h(3), h(4), h(5)].into()
    );
    let mut graph = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits {
        support::receive_commit(&mut graph, meta.clone(), hash(meta));
        model.ingest(meta, false);
        check(&graph, &model, &case, "partial deeper coverage");
    }
    let mut exported = graph.export(&ItemId::Fragment(fragment.range.clone())).unwrap();
    let UnverifiedItem::Fragment { checkpoints, .. } = &mut exported else {
        panic!("expected fragment")
    };
    // Checkpoint 1 is reachable through the bypass from 3 but belongs to
    // boundary 2's history. Root 0, boundary 2, and head 5 are excluded too.
    assert_eq!(checkpoints, &fragment.checkpoints);
    for i in [0, 1, 2, 5] {
        assert!(!checkpoints.contains(&Checkpoint(h(i).as_bytes()[..12].try_into().unwrap())));
    }
    // Reconciliation hints have no role in authentication.
    *checkpoints = BTreeSet::from([Checkpoint([255; 12])]);
    let item = exported.verify().unwrap();
    assert_eq!(
        item.dependencies().copied().collect::<BTreeSet<_>>(),
        [h(0), h(1), h(2)].into()
    );
    // All four surviving ranges were received with their own certificates.
    assert_structural_cover(&graph, expected);
    check_direct_inventory(&graph, &model, &case);
}

#[test]
fn received_interior_checkpoint_is_readable_but_cannot_export_an_excluded_ancestor() {
    let case = deeper_head_containment_case();
    let h = |i: usize| hash(&case.commits[i]);
    let mut graph = CommitGraph::new();
    let mut model = Model::default();
    // Admit the boundary's history, but never receive either interior child's
    // own fragment. Only the higher fragment introduces those checkpoints.
    for meta in &case.commits[..3] {
        support::receive_commit(&mut graph, meta.clone(), hash(meta));
        model.ingest(meta, false);
    }
    let higher = fixture_item(&case.commits, &case.commits[5])
        .verify()
        .unwrap();
    assert_eq!(
        higher.commits().map(hash).collect::<BTreeSet<_>>(),
        [h(3), h(4), h(5)].into()
    );
    assert_eq!(
        higher.dependencies().copied().collect::<BTreeSet<_>>(),
        [h(0), h(1), h(2)].into()
    );
    let accepted = higher.commits().cloned().collect();
    graph.ingest(higher, hash).unwrap();
    model.accepted(accepted);
    check(
        &graph,
        &model,
        &case,
        "higher fragment introduces uncertified interiors",
    );

    let child = case_range(&case, 4);
    assert!(child.boundary.is_empty());
    let mut read = Vec::new();
    graph
        .traverse_fragment(&child, |meta, payload| {
            assert_eq!(payload, meta.hash());
            read.push(hash(meta));
            ControlFlow::Continue(())
        })
        .unwrap();
    // The child's canonical payload includes root, which the higher fragment
    // excluded via its boundary. Readability must not manufacture a certificate.
    assert_eq!(read, [h(0), h(4)]);
    let structural = summaries(&graph).1;
    assert_eq!(
        structural
            .fragments
            .iter()
            .map(|f| f.range.head)
            .collect::<BTreeSet<_>>(),
        [h(2), h(3), h(4), h(5)].into()
    );
    let inventory = graph.minimal_cover();
    assert!(inventory.loose_commits.is_empty());
    assert_eq!(
        inventory
            .fragments
            .iter()
            .map(|f| f.range.clone())
            .collect::<BTreeSet<_>>(),
        [case_range(&case, 2), case_range(&case, 5)].into()
    );
    for node in [3, 4] {
        let id = ItemId::Fragment(case_range(&case, node));
        assert_eq!(
            graph.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    check_direct_inventory(&graph, &model, &case);

    let mut relay = CommitGraph::new();
    relay.merge(&graph);
    check(
        &relay,
        &model,
        &case,
        "uncertified interior stays readable after relay",
    );
    assert_eq!(relay.minimal_cover(), inventory);
    let id = ItemId::Fragment(child);
    assert_eq!(
        relay.export(&id).unwrap_err(),
        ExportError::NotInMinimalCover(id)
    );
    check_direct_inventory(&relay, &model, &case);
}

#[test]
fn fragment_interiors_release_buffered_received_children() {
    let case = &cases()[0];
    // Head 8's fragment contains checkpoint 4 as an interior member. Neither
    // the interior's own certificate nor a retry of its child should be needed.
    let child = &case.commits[19];
    assert_eq!(child.parents(), &[hash(&case.commits[4])]);
    let child_id = ItemId::Commit(hash(child));
    for trigger in ["receive", "sign", "merge"] {
        let mut graph = CommitGraph::new();
        let report = graph
            .ingest(envelope(child).verify().unwrap(), hash)
            .unwrap();
        assert!(report.admitted.is_empty());
        assert!(report.rejected.is_empty());
        assert_eq!(report.pending, 1);
        assert!(graph.heads().next().is_none());
        assert!(graph.publishable_commit(child.hash()).is_none());
        let higher = fixture_item(&case.commits, &case.commits[8])
            .verify()
            .unwrap();
        assert!(higher.dependencies().next().is_none());
        let higher_id = higher.id().clone();
        let report = match trigger {
            "receive" => graph.ingest(higher, hash).unwrap(),
            "sign" => {
                for meta in &case.commits[..=8] {
                    let report = graph.ingest_local_commit(meta.clone(), hash(meta));
                    assert!(report.admitted.is_empty());
                    assert!(report.rejected.is_empty());
                    assert_eq!(report.pending, 1);
                }
                assert!(!graph.has_commit(child.hash()));
                graph
                    .ingest_signature(
                        author(&key(1)),
                        hash(&case.commits[8]),
                        support::signature(&key(1), hash(&case.commits[8])),
                    )
                    .unwrap()
            }
            "merge" => {
                let mut source = CommitGraph::new();
                source.ingest(higher, hash).unwrap();
                graph.merge(&source)
            }
            _ => unreachable!(),
        };
        assert_eq!(
            report.admitted.into_iter().collect::<BTreeSet<_>>(),
            [higher_id, child_id.clone()].into(),
            "trigger={trigger}"
        );
        assert!(report.rejected.is_empty());
        assert_eq!(report.pending, 0);
        assert_eq!(graph.pending_items(), 0);
        assert!(graph.missing_dependencies().next().is_none());
        assert!(graph.has_commit(child.hash()));
        graph.export(&child_id).unwrap().verify().unwrap();
        assert_eq!(
            graph.heads().collect::<BTreeSet<_>>(),
            [hash(&case.commits[8]), hash(child)].into()
        );
    }
}

#[test]
fn deferred_invalid_fragment_does_not_roll_back_its_dependency_or_block_valid_waiters() {
    let case = &cases()[0];
    let boundary = fixture_item(&case.commits, &case.commits[16])
        .verify()
        .unwrap();
    assert!(boundary.dependencies().next().is_none());
    let boundary_id = boundary.id().clone();
    // Root 0 is behind boundary 16, so including it in head 18's payload is
    // statelessly verifiable but graph-dependent canonical placement is invalid.
    let members: Vec<_> = [0, 17, 18].map(|i| case.commits[i].clone()).into();
    let invalid = UnverifiedItem::Fragment {
        checkpoints: BTreeSet::new(),
        range: case_range(case, 18),
        signatures: members.iter().map(evidence).collect(),
        commits: members,
    }
    .verify()
    .unwrap();
    let invalid_id = invalid.id().clone();
    let child = &case.commits[17];
    for deferred in [false, true] {
        let mut graph = CommitGraph::new();
        if deferred {
            let mut callbacks = 0;
            let report = graph
                .ingest(invalid.clone(), |meta| {
                    callbacks += 1;
                    hash(meta)
                })
                .unwrap();
            assert_eq!(callbacks, 3);
            assert!(report.admitted.is_empty());
            assert!(report.rejected.is_empty());
            assert_eq!(report.pending, 1);
            let report = graph
                .ingest(envelope(child).verify().unwrap(), hash)
                .unwrap();
            assert!(report.admitted.is_empty());
            assert_eq!(report.pending, 2);
            assert!(graph.heads().next().is_none());
            assert_eq!(
                graph
                    .missing_dependencies()
                    .copied()
                    .collect::<BTreeSet<_>>(),
                [hash(&case.commits[16])].into()
            );
            let report = graph.ingest(boundary.clone(), hash).unwrap();
            assert_eq!(
                report.admitted.into_iter().collect::<BTreeSet<_>>(),
                [boundary_id.clone(), ItemId::Commit(hash(child))].into()
            );
            assert_eq!(report.rejected.len(), 1);
            assert_eq!(report.rejected[0].item, invalid_id);
            assert_eq!(report.rejected[0].error, IngestError::InvalidFragment);
            assert_eq!(report.pending, 0);
            assert!(graph.has_commit(child.hash()));
        } else {
            graph.ingest(boundary.clone(), hash).unwrap();
            let before = graph.view_point();
            assert_eq!(
                graph.ingest(invalid.clone(), |_| panic!(
                    "ready invalid fragment requests no payload"
                )),
                Err(IngestError::InvalidFragment)
            );
            assert_eq!(graph.view_point(), before);
            assert!(!graph.has_commit(child.hash()));
        }
        assert_eq!(graph.pending_items(), 0);
        assert!(graph.missing_dependencies().next().is_none());
        assert!(graph.has_commit(&hash(&case.commits[16])));
        assert!(!graph.has_commit(&hash(&case.commits[18])));
        assert!(graph.publishable_commit(&hash(&case.commits[18])).is_none());
        assert!(graph.export(&invalid_id).is_err());
        graph.export(&boundary_id).unwrap().verify().unwrap();
    }
}

#[test]
fn redundant_ancestor_boundary_is_retained_even_when_normalization_excludes_its_history() {
    let seed = 0xb0_0d_a;
    let root = mine(vec![], 0, seed, 0);
    let older = mine(vec![hash(&root)], 1, seed, 1);
    let newer = mine(vec![hash(&older)], 1, seed, 2);
    let head = mine(vec![hash(&newer), hash(&older), hash(&root)], 1, seed, 3);
    let case = Case {
        seed,
        commits: vec![root, older, newer, head],
    };
    let mut graph = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits {
        // Exercise the shared graph-history fixture as well as the walk oracle.
        let item = support::received_item(&graph, meta.clone());
        if hash(meta) == hash(&case.commits[3]) {
            let UnverifiedItem::Fragment {
                range,
                commits,
                signatures,
                ..
            } = &item
            else {
                panic!("nonzero fixture must be a fragment")
            };
            assert_eq!(
                range.boundary,
                [1, 2].map(|i| hash(&case.commits[i])).into()
            );
            assert_eq!(
                item.clone()
                    .verify()
                    .unwrap()
                    .external_parents()
                    .copied()
                    .collect::<BTreeSet<_>>(),
                [0, 1, 2].map(|i| hash(&case.commits[i])).into()
            );
            assert_eq!(commits.iter().map(hash).collect::<Vec<_>>(), [hash(meta)]);
            assert_eq!(
                signatures.iter().map(|s| s.commit).collect::<Vec<_>>(),
                [hash(meta)]
            );
        }
        graph.ingest(item.verify().unwrap(), hash).unwrap();
        model.ingest(meta, false);
    }
    check(&graph, &model, &case, "redundant raw boundary retained");
    assert_eq!(graph.minimal_cover().fragments.len(), 3);
    check_direct_inventory(&graph, &model, &case);
}

#[test]
fn every_arrival_matches_the_walk_in_causal_reverse_and_shuffled_orders() {
    for case in cases() {
        let mut rng = StdRng::seed_from_u64(case.seed ^ 0xa771_a15);
        // Exercise each distinct schedule once; extra seeds are opt-in.
        for schedule in 0..3 {
            let mut order: Vec<_> = (0..case.commits.len()).collect();
            match schedule {
                0 => (),
                1 => order.reverse(),
                _ => order.shuffle(&mut rng),
            }
            let mut graph = CommitGraph::new();
            let mut receiver = Receiver::new(&case.commits, |h| h);
            let mut model = Model::default();
            check(&graph, &model, case, "empty");
            for (step, node) in order.into_iter().enumerate() {
                let meta = &case.commits[node];
                model.accepted(receiver.receive(&mut graph, meta, hash(meta)));
                let event = format!("schedule={schedule}, step={step}, node={node}");
                check(&graph, &model, case, &event);
            }
            assert_eq!(graph.pending_items(), 0);
            check_direct_inventory(&graph, &model, case);
            let all = case.commits.iter().map(hash).collect();
            assert_eq!(model.published(&case.commits), all, "seed={:#x}", case.seed);
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
    let result = graph.ingest_signature(
        author(&key),
        hash(meta),
        support::signature(&key, hash(meta)),
    );
    if result == Err(stockfoil::SignatureError::NotPendingLocalCommit(hash(meta))) {
        assert!(!model.local.contains(meta.hash()) || graph.has_commit(meta.hash()));
    } else {
        result.unwrap();
        model.signatures.insert((author(&key), hash(meta)));
    }
}

#[test]
fn signed_received_duplicate_authenticates_local_head_and_drains_buffered_child() {
    let case = &cases()[0];
    let mut graph = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits[..8] {
        support::receive_commit(&mut graph, meta.clone(), hash(meta));
        model.ingest(meta, false);
    }
    let head = &case.commits[8];
    graph.ingest_local_commit(head.clone(), hash(head));
    model.ingest(head, true);
    let mut receiver = Receiver::new(&case.commits, |h| h);
    let child = &case.commits[9];
    assert!(receiver.receive(&mut graph, child, hash(child)).is_empty());
    check(
        &graph,
        &model,
        case,
        "signed child blocked by unsigned local parent",
    );
    let verified = envelope(child).verify().unwrap();
    assert_eq!(
        verified.dependencies().copied().collect::<Vec<_>>(),
        [hash(head)]
    );
    assert!(!graph.has_commit(head.hash()));
    assert_eq!(graph.pending_items(), 1);
    assert_eq!(
        graph
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        [hash(head)].into()
    );
    assert_eq!(graph.heads().collect::<Vec<_>>(), [hash(head)]);
    assert!(graph.publishable_commit(child.hash()).is_none());
    for meta in [head, child] {
        let id = ItemId::Commit(hash(meta));
        assert_eq!(
            graph.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    // A valid received duplicate is real evidence, unlike a bare envelope or
    // a signature by somebody other than the author. It can publish local work.
    model.accepted(receiver.receive(&mut graph, head, hash(head)));
    assert_eq!(graph.pending_items(), 0);
    check(
        &graph,
        &model,
        case,
        "verified duplicate unlocks buffered child",
    );
}

#[test]
fn signed_canonical_fragment_relays_whole_but_unsigned_subfragment_is_not_advertised() {
    let case = &cases()[0];
    let range = case_range(case, 8);
    let smaller = case_range(case, 6);
    let mut source = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits[..=8] {
        source.ingest_local_commit(meta.clone(), hash(meta));
        model.ingest(meta, true);
    }
    add_signature(&mut source, &mut model, 1, &case.commits[8]);
    check(&source, &model, case, "signed larger canonical fragment");
    // Structural summaries still describe authenticated subfragments, even
    // though their only signature target lies outside their own membership.
    assert!(
        source
            .as_sedimentree()
            .summary()
            .fragments
            .iter()
            .any(|f| f.range == smaller)
    );
    // Valid independently signed envelopes do not grant arbitrary export rights.
    fixture_item(&case.commits, &case.commits[6])
        .verify()
        .unwrap();
    envelope(&case.commits[3]).verify().unwrap();
    for id in [
        ItemId::Fragment(smaller.clone()),
        ItemId::Commit(smaller.head),
        ItemId::Commit(hash(&case.commits[3])),
    ] {
        assert_eq!(
            source.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    let inventory = source.minimal_cover();
    assert_eq!(inventory.fragments.len(), 1);
    assert_eq!(inventory.fragments[0].range, range);
    assert!(inventory.loose_commits.is_empty());
    assert!(!inventory.fragments.iter().any(|f| f.range == smaller));

    let id = ItemId::Fragment(range.clone());
    let mut destination = CommitGraph::new();
    let verified = source.export(&id).unwrap().verify().unwrap();
    let mut supplied = Vec::new();
    destination
        .ingest(verified, |meta| {
            supplied.push(hash(meta));
            hash(meta)
        })
        .unwrap();
    let members: BTreeSet<_> = case.commits[..=8].iter().map(hash).collect();
    assert_eq!(supplied, export_order(&case.commits, &members));
    check(
        &destination,
        &model,
        case,
        "self-contained fragment transport",
    );
    assert_eq!(destination.minimal_cover(), inventory);
    let stale = ItemId::Fragment(smaller);
    assert_eq!(
        destination.export(&stale).unwrap_err(),
        ExportError::NotInMinimalCover(stale)
    );
    // The retained internal signature supports another whole-fragment relay.
    let mut third = CommitGraph::new();
    third.merge(&destination);
    check(
        &third,
        &model,
        case,
        "re-export retained internal signature",
    );
    assert_eq!(third.minimal_cover(), inventory);
}

#[test]
fn pending_local_items_do_not_enter_the_index_until_their_certificates_exist() {
    let case = &cases()[0];
    let mut source = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits[..8] {
        support::receive_commit(&mut source, meta.clone(), hash(meta));
        model.ingest(meta, false);
    }
    let old_model = model.clone();
    let old = source.minimal_cover();
    assert!(!old.fragments.is_empty());
    for meta in &case.commits[8..=9] {
        source.ingest_local_commit(meta.clone(), hash(meta));
        model.ingest(meta, true);
    }
    // Signing a shallow descendant does not admit its pending prerequisite.
    add_signature(&mut source, &mut model, 1, &case.commits[9]);
    check(
        &source,
        &model,
        case,
        "pending checkpoint lacks its certificate",
    );
    assert_eq!(model.published(&case.commits).len(), 8);
    assert_eq!(source.minimal_cover(), old);
    check_direct_inventory(&source, &model, case);
    for id in [
        ItemId::Fragment(case_range(case, 8)),
        ItemId::Commit(hash(&case.commits[9])),
    ] {
        assert_eq!(
            source.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    // Historical signature ingestion is rejected without affecting the cover.
    add_signature(&mut source, &mut model, 1, &case.commits[6]);
    assert_eq!(source.minimal_cover(), old);
    let mut destination = CommitGraph::new();
    destination.merge(&source);
    check(
        &destination,
        &old_model,
        case,
        "old closed cover still merges",
    );

    add_signature(&mut source, &mut model, 1, &case.commits[8]);
    assert_structural_cover(&source, summaries(&source).1);
    assert_ne!(source.minimal_cover(), old);
    check_direct_inventory(&source, &model, case);
    // Snapshots do not pin their superseded certificates, even though the old
    // ranges and commits remain valid and readable in the structural graph.
    for id in cover_ids(&old.loose_commits, &old.fragments) {
        assert_eq!(
            source.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    destination.merge(&source);
    check(
        &destination,
        &model,
        case,
        "checkpoint admission unlocks its signed child",
    );
}

#[test]
fn pending_local_fragment_does_not_block_an_independent_certified_fragment() {
    let seed = 0xb0_0d;
    let alice = author(&key(1));
    let bob = author(&key(2));
    let root = mine_as(vec![], 0, seed, 0, alice);
    let boundary = mine_as(vec![hash(&root)], 1, seed, 1, bob);
    let descendant = mine_as(vec![hash(&boundary)], 0, seed, 2, alice);
    let head = mine_as(vec![hash(&boundary)], 1, seed, 3, bob);
    let sibling = mine_as(vec![hash(&root)], 1, seed, 4, alice);
    let case = Case {
        seed,
        commits: vec![root, boundary, descendant, head, sibling],
    };
    let [_root, boundary, descendant, head, sibling] = case.commits.as_slice() else {
        unreachable!()
    };
    let mut source = CommitGraph::new();
    let mut model = Model::default();
    for meta in &case.commits {
        source.ingest_local_commit(meta.clone(), hash(meta));
        model.ingest(meta, true);
    }
    for (signer, meta) in [(2, boundary), (1, descendant), (2, head), (1, sibling)] {
        add_signature(&mut source, &mut model, signer, meta);
        check(&source, &model, &case, "partial mixed-author signatures");
        assert!(source.minimal_cover().loose_commits.is_empty());
        assert_eq!(
            source.minimal_cover().fragments.len(),
            usize::from(hash(meta) == hash(sibling))
        );
        check_direct_inventory(&source, &model, &case);
    }
    assert_eq!(model.published(&case.commits).len(), 2);
    // The independent sibling publishes immediately. B and its descendants
    // remain pending, and cannot borrow Alice's certificate from another range.
    for id in [
        ItemId::Commit(hash(boundary)),
        ItemId::Fragment(case_range(&case, 1)),
        ItemId::Fragment(case_range(&case, 3)),
    ] {
        assert_eq!(
            source.export(&id).unwrap_err(),
            ExportError::NotInMinimalCover(id)
        );
    }
    let mut destination = CommitGraph::new();
    destination.merge(&source);
    check(
        &destination,
        &model,
        &case,
        "independent certified history transfers",
    );
    add_signature(&mut source, &mut model, 1, boundary);
    assert_structural_cover(&source, summaries(&source).1);
    check_direct_inventory(&source, &model, &case);
    let item = source
        .export(&ItemId::Fragment(case_range(&case, 3)))
        .unwrap()
        .verify()
        .unwrap();
    assert_eq!(
        item.dependencies().copied().collect::<Vec<_>>(),
        [hash(boundary)]
    );
    let id = item.id().clone();
    let report = destination.ingest(item, hash).unwrap();
    assert!(report.admitted.is_empty());
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 1);
    assert_eq!(
        destination
            .missing_dependencies()
            .copied()
            .collect::<BTreeSet<_>>(),
        [hash(boundary)].into()
    );
    assert!(destination.publishable_commit(head.hash()).is_none());
    let report = destination.merge(&source);
    assert!(report.admitted.contains(&id));
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 0);
    check(
        &destination,
        &model,
        &case,
        "complete mixed-author cover merges",
    );
    let mut relay = CommitGraph::new();
    relay.merge(&destination);
    check(&relay, &model, &case, "complete mixed-author cover relays");
}

#[test]
fn late_other_author_fork_requires_fresh_evidence_after_certificate_replacement() {
    let seed = 0xce47;
    let root = mine_as(vec![], 0, seed, 0, author(&key(1)));
    let head = mine_as(vec![hash(&root)], 1, seed, 1, author(&key(1)));
    let fork = mine_as(vec![hash(&root)], 1, seed, 2, author(&key(2)));
    let case = Case {
        seed,
        commits: vec![root, head, fork],
    };
    let [root, head, fork] = case.commits.as_slice() else {
        unreachable!()
    };
    let mut source = CommitGraph::new();
    let mut model = Model::default();
    support::receive_commit(&mut source, root.clone(), hash(root));
    model.ingest(root, false);
    let stale = ItemId::Commit(hash(root));
    source.export(&stale).unwrap().verify().unwrap();
    support::receive_commit(&mut source, head.clone(), hash(head));
    model.ingest(head, false);
    let old = source.minimal_cover();
    assert_eq!(old.fragments.len(), 1);
    assert!(old.loose_commits.is_empty());
    assert_eq!(
        source.export(&stale).unwrap_err(),
        ExportError::NotInMinimalCover(stale.clone())
    );
    assert!(source.has_commit(root.hash()));

    // The old loose certificate is no longer available to a new local fork.
    model.signatures.remove(&(*root.author(), hash(root)));
    // Bob's signature alone cannot admit an Alice-authored interior ancestor.
    source.ingest_local_commit(fork.clone(), hash(fork));
    model.ingest(fork, true);
    add_signature(&mut source, &mut model, 2, fork);
    check(
        &source,
        &model,
        &case,
        "late fork with pruned ancestor certificate",
    );
    assert_eq!(model.published(&case.commits).len(), 2);
    assert_eq!(source.minimal_cover(), old);
    let fork_id = ItemId::Fragment(case_range(&case, 2));
    assert_eq!(
        source.export(&fork_id).unwrap_err(),
        ExportError::NotInMinimalCover(fork_id)
    );
    check_direct_inventory(&source, &model, &case);

    // Alice co-signs the pending local head, not the historical root.
    add_signature(&mut source, &mut model, 1, fork);
    assert_structural_cover(&source, summaries(&source).1);
    assert_eq!(source.minimal_cover().fragments.len(), 2);
    // The old covered commit still cannot be exported on its own.
    assert_eq!(
        source.export(&stale).unwrap_err(),
        ExportError::NotInMinimalCover(stale)
    );
    check_direct_inventory(&source, &model, &case);
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
    let id = ItemId::Fragment(range.clone());
    assert_eq!(
        graph.export(&id).unwrap_err(),
        ExportError::NotInMinimalCover(id),
        "transport exposes only current cached entries, not borrowed-read errors"
    );
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

    let mut receiver = Receiver::new(&case.commits, |h| h);
    assert!(
        receiver
            .receive(&mut graph, &case.commits[6], head)
            .is_empty()
    );
    assert_eq!(graph.pending_items(), 1);
    assert!(graph.publishable_commit(&head).is_none());
    assert_read_error(&graph, &range, FragmentReadError::MissingCommit(head));
    for meta in &case.commits {
        receiver.receive(&mut graph, meta, hash(meta));
    }
    assert_eq!(graph.pending_items(), 0);
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
fn boundary_bypass_ancestors_are_dependencies_not_payload_and_break_is_global() {
    for case in cases() {
        let mut graph = CommitGraph::new();
        for meta in &case.commits[..=18] {
            support::receive_commit(&mut graph, meta.clone(), hash(meta));
        }
        let range = case_range(case, 18);
        let mut actual = Vec::new();
        graph
            .traverse_fragment(&range, |meta, _| {
                actual.push(hash(meta));
                ControlFlow::Continue(())
            })
            .unwrap();
        let boundary = hash(&case.commits[16]);
        let bypass = hash(&case.commits[0]);
        assert_eq!(range.boundary, BTreeSet::from([boundary]));
        assert_eq!(actual, [17, 18].map(|i| hash(&case.commits[i])));
        assert!(!actual.contains(&bypass));
        assert!(!actual.contains(&boundary));
        let verified = graph
            .export(&ItemId::Fragment(range.clone()))
            .unwrap()
            .verify()
            .unwrap();
        assert_eq!(verified.commits().map(hash).collect::<Vec<_>>(), actual);
        assert_eq!(
            verified.dependencies().copied().collect::<BTreeSet<_>>(),
            [boundary, bypass].into()
        );
        assert!(
            graph
                .minimal_cover()
                .fragments
                .iter()
                .any(|f| f.range.head == boundary)
        );
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
fn local_fragment_reads_require_delivery_then_verified_admission() {
    let case = &cases()[0];
    let range = case_range(case, 8);
    let mut graph = CommitGraph::new();
    assert!(graph.publishable_commit(&range.head).is_none());
    assert_read_error(&graph, &range, FragmentReadError::MissingCommit(range.head));
    assert_eq!(
        graph.ingest_signature(
            author(&key(1)),
            range.head,
            support::signature(&key(1), range.head),
        ),
        Err(stockfoil::SignatureError::NotPendingLocalCommit(range.head))
    );
    for meta in &case.commits[..8] {
        support::receive_commit(&mut graph, meta.clone(), hash(meta));
    }
    graph.ingest_local_commit(case.commits[8].clone(), range.head);
    assert!(graph.publishable_commit(&range.head).is_none());
    assert_read_error(
        &graph,
        &range,
        FragmentReadError::NotPublishable(range.head),
    );
    // Another author's valid signature does not authenticate this head.
    sign(&mut graph, &key(2), range.head);
    assert_read_error(
        &graph,
        &range,
        FragmentReadError::NotPublishable(range.head),
    );
    sign(&mut graph, &key(1), range.head);
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
            support::receive_commit(&mut graph, meta.clone(), hash(meta));
        }
        graph.ingest_local_commit(ancestor.clone(), hash(ancestor));
        if signature_first {
            assert_eq!(
                graph.ingest_signature(
                    author(&key(2)),
                    range.head,
                    support::signature(&key(2), range.head)
                ),
                Err(stockfoil::SignatureError::NotPendingLocalCommit(range.head))
            );
        }
        graph.ingest_local_commit(descendant.clone(), hash(&descendant));
        sign(&mut graph, &key(2), range.head);
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
        assert_eq!(seen, [range.head]);
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
    assert_eq!(memberships[&hash(&head)], BTreeSet::from([hash(&head)]));
    for reverse in [false, true] {
        let mut graph = CommitGraph::new();
        let mut order: Vec<_> = commits.iter().collect();
        if reverse {
            order.reverse();
        }
        let mut receiver = Receiver::new(&commits, |h| h);
        for meta in order {
            receiver.receive(&mut graph, meta, hash(meta));
        }
        assert_eq!(graph.pending_items(), 0);
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
    // Reverse transport order exercises graph buffering without payload clones.
    let mut receiver = Receiver::new(&case.commits, NoClone);
    for meta in case.commits[..7].iter().rev() {
        receiver.receive(&mut graph, meta, NoClone(hash(meta)));
    }
    assert_eq!(graph.pending_items(), 0);
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
        support::receive_commit(&mut graph, meta.clone(), NoClone(hash(meta)));
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
