//! Public-API regression tests for graph-local ancestry and captured membership.
//! The oracle follows fixture parent edges only; it never asks a graph for reachability.
mod support;

use std::collections::{HashMap, HashSet};

use stockfoil::{
    Author, BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, DiffError, QueryError,
    ResolveError, SignatureEvidence, UnverifiedItem, ViewPoint, Vis, VisDiff, WritePermission,
};
use support::{author, key, signature};

type Permissions = HashMap<Author, WritePermission>;

struct Rng(u64);

impl Rng {
    fn pick(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn authors() -> [Author; 3] {
    [author(&key(1)), author(&key(2)), author(&key(3))]
}

fn allowed() -> Permissions {
    authors()
        .into_iter()
        .map(|a| (a, WritePermission::Unbounded))
        .collect()
}

fn metadata(seed: u64, id: usize, parents: Vec<CommitHash>) -> CommitMeta {
    let mut payload = seed.to_le_bytes().to_vec();
    payload.extend_from_slice(&(id as u64).to_le_bytes());
    CommitMeta::new(CommitContents {
        parents,
        author: authors()[id % 3],
        payload: BlobHash::of_contents(&payload),
    })
}

/// A shared prefix, two independently appendable branches, and a final join.
/// The fixed prefix guarantees siblings, multiple roots, repeated edges, and
/// an allowed -> denied -> allowed path, regardless of the random seed.
fn fixture(seed: u64) -> Vec<CommitMeta> {
    let mut rng = Rng(seed);
    let mut nodes: Vec<CommitMeta> = Vec::new();
    for i in 0..102 {
        let parents = match i {
            0 | 1 => vec![],
            2 | 3 => vec![0],
            4 => vec![2, 3, 2],
            5 => vec![4, 1],
            6 => vec![4],
            7 => vec![5, 6, 6],
            100 => vec![69, 99, 69],
            101 => vec![100, 100],
            _ if i % 19 == 0 => vec![],
            _ => {
                // The right branch may depend on the prefix, but not the left.
                let candidates: Vec<_> = (0..i).filter(|p| i < 70 || *p < 40 || *p >= 70).collect();
                let mut parents = vec![if i == 70 { 39 } else { i - 1 }];
                for _ in 0..rng.pick(3) {
                    parents.push(candidates[rng.pick(candidates.len())]);
                }
                if i % 11 == 0 {
                    parents.push(parents[0]);
                }
                parents
            }
        };
        nodes.push(metadata(
            seed,
            i,
            parents.into_iter().map(|p| *nodes[p].hash()).collect(),
        ));
    }
    nodes
}

struct Oracle {
    nodes: HashMap<CommitHash, CommitMeta>,
}

impl Oracle {
    fn new(nodes: impl IntoIterator<Item = CommitMeta>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|m| (*m.hash(), m)).collect(),
        }
    }

    /// Inclusive history, computed by an ordinary stack-based parent walk.
    fn history(&self, heads: &[CommitHash]) -> HashSet<CommitHash> {
        let mut seen = HashSet::new();
        let mut stack = heads.to_vec();
        while let Some(hash) = stack.pop() {
            if let Some(meta) = self.nodes.get(&hash) {
                if seen.insert(hash) {
                    stack.extend_from_slice(meta.parents());
                }
            }
        }
        seen
    }

    fn visible(&self, view: &ViewPoint) -> HashSet<CommitHash> {
        let history = self.history(view.heads());
        history
            .iter()
            .copied()
            .filter(
                |hash| match view.write_permissions().get(self.nodes[hash].author()) {
                    Some(WritePermission::Unbounded) => true,
                    Some(WritePermission::Bounded(bounds)) => bounds.iter().any(|bound| {
                        // A known bound outside these captured heads proves nothing.
                        // Neither the bound's author nor intermediate authors matter.
                        history.contains(bound) && self.history(&[*bound]).contains(hash)
                    }),
                    Some(WritePermission::Denied) | None => false,
                },
            )
            .collect()
    }
}

fn receive(graph: &mut CommitGraph<usize>, meta: &CommitMeta, id: usize) {
    let signing_key = key((id % 3 + 1) as u8);
    // Standalone signed commits are valid at every hash depth. Do not invent
    // fragment certificates or filter fixture hashes to depth zero.
    let item = UnverifiedItem::Commit {
        meta: meta.clone(),
        signatures: vec![SignatureEvidence {
            author: author(&signing_key),
            commit: *meta.hash(),
            signature: signature(&signing_key, *meta.hash()),
        }],
    }
    .verify()
    .unwrap();
    graph
        .ingest(item, |received| {
            assert_eq!(received.hash(), meta.hash());
            id
        })
        .unwrap();
}

