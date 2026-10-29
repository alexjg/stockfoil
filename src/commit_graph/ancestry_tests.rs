use super::*;

// These fixtures exercise the ancestry index without signature or fragment
// maintenance. They are still parent-closed local drafts, never admitted data.
fn author(seed: u8) -> Author {
    let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

fn append(graph: &mut CommitGraph<()>, parents: &[NodeIdx]) -> NodeIdx {
    append_by(graph, author(1), parents)
}

fn append_by(graph: &mut CommitGraph<()>, author: Author, parents: &[NodeIdx]) -> NodeIdx {
    let idx = NodeIdx(graph.nodes.entries.len());
    let meta = (0u64..)
        .map(|nonce| {
            CommitMeta::new(CommitContents {
                author,
                parents: parents
                    .iter()
                    .map(|&p| *graph.nodes[p].meta.hash())
                    .collect(),
                payload: BlobHash::of_contents(
                    &[
                        idx.0.to_le_bytes().as_slice(),
                        nonce.to_le_bytes().as_slice(),
                    ]
                    .concat(),
                ),
            })
        })
        .find(|meta| sedimentree_depth(meta.hash()) == 0)
        .unwrap();
    graph.pending_local.insert(*meta.hash());
    graph.insert_commit(meta, ());
    idx
}

fn walk(nodes: &NodeStore, head: NodeIdx) -> HashSet<NodeIdx> {
    let mut seen = HashSet::new();
    let mut stack = vec![head];
    while let Some(idx) = stack.pop() {
        if seen.insert(idx) {
            stack.extend(nodes.parents(idx));
        }
    }
    seen
}

#[test]
fn repeated_diamonds_reuse_ancestral_not_just_parent_chain_tips() {
    let mut graph = CommitGraph::new();
    let root = append(&mut graph, &[]);
    let mut head = root;
    let mut old = None;
    for round in 0..512 {
        let left = append(&mut graph, &[head]);
        let right = append(&mut graph, &[head]);
        assert_ne!(graph.nodes[left].chain, graph.nodes[right].chain);
        if round > 0 {
            // Neither direct parent was a chain tip when right was inserted:
            // it reused the dormant tip on the other side of an older diamond.
            assert_ne!(graph.nodes[right].chain, graph.nodes[head].chain);
        }
        head = append(&mut graph, &[left, right]);
        assert_eq!(graph.nodes.chain_tips.len(), 2);
        assert_eq!(graph.nodes.clock(head).entries().count(), 2);
        if round == 0 {
            old = Some((head, Arc::clone(graph.nodes.clock(head))));
        }
    }
    let (old_head, old_clock) = old.unwrap();
    assert!(Arc::ptr_eq(&old_clock, graph.nodes.clock(old_head)));
    assert!(old_clock.contains(graph.nodes[root].chain, root));
    assert!(!old_clock.contains(graph.nodes[head].chain, head));
    assert_eq!(graph.nodes.clock(head).latest(), Some(head));
    // Even eagerly cached merge clocks stay two-dimensional throughout.
    assert!(
        graph
            .nodes
            .entries
            .iter()
            .filter_map(|n| n.clock.get())
            .all(|clock| clock.entries().count() <= 2)
    );
}

#[test]
fn alternating_authors_reuse_only_their_own_ancestral_chain_tips() {
    let mut graph = CommitGraph::new();
    let root = append_by(&mut graph, author(1), &[]);
    let mut tip = root;
    for i in 1..65 {
        tip = append_by(&mut graph, author(1 + (i % 2) as u8), &[tip]);
        assert_eq!(graph.nodes.chain_tips.len(), 2);
    }
    // Reuse must cross the other author's intervening commit, but never put
    // two authors on the same chain. NodeIdx(0) is still a real coordinate.
    assert_eq!(root, NodeIdx(0));
    assert_eq!(graph.nodes.chain_authors, vec![author(1), author(2)]);
    for (idx, node) in &graph.nodes {
        assert_eq!(graph.nodes.chain_authors[node.chain], *node.meta.author());
        assert_eq!(node.chain, idx.0 % 2);
        let expected = walk(&graph.nodes, idx);
        let clock = graph.nodes.clock(idx);
        for (other, other_node) in &graph.nodes {
            assert_eq!(clock.contains(other_node.chain, other), expected.contains(&other));
        }
    }
}

#[test]
fn same_author_forks_have_independent_permission_coordinates() {
    use crate::Vis;

    let alice = author(1);
    let bob = author(2);
    let mut graph = CommitGraph::new();
    let root = append_by(&mut graph, alice, &[]);
    let middle = append_by(&mut graph, bob, &[root]);
    let left = append_by(&mut graph, alice, &[middle]);
    let right = append_by(&mut graph, alice, &[middle]);
    let join = append_by(&mut graph, bob, &[left, right]);
    assert_eq!(graph.nodes[root].chain, graph.nodes[left].chain);
    assert_ne!(graph.nodes[left].chain, graph.nodes[right].chain);
    assert_eq!(graph.nodes[middle].chain, graph.nodes[join].chain);
    assert_eq!(graph.nodes.chain_authors, vec![alice, bob, alice]);
    let hash = |idx: NodeIdx| *graph.nodes[idx].hash();
    let view = |bounds| {
        ViewPoint::from_parts(
            vec![hash(join)],
            HashMap::from([
                (alice, WritePermission::Bounded(bounds)),
                (bob, WritePermission::Denied),
            ]),
        )
    };
    let all = ViewPoint::from_parts(
        vec![hash(join)],
        HashMap::from([
            (alice, WritePermission::Unbounded),
            (bob, WritePermission::Unbounded),
        ]),
    );
    let left_view = view(vec![hash(left)]);
    let right_view = view(vec![hash(right)]);
    let cached = Arc::clone(graph.nodes.clock(join));
    let masked = graph.participation(&left_view).unwrap();
    assert_eq!(
        masked.entries().collect::<Vec<_>>(),
        vec![(graph.nodes[left].chain, left)]
    );
    // The allowed descendant stays visible through Bob's denied middle node.
    let diff = graph.diff(&all, &left_view).unwrap();
    for idx in [root, left] {
        assert_eq!(diff.visibility(&hash(idx)), Ok(Vis::Both));
    }
    for idx in [middle, right, join] {
        assert_eq!(diff.visibility(&hash(idx)), Ok(Vis::Before));
    }
    let changes = diff.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().all(|c| c.visibility_delta == VisDiff::Before));
    assert_eq!(
        changes.iter().map(|c| c.commit_id).collect::<HashSet<_>>(),
        HashSet::from([hash(middle), hash(right), hash(join)])
    );
    let diff = graph.diff(&left_view, &right_view).unwrap();
    assert_eq!(diff.visibility(&hash(root)), Ok(Vis::Both));
    assert_eq!(diff.visibility(&hash(left)), Ok(Vis::Before));
    assert_eq!(diff.visibility(&hash(right)), Ok(Vis::After));
    assert_eq!(diff.visibility(&hash(middle)), Ok(Vis::Neither));
    assert_eq!(diff.visibility(&hash(join)), Ok(Vis::Neither));
    let both = view(vec![hash(left), hash(right), hash(left)]);
    let diff = graph.diff(&left_view, &both).unwrap();
    assert_eq!(diff.visibility(&hash(left)), Ok(Vis::Both));
    assert_eq!(diff.visibility(&hash(right)), Ok(Vis::After));

    // A different author's frontier can bound Alice's chains. A known bound
    // outside the selected history must grant nothing, even to its ancestors.
    let through_bob = view(vec![hash(join)]);
    let diff = graph.diff(&both, &through_bob).unwrap();
    assert_eq!(diff.changed_commits().count(), 0);
    let excluded_bound = ViewPoint::from_parts(
        vec![hash(right)],
        HashMap::from([(alice, WritePermission::Bounded(vec![hash(join)]))]),
    );
    let empty = graph.participation(&excluded_bound).unwrap();
    assert_eq!(empty.latest(), None);
    assert!(!empty.contains(graph.nodes[root].chain, root));

    // Masking never mutates the full ancestry cache or its cross-author edges.
    assert!(Arc::ptr_eq(&cached, graph.nodes.clock(join)));
    for (idx, node) in &graph.nodes {
        assert!(cached.contains(node.chain, idx));
    }
    assert!(graph.is_ancestor(&hash(middle), &hash(left)));
    assert!(!graph.is_ancestor(&hash(left), &hash(right)));
}

