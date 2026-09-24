//! Graph topology tests built from local signed commit fixtures.
//!
//! Arbitrary and reverse insertion orders exercise the local causal queue, not
//! verified receipt: remote admission requires already-authenticated parents.
//! Delivered fixtures are fully signed and can be exchanged by graph merges.
mod support;

use std::ops::ControlFlow;

use stockfoil::{BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta};
use support::{key, sign};

fn meta(value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents: parents.to_vec(),
        author: support::author(&key(1)),
        payload: BlobHash::of_contents(&[value]),
    })
}

/// Sign known local work, including when parents are still missing.
/// These topology fixtures deliberately use the retained local causal queue.
fn ingest_local_signed(graph: &mut CommitGraph<u8>, meta: CommitMeta, value: u8) {
    let hash = *meta.hash();
    graph.ingest_local_commit(meta, value);
    if !graph.has_admitted_commit(&hash) {
        sign(graph, &key(1), hash);
    }
}

/// Insert a local signed graph topology fixture, not a received verified item.
fn insert(graph: &mut CommitGraph<u8>, value: u8, parents: &[CommitHash]) -> CommitHash {
    let meta = meta(value, parents);
    let hash = *meta.hash();
    ingest_local_signed(graph, meta, value);
    hash
}

fn visited(graph: &CommitGraph<u8>) -> Vec<u8> {
    let mut values = Vec::new();
    graph.traverse(|_, value| {
        values.push(*value);
        ControlFlow::Continue(())
    });
    values
}

fn visited_since(graph: &CommitGraph<u8>, since: &[CommitHash]) -> Vec<u8> {
    let mut values = Vec::new();
    graph.traverse_since(since, |_, value| {
        values.push(*value);
        ControlFlow::Continue(())
    });
    values
}

fn assert_heads(graph: &CommitGraph<u8>, expected: &[CommitHash]) {
    let actual = graph
        .delivered_heads()
        .map(|hash| *hash.as_bytes())
        .collect::<Vec<_>>();
    let mut expected = expected
        .iter()
        .map(|hash| *hash.as_bytes())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn heads_follow_chains_forks_merges_and_disconnected_roots() {
    let mut graph = CommitGraph::new();
    assert_heads(&graph, &[]);
    assert_eq!(graph.roots().count(), 0);
    assert!(visited(&graph).is_empty());
    let root = insert(&mut graph, 1, &[]);
    assert_heads(&graph, &[root]);
    let left = insert(&mut graph, 2, &[root]);
    assert_heads(&graph, &[left]);
    let right = insert(&mut graph, 3, &[root]);
    assert_heads(&graph, &[left, right]);
    let unrelated = insert(&mut graph, 4, &[]);
    assert_heads(&graph, &[left, right, unrelated]);
    let merge = insert(&mut graph, 5, &[left, right]);
    assert_heads(&graph, &[merge, unrelated]);

    // Reingesting a non-head must not make it a head again.
    insert(&mut graph, 1, &[]);
    insert(&mut graph, 2, &[root]);
    insert(&mut graph, 5, &[left, right]);
    assert_heads(&graph, &[merge, unrelated]);

    let tip = insert(&mut graph, 6, &[merge, merge]);
    assert_heads(&graph, &[tip, unrelated]);
}

#[test]
fn heads_change_only_when_local_queued_commits_become_ready() {
    let mut graph = CommitGraph::new();
    let first = *meta(1, &[]).hash();
    let second = *meta(2, &[]).hash();
    let merge = *meta(3, &[first, second]).hash();
    let tip = insert(&mut graph, 4, &[merge]);
    insert(&mut graph, 3, &[first, second]);
    insert(&mut graph, 3, &[first, second]);
    assert_heads(&graph, &[]);

    insert(&mut graph, 1, &[]);
    assert_heads(&graph, &[first]);
    let unrelated = insert(&mut graph, 5, &[]);
    assert_heads(&graph, &[first, unrelated]);

    // This releases the local merge and its descendant, even though the
    // descendant was queued first. Neither intermediate commit remains a head.
    insert(&mut graph, 2, &[]);
    assert_heads(&graph, &[tip, unrelated]);
}

#[test]
fn chain_preserves_metadata_payloads_and_edges() {
    let mut graph = CommitGraph::new();
    let first = insert(&mut graph, 1, &[]);
    let second = insert(&mut graph, 2, &[first]);
    let third = insert(&mut graph, 3, &[second]);

    let roots = graph.roots().collect::<Vec<_>>();
    assert_eq!(roots.len(), 1);
    let root = roots[0];
    assert_eq!(graph.parents(root).count(), 0);
    let children = graph.children(root).collect::<Vec<_>>();
    assert_eq!(children.len(), 1);
    let child = children[0];
    assert_eq!(graph.parents(child).collect::<Vec<_>>(), vec![root]);
    let grandchildren = graph.children(child).collect::<Vec<_>>();
    assert_eq!(grandchildren.len(), 1);
    let grandchild = grandchildren[0];
    assert_eq!(graph.parents(grandchild).collect::<Vec<_>>(), vec![child]);
    assert_eq!(graph.children(grandchild).count(), 0);

    let mut hashes = Vec::new();
    graph.traverse(|meta, value| {
        hashes.push(*meta.hash());
        assert_eq!(*value as usize, hashes.len());
        ControlFlow::Continue(())
    });
    assert!(hashes == vec![first, second, third]);
}

#[test]
fn merge_is_child_of_every_parent_and_visited_once() {
    let mut graph = CommitGraph::new();
    let root_hash = insert(&mut graph, 1, &[]);
    let left_hash = insert(&mut graph, 2, &[root_hash]);
    let right_hash = insert(&mut graph, 3, &[root_hash]);
    let merge_hash = insert(&mut graph, 4, &[left_hash, right_hash]);
    insert(&mut graph, 5, &[merge_hash]);

    let root = graph.roots().next().unwrap();
    let branches = graph.children(root).collect::<Vec<_>>();
    assert_eq!(branches.len(), 2);
    let merges = graph.children(branches[0]).collect::<Vec<_>>();
    assert_eq!(merges.len(), 1);
    assert_eq!(graph.children(branches[1]).collect::<Vec<_>>(), merges);
    assert_eq!(graph.parents(merges[0]).collect::<Vec<_>>(), branches);
    assert_eq!(visited(&graph), vec![1, 2, 3, 4, 5]);
}

#[test]
fn roots_keep_their_indices_when_interspersed_with_children() {
    let mut graph = CommitGraph::new();
    let first = insert(&mut graph, 1, &[]);
    insert(&mut graph, 2, &[first]);
    let second = insert(&mut graph, 3, &[]);
    insert(&mut graph, 4, &[first, second]);

    let roots = graph.roots().collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    for root in &roots {
        assert_eq!(graph.parents(*root).count(), 0);
    }
    assert_eq!(graph.children(roots[0]).count(), 2);
    let children = graph.children(roots[1]).collect::<Vec<_>>();
    assert_eq!(children.len(), 1);
    assert_eq!(graph.parents(children[0]).collect::<Vec<_>>(), roots);
    assert_eq!(visited(&graph), vec![1, 2, 3, 4]);
}

#[test]
fn duplicate_ingestion_preserves_original_payload_and_edges() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    let child = insert(&mut graph, 2, &[root]);
    ingest_local_signed(&mut graph, meta(1, &[]), 99);
    ingest_local_signed(&mut graph, meta(2, &[root]), 99);
    insert(&mut graph, 3, &[child]);

    assert_eq!(graph.roots().count(), 1);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 1);
    assert_eq!(visited(&graph), vec![1, 2, 3]);
}

