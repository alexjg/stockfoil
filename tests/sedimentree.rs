//! Structural summary tests: real commit hashes, no blobs, codec, or transport.
mod support;

use std::{
    collections::{BTreeSet, HashMap},
    sync::OnceLock,
};

use rand::{SeedableRng, seq::SliceRandom};
use stockfoil::{
    BlobHash, Checkpoint, CommitContents, CommitGraph, CommitHash, CommitMeta, Fragment,
    FragmentRange, LooseCommit, SedimentreeSummary, WritePermission,
};
use support::{author, key, sign};

// Mine actual hashes at exact depths. Depth fixtures never replace the live
// leading-zero-byte metric. Shared depth-2 commits are mined only once.
fn commit(parents: &[CommitHash], depth: usize, label: &str) -> CommitMeta {
    let author = author(&key(1));
    for nonce in 0..2_000_000u64 {
        let meta = CommitMeta::new(CommitContents {
            parents: parents.to_vec(),
            author,
            payload: BlobHash::of_contents(format!("{label}:{nonce}").as_bytes()),
        });
        if meta
            .hash()
            .as_bytes()
            .iter()
            .take_while(|b| **b == 0)
            .count()
            == depth
        {
            return meta;
        }
    }
    panic!("could not mine depth-{depth} fixture {label}");
}

fn hash(meta: &CommitMeta) -> CommitHash {
    *meta.hash()
}

fn loose(meta: &CommitMeta) -> LooseCommit {
    LooseCommit {
        commit: hash(meta),
        parents: meta.parents().iter().copied().collect(),
    }
}

fn fragment(head: &CommitMeta, boundary: &[&CommitMeta], checkpoints: &[&CommitMeta]) -> Fragment {
    Fragment {
        range: FragmentRange {
            head: hash(head),
            boundary: boundary.iter().map(|m| hash(m)).collect(),
        },
        checkpoints: checkpoints
            .iter()
            .map(|m| Checkpoint(m.hash().as_bytes()[..12].try_into().unwrap()))
            .collect(),
    }
}

fn expected(loose: &[&CommitMeta], mut fragments: Vec<Fragment>) -> SedimentreeSummary {
    let mut loose_commits: Vec<_> = loose.iter().map(|m| self::loose(m)).collect();
    loose_commits.sort_by_key(|c| c.commit);
    fragments.sort_by(|a, b| a.range.cmp(&b.range));
    SedimentreeSummary {
        loose_commits,
        fragments,
    }
}

fn graph(commits: &[&CommitMeta]) -> CommitGraph<()> {
    let mut graph = CommitGraph::new();
    for meta in commits {
        graph.ingest_commit((*meta).clone(), ());
    }
    graph
}

struct Nested {
    root: CommitMeta,
    left: CommitMeta,
    right: CommitMeta,
    merge: CommitMeta,
    outer: CommitMeta,
    tail: CommitMeta,
}

fn nested() -> &'static Nested {
    static FIXTURE: OnceLock<Nested> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let root = commit(&[], 0, "root");
        let left = commit(&[hash(&root)], 1, "left");
        let right = commit(&[hash(&root)], 1, "right");
        let merge = commit(&[hash(&left), hash(&right)], 1, "merge");
        let outer = commit(&[hash(&merge)], 2, "outer");
        let tail = commit(&[hash(&outer)], 0, "tail");
        Nested {
            root,
            left,
            right,
            merge,
            outer,
            tail,
        }
    })
}