#[test]
fn masks_cap_or_remove_coordinates_without_mutating_history() {
    let history = AncestorClock::from_tips(&[NodeIdx(0), NodeIdx(5), NodeIdx(9)]);
    let masked = history.mask(|chain, _| match chain {
        0 => Some(NodeIdx(usize::MAX)),
        1 => None,
        2 => Some(NodeIdx(7)),
        _ => unreachable!(),
    });
    assert_eq!(
        masked.entries().collect::<Vec<_>>(),
        vec![(0, NodeIdx(0)), (2, NodeIdx(7))]
    );
    assert!(masked.contains(0, NodeIdx(0)));
    assert!(!masked.contains(1, NodeIdx(0)));
    assert!(!masked.contains(2, NodeIdx(8)));
    assert_eq!(masked.latest(), Some(NodeIdx(7)));
    assert_eq!(history.position(1), Some(NodeIdx(5)));
    assert_eq!(history.position(2), Some(NodeIdx(9)));
    let empty = history.mask(|_, _| None);
    assert_eq!(empty.latest(), None);
    assert!(!empty.contains(0, NodeIdx(0)));
    assert_eq!(AncestorClock::default().mask(|_, tip| Some(tip)).latest(), None);
}

#[test]
fn cached_clocks_are_exact_downsets_not_topological_prefixes() {
    let mut graph = CommitGraph::new();
    let root = append(&mut graph, &[]);
    assert_eq!(root, NodeIdx(0));
    let left = append(&mut graph, &[root]);
    let other_root = append(&mut graph, &[]);
    let right = append(&mut graph, &[root, other_root]);
    let join = append(&mut graph, &[left, right, right]);
    let late_fork = append(&mut graph, &[root]);
    let tip = append(&mut graph, &[join, late_fork]);
    for i in 0..graph.nodes.entries.len() {
        let head = NodeIdx(i);
        let expected = walk(&graph.nodes, head);
        let clock = graph.nodes.clock(head);
        for (idx, node) in &graph.nodes {
            assert_eq!(clock.contains(node.chain, idx), expected.contains(&idx));
        }
    }
    assert!(
        !graph
            .nodes
            .clock(right)
            .contains(graph.nodes[left].chain, left)
    );
    assert!(
        !graph
            .nodes
            .clock(root)
            .contains(graph.nodes[other_root].chain, other_root)
    );
    assert!(!graph.nodes.clock(root).contains(usize::MAX, NodeIdx(0)));
    assert!(
        !graph
            .nodes
            .clock(join)
            .contains(graph.nodes[late_fork].chain, late_fork)
    );
    assert_eq!(walk(&graph.nodes, tip).len(), graph.nodes.entries.len());
    // Multi-head union must not include the later join or the late fork.
    let clock = graph.nodes.clock_for_heads(&[right, left, right]);
    assert!(clock.contains(graph.nodes[other_root].chain, other_root));
    assert!(!clock.contains(graph.nodes[join].chain, join));
    assert!(!clock.contains(graph.nodes[late_fork].chain, late_fork));
}