#[test]
fn repeated_parent_does_not_duplicate_children_or_visits() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    insert(&mut graph, 2, &[root, root]);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 1);
    assert_eq!(visited(&graph), vec![1, 2]);
}

#[test]
fn merge_waits_for_all_parents_then_releases_descendants() {
    let mut graph = CommitGraph::new();
    let first = *meta(1, &[]).hash();
    let second = *meta(2, &[]).hash();
    let merge = insert(&mut graph, 3, &[first, second]);
    insert(&mut graph, 4, &[merge]);
    assert!(visited(&graph).is_empty());
    assert_eq!(graph.roots().count(), 0);

    insert(&mut graph, 1, &[]);
    assert_eq!(visited(&graph), vec![1]);
    assert_eq!(graph.roots().count(), 1);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 0);

    insert(&mut graph, 2, &[]);
    assert_eq!(visited(&graph), vec![1, 2, 3, 4]);
    let roots = graph.roots().collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    let merges = graph.children(roots[0]).collect::<Vec<_>>();
    assert_eq!(merges.len(), 1);
    assert_eq!(graph.children(roots[1]).collect::<Vec<_>>(), merges);
    assert_eq!(graph.parents(merges[0]).collect::<Vec<_>>(), roots);
    assert_eq!(graph.children(merges[0]).count(), 1);
}

#[test]
fn local_signed_queued_duplicates_preserve_first_payload() {
    let mut graph = CommitGraph::new();
    let root = *meta(1, &[]).hash();
    insert(&mut graph, 2, &[root, root]);
    ingest_local_signed(&mut graph, meta(2, &[root, root]), 99);
    assert!(visited(&graph).is_empty());

    insert(&mut graph, 1, &[]);
    ingest_local_signed(&mut graph, meta(2, &[root, root]), 100);
    assert_eq!(visited(&graph), vec![1, 2]);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 1);
}

#[test]
fn ancestry_is_strict_and_transitive() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    let child = insert(&mut graph, 2, &[root]);
    let tip = insert(&mut graph, 3, &[child]);

    assert!(graph.is_ancestor(&root, &child));
    assert!(graph.is_ancestor(&child, &tip));
    assert!(graph.is_ancestor(&root, &tip));
    assert!(!graph.is_ancestor(&tip, &root));
    assert!(!graph.is_ancestor(&child, &root));
    for hash in [root, child, tip] {
        assert!(!graph.is_ancestor(&hash, &hash));
    }
}