#[test]
fn empty_graph_has_empty_summary() {
    let graph = CommitGraph::<()>::new();
    assert_eq!(
        graph.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
}

#[test]
fn loose_chain_and_merge_preserve_parent_sets_in_hash_order() {
    let a = commit(&[], 0, "a");
    let b = commit(&[hash(&a)], 0, "b");
    let c = commit(&[], 0, "c");
    // The envelope may carry a different parent order; the summary is a set.
    let d = commit(&[hash(&c), hash(&b)], 0, "d");
    assert_eq!(
        graph(&[&a, &b, &c, &d]).as_sedimentree().summary(),
        expected(&[&a, &b, &c, &d], vec![]),
    );
}

#[test]
fn adjacent_fragments_keep_inclusive_boundaries_and_uncovered_tail() {
    let root = commit(&[], 0, "adjacent root");
    let first = commit(&[hash(&root)], 1, "first");
    let middle = commit(&[hash(&first)], 0, "middle");
    let second = commit(&[hash(&middle)], 1, "second");
    let tail = commit(&[hash(&second)], 0, "tail");
    assert_eq!(
        graph(&[&root, &first, &middle, &second, &tail])
            .as_sedimentree()
            .summary(),
        expected(
            &[&tail],
            vec![
                fragment(&first, &[], &[]),
                fragment(&second, &[&first], &[])
            ]
        ),
    );
}

#[test]
fn merge_has_multiple_boundaries_and_keeps_overlapping_predecessor_ranges() {
    let f = nested();
    assert_eq!(
        graph(&[&f.root, &f.left, &f.right, &f.merge])
            .as_sedimentree()
            .summary(),
        expected(
            &[],
            vec![
                fragment(&f.left, &[], &[]),
                fragment(&f.right, &[], &[]),
                fragment(&f.merge, &[&f.left, &f.right], &[]),
            ]
        ),
    );
}

#[test]
fn deeper_fragment_subsumes_nested_ranges_but_not_an_unrelated_branch() {
    let f = nested();
    let unrelated = commit(&[hash(&f.root)], 0, "unrelated branch");
    let mut graph = graph(&[&f.root, &f.left, &f.right, &f.merge, &unrelated]);
    let before = graph.as_sedimentree().summary();
    graph.ingest_commit(f.outer.clone(), ());
    graph.ingest_commit(f.tail.clone(), ());
    assert_eq!(
        graph.as_sedimentree().summary(),
        expected(
            &[&f.tail, &unrelated],
            vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge]),]
        ),
    );
    // An owned summary is a snapshot, not a live view of the mutable index.
    assert_eq!(before.fragments.len(), 3);
    assert_eq!(before.loose_commits, vec![loose(&unrelated)]);
}

#[test]
fn deeper_boundary_stops_the_walk_and_remains_in_the_cover() {
    let f = nested();
    let middle = commit(&[hash(&f.outer)], 0, "after depth two");
    let shallow = commit(&[hash(&middle)], 1, "shallow after depth two");
    assert_eq!(
        graph(&[
            &f.root, &f.left, &f.right, &f.merge, &f.outer, &middle, &shallow
        ])
        .as_sedimentree()
        .summary(),
        expected(
            &[],
            vec![
                fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge]),
                fragment(&shallow, &[&f.outer], &[]),
            ]
        ),
    );
}

#[test]
fn equal_depth_boundary_is_not_an_interior_checkpoint() {
    let f = nested();
    let inner = commit(&[hash(&f.outer)], 1, "inner after outer");
    let next = commit(&[hash(&inner)], 2, "next depth two");
    assert_eq!(
        graph(&[
            &f.root, &f.left, &f.right, &f.merge, &f.outer, &inner, &next
        ])
        .as_sedimentree()
        .summary(),
        expected(
            &[],
            vec![
                fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge]),
                fragment(&next, &[&f.outer], &[&inner]),
            ]
        ),
    );
}

#[test]
fn root_checkpoint_forms_a_fragment_without_an_invented_boundary() {
    let root = commit(&[], 1, "checkpoint root");
    assert_eq!(
        graph(&[&root]).as_sedimentree().summary(),
        expected(&[], vec![fragment(&root, &[], &[])])
    );
    let head = commit(&[hash(&root)], 1, "after checkpoint root");
    // The singleton root range is wholly contained by the next range.
    assert_eq!(
        graph(&[&root, &head]).as_sedimentree().summary(),
        expected(&[], vec![fragment(&head, &[&root], &[])])
    );
}