#[test]
fn long_checkpoint_free_runs_have_bounded_walks_and_shared_query_caches() {
    let mut graph = CommitGraph::new();
    let root = append(&mut graph, &[]);
    let mut tip = root;
    for _ in 1..1023 {
        tip = append(&mut graph, &[tip]);
    }
    assert_eq!(graph.nodes.chain_tips.len(), 1);
    assert!(graph.nodes[tip].clock.get().is_none());
    assert_eq!(graph.nodes[tip].uncached_depth, 127);
    assert_eq!(
        graph
            .nodes
            .entries
            .iter()
            .filter(|n| n.clock.get().is_some())
            .count(),
        7
    );
    let clock = Arc::clone(graph.nodes.clock(tip));
    assert!(clock.contains(0, root));
    assert!(Arc::ptr_eq(&clock, graph.nodes.clock(tip)));
    let first = graph.nodes.clock_for_heads(&[tip]);
    let second = graph.nodes.clock_for_heads(&[tip]);
    assert!(Arc::ptr_eq(&first, &second));

    let mut fork = graph.clone();
    assert!(Arc::ptr_eq(&clock, fork.nodes.clock(tip)));
    let left = append(&mut graph, &[tip]);
    let right = append(&mut fork, &[root]);
    assert!(!graph.nodes.clock(left).contains(1, right));
    assert!(!fork.nodes.clock(right).contains(fork.nodes[tip].chain, tip));
    assert!(Arc::ptr_eq(&clock, fork.nodes.clock(tip)));
}

