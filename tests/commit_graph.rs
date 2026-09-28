//! Graph topology tests built from local signed commit fixtures.
//!
//! Local insertion requires delivered parents; callers retry rejected children.
//! Draft parents need not be signed, while verified receipt requires admitted
//! dependencies. Signed fixtures can be exchanged by graph merges.
mod support;

use std::{
    ops::ControlFlow,
    panic::{AssertUnwindSafe, catch_unwind},
};

use stockfoil::{BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, SignatureError};
use support::{key, sign};

fn meta(value: u8, parents: &[CommitHash]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents: parents.to_vec(),
        author: support::author(&key(1)),
        payload: BlobHash::of_contents(&[value]),
    })
}

/// Sign local work after all of its parents have been delivered.
fn ingest_local_signed(graph: &mut CommitGraph<u8>, meta: CommitMeta, value: u8) {
    let hash = *meta.hash();
    graph.ingest_local_commit(meta, value);
    if !graph.has_commit(&hash) {
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

/// A missing parent is a caller error, not a request to retain local work.
fn reject_local(graph: &mut CommitGraph<u8>, meta: CommitMeta, value: u8) -> CommitHash {
    let hash = *meta.hash();
    assert!(catch_unwind(AssertUnwindSafe(|| graph.ingest_local_commit(meta, value))).is_err());
    assert!(!graph.has_commit(&hash));
    assert_eq!(
        graph.ingest_signature(
            support::author(&key(1)),
            hash,
            support::signature(&key(1), hash)
        ),
        Err(SignatureError::NotPendingLocalCommit(hash))
    );
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
fn rejected_children_change_heads_only_after_explicit_parents_first_retry() {
    let mut graph = CommitGraph::new();
    let first = *meta(1, &[]).hash();
    let second = *meta(2, &[]).hash();
    let merge = *meta(3, &[first, second]).hash();
    let tip = reject_local(&mut graph, meta(4, &[merge]), 40);
    reject_local(&mut graph, meta(3, &[first, second]), 30);
    reject_local(&mut graph, meta(3, &[first, second]), 99);
    assert_heads(&graph, &[]);
    assert_eq!(graph.roots().count(), 0);
    assert!(visited(&graph).is_empty());

    insert(&mut graph, 1, &[]);
    assert_heads(&graph, &[first]);
    let unrelated = insert(&mut graph, 5, &[]);
    assert_heads(&graph, &[first, unrelated]);
    insert(&mut graph, 2, &[]);
    // Inserting parents does not resurrect either rejected payload.
    assert_heads(&graph, &[first, second, unrelated]);
    assert_eq!(visited(&graph), vec![1, 5, 2]);

    insert(&mut graph, 3, &[first, second]);
    assert_heads(&graph, &[merge, unrelated]);
    assert_eq!(visited(&graph), vec![1, 5, 2, 3]);
    insert(&mut graph, 4, &[merge]);
    assert_heads(&graph, &[tip, unrelated]);
    assert_eq!(visited(&graph), vec![1, 5, 2, 3, 4]);
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
    let first = insert(&mut graph, 1, &[]);
    let second = insert(&mut graph, 2, &[]);
    insert(&mut graph, 3, &[second, first, second]);
    let roots = graph.roots().collect::<Vec<_>>();
    let child = graph.children(roots[0]).next().unwrap();
    // Parent slices preserve envelope order and duplicates, not node-index order.
    assert_eq!(
        graph.parents(child).collect::<Vec<_>>(),
        vec![roots[1], roots[0], roots[1]]
    );
    for root in roots {
        assert_eq!(graph.children(root).collect::<Vec<_>>(), vec![child]);
    }
    assert_eq!(visited(&graph), vec![1, 2, 3]);
}

#[test]
fn children_index_grows_in_insertion_order_and_clones_independently() {
    let mut graph = CommitGraph::new();
    let root_hash = insert(&mut graph, 1, &[]);
    let root = graph.roots().next().unwrap().clone();
    assert_eq!(graph.children(&root).count(), 0);
    insert(&mut graph, 2, &[root_hash]);
    let first = graph.children(&root).next().unwrap().clone();
    let mut cloned = graph.clone();

    // Interleave an unrelated root and repeat parents on a later child.
    insert(&mut graph, 3, &[]);
    insert(&mut graph, 4, &[root_hash, root_hash]);
    let children = graph.children(&root).collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], &first);
    assert!(children[0] < children[1]);
    assert_eq!(cloned.children(&root).collect::<Vec<_>>(), vec![&first]);

    insert(&mut cloned, 5, &[root_hash]);
    insert(&mut cloned, 6, &[root_hash]);
    assert_eq!(cloned.children(&root).count(), 3);
    assert_eq!(graph.children(&root).collect::<Vec<_>>(), children);
}

#[test]
fn missing_later_parent_rejects_before_any_graph_mutation() {
    let mut graph = CommitGraph::new();
    let root = insert(&mut graph, 0, &[]);
    let first = insert(&mut graph, 1, &[root]);
    let second = *meta(2, &[]).hash();
    let roots_before = graph.roots().cloned().collect::<Vec<_>>();
    let first_commit_hash = graph.children(&roots_before[0]).next().unwrap().clone();
    let heads_before = graph.heads().collect::<Vec<_>>();
    let cover_before = graph.minimal_cover();
    let working_before = graph.view_point();
    let signed_before = graph.signed_view_point();
    let mut metadata_before = Vec::new();
    graph.traverse(|meta, value| {
        metadata_before.push((meta.clone(), *value));
        ControlFlow::Continue(())
    });

    // The first parent exists: even parent links and heads must remain untouched
    // when validation encounters the missing second parent.
    let merge = reject_local(&mut graph, meta(3, &[first, second]), 30);
    assert_eq!(graph.heads().collect::<Vec<_>>(), heads_before);
    assert_eq!(graph.roots().cloned().collect::<Vec<_>>(), roots_before);
    assert_eq!(
        graph.children(&roots_before[0]).cloned().collect::<Vec<_>>(),
        vec![first_commit_hash]
    );
    assert_eq!(graph.parents(&first_commit_hash).cloned().collect::<Vec<_>>(), roots_before);
    assert_eq!(graph.children(&first_commit_hash).count(), 0);
    assert_eq!(visited(&graph), vec![0, 1]);
    assert_eq!(visited_since(&graph, &[root]), vec![1]);
    let mut metadata_after = Vec::new();
    graph.traverse(|meta, value| {
        assert_ne!(*meta.hash(), merge);
        metadata_after.push((meta.clone(), *value));
        ControlFlow::Continue(())
    });
    assert_eq!(metadata_after.len(), metadata_before.len());
    for ((after, after_value), (before, before_value)) in
        metadata_after.iter().zip(&metadata_before)
    {
        assert_eq!(after.hash(), before.hash());
        assert_eq!(after.parents(), before.parents());
        assert_eq!(after.author(), before.author());
        assert_eq!(after.payload_hash(), before.payload_hash());
        assert_eq!(after_value, before_value);
    }
    assert!(graph.publishable_commit(&merge).is_none());
    assert_eq!(graph.minimal_cover(), cover_before);
    assert_eq!(graph.view_point(), working_before);
    assert_eq!(graph.signed_view_point(), signed_before);
    assert!(graph.resolve(&signed_before).is_ok());

    insert(&mut graph, 2, &[]);
    assert_eq!(visited(&graph), vec![0, 1, 2]);
    assert_heads(&graph, &[first, second]);
    assert_eq!(graph.children(&first_commit_hash).count(), 0);
    assert!(!graph.has_commit(&merge));
    insert(&mut graph, 3, &[first, second]);
    insert(&mut graph, 4, &[merge]);
    assert_eq!(visited(&graph), vec![0, 1, 2, 3, 4]);
    let roots = graph.roots().collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    let merges = graph.children(&first_commit_hash).collect::<Vec<_>>();
    assert_eq!(merges.len(), 1);
    assert_eq!(graph.children(roots[1]).collect::<Vec<_>>(), merges);
    assert_eq!(
        graph.parents(merges[0]).collect::<Vec<_>>(),
        vec![&first_commit_hash, roots[1]]
    );
    assert_eq!(graph.children(merges[0]).count(), 1);
}

#[test]
fn local_draft_parents_are_sufficient_and_pending_duplicates_preserve_first_payload() {
    let mut graph = CommitGraph::new();
    let root_meta = meta(1, &[]);
    let root = *root_meta.hash();
    graph.ingest_local_commit(root_meta.clone(), 1);
    graph.ingest_local_commit(root_meta, 99);
    let child_meta = meta(2, &[root, root]);
    let child = *child_meta.hash();
    graph.ingest_local_commit(child_meta.clone(), 2);
    graph.ingest_local_commit(child_meta.clone(), 99);
    assert_eq!(visited(&graph), vec![1, 2]);
    assert_heads(&graph, &[child]);
    assert!(!graph.has_commit(&root));
    assert!(!graph.has_commit(&child));
    assert!(graph.signed_view_point().heads().is_empty());

    // Signing a child before its delivered draft parent is allowed, but does
    // not admit it. Duplicates must preserve both payload and signature.
    sign(&mut graph, &key(1), child);
    graph.ingest_local_commit(child_meta, 100);
    assert!(!graph.has_commit(&child));
    sign(&mut graph, &key(1), root);
    assert!(graph.has_commit(&root));
    assert!(graph.has_commit(&child));
    assert_eq!(visited(&graph), vec![1, 2]);
    assert_eq!(graph.children(graph.roots().next().unwrap()).count(), 1);
}

#[test]
fn signature_before_local_insertion_is_rejected_not_retained() {
    let mut graph = CommitGraph::new();
    let root_meta = meta(1, &[]);
    let root = *root_meta.hash();
    assert_eq!(
        graph.ingest_signature(
            support::author(&key(1)),
            root,
            support::signature(&key(1), root)
        ),
        Err(SignatureError::NotPendingLocalCommit(root))
    );
    assert!(visited(&graph).is_empty());
    graph.ingest_local_commit(root_meta, 1);
    assert!(!graph.has_commit(&root));
    assert!(graph.signed_view_point().heads().is_empty());
    sign(&mut graph, &key(1), root);
    assert!(graph.has_commit(&root));
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
fn ancestry_ignores_rejected_commits_until_explicit_retry() {
    let mut graph = CommitGraph::new();
    let missing = *meta(1, &[]).hash();
    assert!(!graph.is_ancestor(&missing, &missing));
    let root = insert(&mut graph, 0, &[]);
    let rejected = reject_local(&mut graph, meta(2, &[root, missing]), 2);
    let tip = reject_local(&mut graph, meta(3, &[rejected]), 3);

    assert!(!graph.is_ancestor(&root, &missing));
    assert!(!graph.is_ancestor(&missing, &root));
    assert!(!graph.is_ancestor(&root, &rejected));
    assert!(!graph.is_ancestor(&rejected, &root));
    assert!(!graph.is_ancestor(&rejected, &tip));

    insert(&mut graph, 1, &[]);
    assert!(!graph.is_ancestor(&root, &rejected));
    assert!(!graph.is_ancestor(&missing, &tip));
    insert(&mut graph, 2, &[root, missing]);
    assert!(graph.is_ancestor(&root, &rejected));
    assert!(!graph.is_ancestor(&rejected, &tip));
    insert(&mut graph, 3, &[rejected]);
    assert!(graph.is_ancestor(&root, &tip));
    assert!(graph.is_ancestor(&missing, &tip));
    assert!(graph.is_ancestor(&rejected, &tip));
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
    let before = graph.heads().collect::<Vec<_>>();
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
    assert!(visited_since(&graph, &graph.heads().collect::<Vec<_>>()).is_empty());
}

#[test]
fn traverse_since_handles_empty_unknown_and_rejected_heads() {
    let mut graph = CommitGraph::new();
    let unknown = *meta(99, &[]).hash();
    assert!(visited_since(&graph, &[]).is_empty());
    assert!(visited_since(&graph, &[unknown]).is_empty());
    let root = insert(&mut graph, 1, &[]);
    let child = insert(&mut graph, 2, &[root]);
    let rejected = reject_local(&mut graph, meta(3, &[child, unknown]), 3);

    assert_eq!(visited_since(&graph, &[]), visited(&graph));
    assert_eq!(visited_since(&graph, &[unknown, rejected]), vec![1, 2]);
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