#[test]
fn mixed_merge_paths_stop_at_checkpoints_or_roots_independently() {
    let root = commit(&[], 0, "mixed root");
    let boundary = commit(&[hash(&root)], 1, "mixed boundary");
    let other_root = commit(&[], 0, "other root");
    let head = commit(&[hash(&boundary), hash(&other_root)], 1, "mixed head");
    assert_eq!(
        graph(&[&root, &boundary, &other_root, &head])
            .as_sedimentree()
            .summary(),
        expected(
            &[],
            vec![
                fragment(&boundary, &[], &[]),
                fragment(&head, &[&boundary], &[])
            ]
        ),
    );
}

#[test]
fn a_merge_can_reach_ancestry_also_hidden_behind_another_paths_boundary() {
    let root = commit(&[], 0, "bypassed root");
    let boundary = commit(&[hash(&root)], 1, "bypassed boundary");
    let other_path = commit(&[hash(&root)], 0, "bypass");
    let head = commit(&[hash(&boundary), hash(&other_path)], 1, "bypassing head");
    // The root is still included through the other path. The earlier range is
    // therefore fully contained, despite the cut on the boundary's parent edge.
    assert_eq!(
        graph(&[&root, &boundary, &other_path, &head])
            .as_sedimentree()
            .summary(),
        expected(&[], vec![fragment(&head, &[&boundary], &[])]),
    );
}

#[test]
fn queued_commits_are_absent_until_all_parents_arrive() {
    let f = nested();
    let mut graph = graph(&[&f.merge, &f.tail, &f.outer, &f.left, &f.right]);
    assert_eq!(
        graph.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
    graph.ingest_commit(f.root.clone(), ());
    assert_eq!(
        graph.as_sedimentree().summary(),
        expected(
            &[&f.tail],
            vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge])]
        ),
    );
}

#[test]
fn summaries_are_independent_of_arrival_order_and_duplicate_ingestion() {
    let f = nested();
    let mut commits = [&f.root, &f.left, &f.right, &f.merge, &f.outer, &f.tail];
    let expected = expected(
        &[&f.tail],
        vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge])],
    );
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    for _ in 0..40 {
        commits.shuffle(&mut rng);
        let mut graph = graph(&commits);
        assert_eq!(graph.as_sedimentree().summary(), expected);
        for meta in commits {
            graph.ingest_commit(meta.clone(), ());
            assert_eq!(graph.as_sedimentree().summary(), expected);
        }
    }
}

#[test]
fn unsigned_local_history_and_its_descendants_are_withheld_until_publishable() {
    let f = nested();
    let mut graph = CommitGraph::new();
    graph.ingest_local_commit(f.root.clone(), ());
    // Received descendants must not expose withheld local ancestry either.
    graph.ingest_commit(f.left.clone(), ());
    graph.ingest_commit(f.right.clone(), ());
    graph.ingest_commit(f.merge.clone(), ());
    graph.ingest_commit(f.outer.clone(), ());
    assert_eq!(
        graph.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
    sign(&mut graph, &key(1), hash(&f.root));
    assert_eq!(
        graph.as_sedimentree().summary(),
        expected(
            &[],
            vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge]),]
        )
    );
    let draft = commit(&[hash(&f.outer)], 0, "draft");
    let before = graph.as_sedimentree().summary();
    graph.ingest_local_commit(draft, ());
    assert_eq!(graph.as_sedimentree().summary(), before);
}

#[test]
fn summary_uses_existing_publication_policy_not_blob_local_proof_requirements() {
    let root = commit(&[], 0, "signed ancestor");
    let tip = commit(&[hash(&root)], 0, "signed tip");
    let mut graph = CommitGraph::new();
    graph.ingest_local_commit(root.clone(), ());
    graph.ingest_local_commit(tip.clone(), ());
    sign(&mut graph, &key(1), hash(&tip));
    // Global author coverage publishes this history. This structural summary
    // does NOT claim the unsigned ancestor can be served as a sealed loose blob.
    assert_eq!(
        graph.as_sedimentree().summary(),
        expected(&[&root, &tip], vec![])
    );
}

