use std::ops::ControlFlow;

use stockfoil::{BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta};

fn meta(value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents: parents.to_vec(),
        author: None,
        signatures: Vec::new(),
        payload: BlobHash::of_contents(&[value]),
    })
}

fn insert(graph: &mut CommitGraph<u8>, value: u8, parents: &[CommitHash]) -> CommitHash {
    let meta = meta(value, parents);
    let hash = *meta.hash();
    graph.ingest_commit(meta, value);
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

fn assert_heads(graph: &CommitGraph<u8>, expected: &[CommitHash]) {
    let actual = graph
        .heads()
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
fn heads_change_only_when_queued_commits_become_ready() {
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

    // This releases the merge and its descendant, even though the descendant
    // arrived first. Neither intermediate commit should remain a head.
    insert(&mut graph, 2, &[]);
    assert_heads(&graph, &[tip, unrelated]);
}

#[test]
fn empty_graph() {
    let graph = CommitGraph::<u8>::default();
    assert_eq!(graph.roots().count(), 0);
    assert!(visited(&graph).is_empty());
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
    graph.ingest_commit(meta(1, &[]), 99);
    graph.ingest_commit(meta(2, &[root]), 99);
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
fn reverse_order_chain_is_delivered_causally() {
    let mut graph = CommitGraph::new();
    let root = meta(1, &[]);
    let child = meta(2, &[*root.hash()]);
    let grandchild = meta(3, &[*child.hash()]);

    graph.ingest_commit(grandchild, 3);
    graph.ingest_commit(child, 2);
    assert!(visited(&graph).is_empty());
    assert_eq!(graph.roots().count(), 0);

    graph.ingest_commit(root, 1);
    assert_eq!(visited(&graph), vec![1, 2, 3]);
    let root = graph.roots().next().unwrap();
    let child = graph.children(root).next().unwrap();
    let grandchild = graph.children(child).next().unwrap();
    assert_eq!(graph.parents(child).collect::<Vec<_>>(), vec![root]);
    assert_eq!(graph.parents(grandchild).collect::<Vec<_>>(), vec![child]);
}

#[test]
fn queued_duplicates_preserve_first_payload() {
    let mut graph = CommitGraph::new();
    let root = *meta(1, &[]).hash();
    insert(&mut graph, 2, &[root, root]);
    graph.ingest_commit(meta(2, &[root, root]), 99);
    assert!(visited(&graph).is_empty());

    insert(&mut graph, 1, &[]);
    graph.ingest_commit(meta(2, &[root, root]), 100);
    assert_eq!(visited(&graph), vec![1, 2]);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 1);
}

#[test]
fn blocked_commits_do_not_block_unrelated_ready_commits() {
    let mut graph = CommitGraph::new();
    let missing = *meta(1, &[]).hash();
    insert(&mut graph, 2, &[missing]);
    let root = insert(&mut graph, 3, &[]);
    insert(&mut graph, 4, &[root]);
    assert_eq!(visited(&graph), vec![3, 4]);
    assert_eq!(graph.roots().count(), 1);

    insert(&mut graph, 1, &[]);
    assert_eq!(visited(&graph), vec![3, 4, 1, 2]);
    assert_eq!(graph.roots().count(), 2);
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