#[test]
fn current_history_clock_handles_duplicate_and_nonmaximal_heads() {
    let mut graph = CommitGraph::new();
    let a = append(&mut graph, &[]);
    let b = append(&mut graph, &[]);
    let a_hash = *graph.nodes[a].meta.hash();
    let b_hash = *graph.nodes[b].meta.hash();
    // Length alone must not mistake [a, a] for the complete working frontier.
    let clock = graph.history_clock(&[a_hash, a_hash]).unwrap();
    assert!(clock.contains(graph.nodes[a].chain, a));
    assert!(!clock.contains(graph.nodes[b].chain, b));
    let full = graph.history_clock(&[b_hash, a_hash, b_hash]).unwrap();
    assert!(full.contains(graph.nodes[a].chain, a));
    assert!(full.contains(graph.nodes[b].chain, b));
    let tip = append(&mut graph, &[a, b]);
    let clock = graph
        .history_clock(&[a_hash, *graph.nodes[tip].meta.hash()])
        .unwrap();
    assert!(clock.contains(graph.nodes[b].chain, b));
    assert!(clock.contains(graph.nodes[tip].chain, tip));
}

/// Opt-in comparison with the previous parent-walk ancestry implementation.
/// Run with: cargo test --release benchmark_ancestry -- --ignored --nocapture
#[test]
#[ignore]
fn benchmark_ancestry() {
    use std::{hint::black_box, time::Instant};
    let mut graph = CommitGraph::new();
    let mut head = append(&mut graph, &[]);
    let mut targets = Vec::new();
    for _ in 0..3000 {
        let left = append(&mut graph, &[head]);
        let right = append(&mut graph, &[head]);
        head = append(&mut graph, &[left, right]);
        if targets.len() < 1000 {
            targets.push(right);
        }
    }
    let descendant = *graph.nodes[head].meta.hash();
    let started = Instant::now();
    for &target in &targets {
        assert!(black_box(
            graph.is_ancestor(graph.nodes[target].meta.hash(), &descendant)
        ));
    }
    let indexed = started.elapsed();
    let started = Instant::now();
    for &target in &targets {
        let mut stack = vec![head];
        let mut visited = HashSet::new();
        let found = loop {
            let Some(idx) = stack.pop() else {
                break false;
            };
            if idx == target {
                break true;
            }
            if idx < target || !visited.insert(idx) {
                continue;
            }
            stack.extend(graph.nodes.parents(idx));
        };
        assert!(black_box(found));
    }
    std::eprintln!(
        "{} nodes, {} queries, {} chains: clocks {indexed:?}, parent walks {:?}",
        graph.nodes.entries.len(),
        targets.len(),
        graph.nodes.chain_tips.len(),
        started.elapsed()
    );
}