fn assert_ancestry(graph: &CommitGraph<usize>, oracle: &Oracle, hashes: &[CommitHash]) {
    for descendant in hashes {
        let history = oracle.history(&[*descendant]);
        for ancestor in hashes {
            assert_eq!(
                graph.is_ancestor(ancestor, descendant),
                ancestor != descendant && history.contains(ancestor),
                "ancestry {ancestor:?} -> {descendant:?}"
            );
        }
    }
}

fn assert_views(
    graph: &CommitGraph<usize>,
    oracle: &Oracle,
    views: &[ViewPoint],
    unknown: CommitHash,
    drafts: bool,
) {
    let expected: Vec<_> = views.iter().map(|v| oracle.visible(v)).collect();
    for (i, view) in views.iter().enumerate() {
        if drafts && !view.heads().is_empty() {
            assert_eq!(
                graph.resolve(view).unwrap_err(),
                ResolveError::PendingLocalCommit(view.heads()[0])
            );
        } else {
            let resolved = graph.resolve(view).unwrap();
            for hash in oracle.nodes.keys() {
                assert_eq!(resolved.visible(hash).unwrap(), expected[i].contains(hash));
            }
            assert_eq!(
                resolved.visible(&unknown),
                Err(QueryError::UnknownCommit(unknown))
            );
        }
        // Compare both directions and identical snapshots, without assuming any
        // changed_commits iteration order, and explicitly reject duplicates.
        for (j, other) in views.iter().enumerate() {
            let diff = graph.diff(view, other).unwrap();
            let mut changes = HashMap::new();
            for hash in oracle.nodes.keys() {
                let visibility = match (expected[i].contains(hash), expected[j].contains(hash)) {
                    (true, true) => Vis::Both,
                    (true, false) => {
                        changes.insert(*hash, VisDiff::Before);
                        Vis::Before
                    }
                    (false, true) => {
                        changes.insert(*hash, VisDiff::After);
                        Vis::After
                    }
                    (false, false) => Vis::Neither,
                };
                assert_eq!(
                    diff.visibility(hash).unwrap(),
                    visibility,
                    "views {i} -> {j}"
                );
            }
            assert_eq!(
                diff.visibility(&unknown),
                Err(QueryError::UnknownCommit(unknown))
            );
            let actual: Vec<_> = diff.changed_commits().collect();
            let unique: HashMap<_, _> = actual
                .iter()
                .map(|c| (c.commit_id, c.visibility_delta))
                .collect();
            assert_eq!(actual.len(), unique.len(), "duplicate changed commit");
            assert_eq!(unique, changes, "changes for views {i} -> {j}");
        }
    }

    for permissions in [allowed(), HashMap::new()] {
        for heads in [
            vec![unknown],
            vec![*oracle.nodes.keys().next().unwrap(), unknown],
        ] {
            let invalid = ViewPoint::from_parts(heads, permissions.clone());
            // Diff and resolve must reject missing heads even under total denial.
            assert_eq!(
                graph.diff(&views[0], &invalid).unwrap_err(),
                DiffError::MissingCommit(unknown)
            );
            assert_eq!(
                graph.diff(&invalid, &views[0]).unwrap_err(),
                DiffError::MissingCommit(unknown)
            );
            if !drafts {
                assert_eq!(
                    graph.resolve(&invalid).unwrap_err(),
                    ResolveError::MissingCommit(unknown)
                );
            }
        }
    }
}

fn views(
    nodes: &[CommitMeta],
    unknown: CommitHash,
    mut captured: Vec<ViewPoint>,
) -> Vec<ViewPoint> {
    let h = |i: usize| *nodes[i].hash();
    let [a, b, c] = authors();
    let bounded = HashMap::from([(a, WritePermission::Bounded(vec![h(4), h(4), unknown]))]);
    captured.extend([
        ViewPoint::from_parts(vec![], allowed()),
        ViewPoint::from_parts(vec![h(101)], allowed()),
        ViewPoint::from_parts(vec![h(101)], HashMap::new()),
        // Node 4 is globally known but outside this snapshot's history.
        ViewPoint::from_parts(vec![h(2), h(3), h(1), h(2)], bounded.clone()),
        ViewPoint::from_parts(vec![h(4)], bounded),
        ViewPoint::from_parts(
            vec![h(101)],
            HashMap::from([
                (
                    a,
                    WritePermission::Bounded(vec![h(17), h(35), h(35), unknown]),
                ),
                (b, WritePermission::Denied),
                (c, WritePermission::Unbounded),
            ]),
        ),
        ViewPoint::from_parts(
            vec![h(69), h(99)],
            HashMap::from([
                (a, WritePermission::Bounded(vec![])),
                (b, WritePermission::Bounded(vec![h(3), h(99), unknown])),
            ]),
        ),
        ViewPoint::from_parts(
            vec![h(101)],
            authors()
                .into_iter()
                .map(|a| (a, WritePermission::Bounded(vec![unknown])))
                .collect(),
        ),
    ]);
    captured
}