#[test]
fn signatures_before_delivery_and_after_delivery_produce_the_same_summary() {
    let f = nested();
    let mut before = CommitGraph::new();
    let mut after = CommitGraph::new();
    sign(&mut before, &key(1), hash(&f.outer));
    assert_eq!(
        before.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
    for meta in [&f.outer, &f.merge, &f.right, &f.left, &f.root] {
        before.ingest_local_commit(meta.clone(), ());
        after.ingest_local_commit(meta.clone(), ());
    }
    assert_eq!(
        after.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
    sign(&mut after, &key(1), hash(&f.outer));
    assert_eq!(
        before.as_sedimentree().summary(),
        after.as_sedimentree().summary()
    );
    assert_eq!(
        after.as_sedimentree().summary(),
        expected(
            &[],
            vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge]),]
        )
    );
}

#[test]
fn another_authors_signature_cannot_publish_a_local_commit() {
    let root = commit(&[], 0, "alice local root");
    let bob = key(2);
    let bob_tip = CommitMeta::new(CommitContents {
        parents: vec![hash(&root)],
        author: author(&bob),
        payload: BlobHash::of_contents(b"bob tip"),
    });
    let mut graph = CommitGraph::new();
    graph.ingest_local_commit(root.clone(), ());
    graph.ingest_local_commit(bob_tip.clone(), ());
    sign(&mut graph, &bob, hash(&bob_tip));
    assert_eq!(
        graph.as_sedimentree().summary(),
        SedimentreeSummary::default()
    );
    sign(&mut graph, &key(1), hash(&root));
    assert_eq!(
        graph.as_sedimentree().summary(),
        self::graph(&[&root, &bob_tip]).as_sedimentree().summary(),
    );
}

#[test]
fn permission_changes_and_viewpoints_do_not_change_the_summary() {
    let f = nested();
    let mut graph = graph(&[&f.root, &f.left, &f.right, &f.merge, &f.outer]);
    sign(&mut graph, &key(1), hash(&f.outer));
    let summary = graph.as_sedimentree().summary();
    for permission in [
        WritePermission::Unbounded,
        WritePermission::Bounded(vec![hash(&f.root)]),
        WritePermission::Denied,
    ] {
        graph.set_write_permissions(&HashMap::from([(author(&key(1)), permission)]));
        let _view = graph.view_point();
        let _resolved = graph.resolve(&graph.signed_view_point()).unwrap();
        assert_eq!(graph.as_sedimentree().summary(), summary);
    }
    graph.set_write_permissions(&HashMap::new());
    assert_eq!(graph.as_sedimentree().summary(), summary);
}

#[test]
fn graph_merge_and_clone_keep_indexes_consistent_and_independent() {
    let f = nested();
    let mut left = graph(&[&f.root, &f.left]);
    let right = graph(&[&f.root, &f.right, &f.merge, &f.outer]);
    left.merge(&right);
    let expected = expected(
        &[],
        vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge])],
    );
    assert_eq!(left.as_sedimentree().summary(), expected);
    let mut cloned = left.clone();
    cloned.ingest_commit(f.tail.clone(), ());
    assert_eq!(left.as_sedimentree().summary(), expected);
    assert_eq!(
        cloned.as_sedimentree().summary().loose_commits,
        vec![loose(&f.tail)]
    );
    left.merge(&cloned);
    assert_eq!(
        left.as_sedimentree().summary(),
        cloned.as_sedimentree().summary()
    );
}

#[test]
fn summary_needs_no_payload_contents_or_clones() {
    struct OpaquePayload;
    impl Clone for OpaquePayload {
        fn clone(&self) -> Self {
            panic!("summary must not clone payloads")
        }
    }
    let f = nested();
    let mut graph = CommitGraph::new();
    for meta in [&f.root, &f.left, &f.right, &f.merge, &f.outer] {
        graph.ingest_commit(meta.clone(), OpaquePayload);
    }
    let view = graph.as_sedimentree();
    let summary = view.summary();
    assert_eq!(summary, view.summary());
    assert_eq!(
        summary,
        expected(
            &[],
            vec![fragment(&f.outer, &[], &[&f.left, &f.right, &f.merge])]
        )
    );
    let interior: BTreeSet<_> = summary.fragments[0].checkpoints.iter().copied().collect();
    assert_eq!(interior.len(), 3);
}