#[test]
fn ancestry_follows_all_merge_parents_but_not_concurrent_branches() {
    let mut graph = CommitGraph::new();
    let unrelated = insert(&mut graph, 0, &[]);
    let root = insert(&mut graph, 1, &[]);
    let left = insert(&mut graph, 2, &[root]);
    let right = insert(&mut graph, 3, &[root]);
    let merge = insert(&mut graph, 4, &[left, right, right]);
    let tip = insert(&mut graph, 5, &[merge]);

    for ancestor in [root, left, right, merge] {
        assert!(graph.is_ancestor(&ancestor, &tip));
    }
    assert!(!graph.is_ancestor(&left, &right));
    assert!(!graph.is_ancestor(&right, &left));
    assert!(!graph.is_ancestor(&unrelated, &tip));
    assert!(!graph.is_ancestor(&tip, &unrelated));
}

#[test]
fn ancestry_ignores_unknown_and_queued_commits_until_ready() {
    let mut graph = CommitGraph::new();
    let missing = *meta(1, &[]).hash();
    assert!(!graph.is_ancestor(&missing, &missing));
    let root = insert(&mut graph, 0, &[]);
    let queued = insert(&mut graph, 2, &[root, missing]);
    let tip = insert(&mut graph, 3, &[queued]);

    assert!(!graph.is_ancestor(&root, &missing));
    assert!(!graph.is_ancestor(&missing, &root));
    assert!(!graph.is_ancestor(&root, &queued));
    assert!(!graph.is_ancestor(&queued, &root));
    assert!(!graph.is_ancestor(&queued, &tip));

    insert(&mut graph, 1, &[]);
    assert!(graph.is_ancestor(&root, &tip));
    assert!(graph.is_ancestor(&missing, &tip));
    assert!(graph.is_ancestor(&queued, &tip));
}

#[test]
fn traverse_since_excludes_heads_and_all_their_ancestors() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    let left = insert(&mut graph, 2, &[root]);
    let right = insert(&mut graph, 3, &[root]);
    let merge = insert(&mut graph, 4, &[left, right]);
    let tip = insert(&mut graph, 5, &[merge]);

    assert_eq!(visited_since(&graph, &[merge]), vec![5]);
    assert_eq!(visited_since(&graph, &[left, right]), vec![4, 5]);
    assert_eq!(
        visited_since(&graph, &[root, left, left, right]),
        vec![4, 5]
    );
    assert!(visited_since(&graph, &[tip]).is_empty());
    // Earlier insertion does not imply ancestry: left is concurrent with right.
    assert_eq!(visited_since(&graph, &[right]), vec![2, 4, 5]);
}

#[test]
fn traverse_since_delivers_only_new_history_after_merge() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    let mut other = graph.clone();
    insert(&mut graph, 2, &[root]);
    let before = graph.delivered_heads().collect::<Vec<_>>();
    let branch = insert(&mut other, 3, &[root]);
    insert(&mut other, 4, &[branch]);
    insert(&mut other, 5, &[]);

    graph.merge(&other);
    let visited = visited_since(&graph, &before);
    // Transport order may differ from the sender's insertion order for
    // concurrent branches, but parents still precede their children.
    assert!(visited.iter().position(|&n| n == 3) < visited.iter().position(|&n| n == 4));
    let mut contents = visited;
    contents.sort_unstable();
    assert_eq!(contents, vec![3, 4, 5]);
    assert!(visited_since(&graph, &graph.delivered_heads().collect::<Vec<_>>()).is_empty());
}

#[test]
fn traverse_since_handles_empty_unknown_and_queued_heads() {
    let mut graph = CommitGraph::new();
    let unknown = *meta(99, &[]).hash();
    assert!(visited_since(&graph, &[]).is_empty());
    assert!(visited_since(&graph, &[unknown]).is_empty());
    let root = insert(&mut graph, 1, &[]);
    let child = insert(&mut graph, 2, &[root]);
    let queued = insert(&mut graph, 3, &[child, unknown]);

    assert_eq!(visited_since(&graph, &[]), visited(&graph));
    assert_eq!(visited_since(&graph, &[unknown, queued]), vec![1, 2]);
    assert_eq!(visited_since(&graph, &[root, unknown]), vec![2]);
}

#[test]
fn traverse_since_honors_break() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    let child = insert(&mut graph, 2, &[root]);
    insert(&mut graph, 3, &[child]);
    insert(&mut graph, 4, &[]);

    let mut values = Vec::new();
    graph.traverse_since(&[child], |_, value| {
        values.push(*value);
        ControlFlow::Break(())
    });
    assert_eq!(values, vec![3]);
    assert_eq!(visited_since(&graph, &[child]), vec![3, 4]);
}

#[test]
fn break_stops_the_entire_traversal() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 1, &[]);
    insert(&mut graph, 2, &[root]);
    insert(&mut graph, 3, &[]);

    let mut values = Vec::new();
    graph.traverse(|_, value| {
        values.push(*value);
        ControlFlow::Break(())
    });
    assert_eq!(values, vec![1]);
    // Stopping one traversal does not affect subsequent traversals.
    assert_eq!(visited(&graph), vec![1, 2, 3]);
}