#[test]
fn seeded_parent_walk_matches_local_received_reordered_and_merged_graphs() {
    for seed in [0x1234_5678, 0x9876_abcd] {
        let nodes = fixture(seed);
        let unknown = *metadata(seed, 10_000, vec![]).hash();
        let mut hashes: Vec<_> = nodes.iter().map(|m| *m.hash()).collect();
        hashes.push(unknown);
        let oracle = Oracle::new(nodes.clone());

        let mut left = CommitGraph::new();
        left.set_write_permissions(&allowed());
        for (i, meta) in nodes[..40].iter().enumerate() {
            receive(&mut left, meta, i);
        }
        let old = left.view_point();
        let prefix = Oracle::new(nodes[..40].iter().cloned());
        assert_ancestry(&left, &prefix, &hashes);
        assert_views(&left, &prefix, std::slice::from_ref(&old), unknown, false);
        let mut right = left.clone();
        for i in 40..70 {
            receive(&mut left, &nodes[i], i);
        }
        for i in 70..100 {
            receive(&mut right, &nodes[i], i);
        }
        let left_view = left.view_point();
        let right_view = right.view_point();
        assert_ancestry(&left, &Oracle::new(nodes[..70].iter().cloned()), &hashes);
        assert_ancestry(
            &right,
            &Oracle::new(nodes[..40].iter().chain(&nodes[70..100]).cloned()),
            &hashes,
        );
        // Clones assign their new local labels independently. Merge in both
        // directions, then append a real merge commit spanning both branches.
        let left_branch = left.clone();
        left.merge(&right);
        right.merge(&left_branch);
        for i in 100..102 {
            receive(&mut left, &nodes[i], i);
            receive(&mut right, &nodes[i], i);
        }
        let snapshots = views(&nodes, unknown, vec![old, left_view, right_view]);
        for graph in [&left, &right] {
            assert_ancestry(graph, &oracle, &hashes);
            assert_views(graph, &oracle, &snapshots, unknown, false);
        }

        // Choose the highest numbered ready node, rather than insertion order.
        // This changes which siblings extend a graph-local chain.
        let mut reordered = CommitGraph::new();
        let mut inserted = HashSet::new();
        let mut order = Vec::new();
        while inserted.len() < nodes.len() {
            let i = (0..nodes.len())
                .rev()
                .find(|&i| {
                    !inserted.contains(nodes[i].hash())
                        && nodes[i].parents().iter().all(|p| inserted.contains(p))
                })
                .unwrap();
            receive(&mut reordered, &nodes[i], i);
            inserted.insert(*nodes[i].hash());
            order.push(i);
        }
        assert_ne!(order, (0..nodes.len()).collect::<Vec<_>>());
        assert_ancestry(&reordered, &oracle, &hashes);
        assert_views(&reordered, &oracle, &snapshots, unknown, false);

        // Working membership includes drafts, but resolve still requires
        // admission. Every local insertion below has all parents present.
        let mut drafts = CommitGraph::new();
        drafts.set_write_permissions(&allowed());
        let mut draft_snapshots = Vec::new();
        for (i, meta) in nodes.iter().enumerate() {
            drafts.ingest_local_commit(meta.clone(), i);
            if i == 39 {
                draft_snapshots.push(drafts.view_point());
                draft_snapshots.push(drafts.signed_view_point());
            }
        }
        draft_snapshots.push(drafts.view_point());
        let draft_views = views(&nodes, unknown, draft_snapshots);
        assert_ancestry(&drafts, &oracle, &hashes);
        assert_views(&drafts, &oracle, &draft_views, unknown, true);

        // Explicit witnesses keep the permission oracle honest: denial of the
        // middle node 4 does not cut the path 0 -> 3 -> 4 -> 6.
        let witness = ViewPoint::from_parts(
            vec![*nodes[6].hash()],
            HashMap::from([(
                authors()[0],
                WritePermission::Bounded(vec![*nodes[6].hash()]),
            )]),
        );
        let resolved = left.resolve(&witness).unwrap();
        assert!(resolved.visible(nodes[0].hash()).unwrap());
        assert!(resolved.visible(nodes[3].hash()).unwrap());
        assert!(!resolved.visible(nodes[4].hash()).unwrap());
        assert!(resolved.visible(nodes[6].hash()).unwrap());
        assert_eq!(
            oracle.visible(&witness),
            HashSet::from([*nodes[0].hash(), *nodes[3].hash(), *nodes[6].hash(),])
        );
    }
}
