//! Nonempty consumers establish cache predicates through the real append path.
use vstd::prelude::*;
use std::sync::Arc;
use crate::{ancestry_cache::{self, Cache}, ancestry_walk, author::Author, commit_hash::CommitHash,
    clock::NodeIdx, storage::{self, Node}, indexes::{HashIndex, AuthorIndex}, tree::Clock,
    storage_refinement::graph_of, graph};

verus! {

proof fn own_coordinate(clock: &Clock, g: graph::Graph, labels: Seq<u32>, head: int)
    requires 0 <= head < g.len(), labels.len() == g.len(),
        ancestry_walk::summarizes(clock.contents(), labels, graph::history(g, ISet::empty().insert(head))),
    ensures clock.contents().contains_key(labels[head]), clock.contents()[labels[head]]@ == head,
{
    let heads = ISet::empty().insert(head);
    graph::unfold_reaches(g, head, head);
    assert(heads.contains(head));
    assert(graph::history(g, heads).contains(head));
    let tip = clock.contents()[labels[head]]@;
    assert(graph::history(g, heads).contains(tip as int));
    let h = choose|h: int| heads.contains(h) && graph::reaches(g, tip as int, h);
    assert(h == head);
    graph::reaches_order(g, tip as int, head);
}

pub fn append_initialize_and_reuse_real_cells() {
    let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<CommitHash, u64, Cache>();
    let mut owners = Vec::new();
    let mut hashes = HashIndex::new();
    let mut authors = AuthorIndex::new();
    let author = Author([1u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let h2 = CommitHash([2u8; 32]);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && h2.0@[0] == 2);
    assert(h0 != h1 && h0 != h2 && h1 != h2);
    assert(hashes.matches(nodes@));
    let root = ancestry_cache::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &mut hashes, &mut authors, &vec![], Node::new(h0, 100, 0, 0, 1, ()), author);
    proof { storage::labels(nodes@).lemma_cardinality_of_set(); }
    let saved = Arc::clone(ancestry_cache::clock(&nodes, &ids, root));
    proof { own_coordinate(&saved, graph_of(nodes@, ids@), storage::labels(nodes@), root@ as int); }
    assert(saved.contents().contains_key(0));
    assert(saved.contents()[0]@ == 0);
    let ghost root_graph = graph_of(nodes@, ids@);
    let child = ancestry_cache::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &mut hashes, &mut authors, &vec![root, root], Node::new(h1, 200, 0, 0, 2, ()), author);
    proof { storage::labels(nodes@).lemma_cardinality_of_set(); }
    let restored = ancestry_cache::clock(&nodes, &ids, root);
    proof { own_coordinate(restored, graph_of(nodes@, ids@), storage::labels(nodes@), root@ as int); }
    assert(restored.contents().contains_key(0));
    assert(restored.contents()[0]@ == 0);
    let latest = Arc::clone(ancestry_cache::clock(&nodes, &ids, child));
    proof {
        let g = graph_of(nodes@, ids@);
        graph::unfold_reaches(g, child@ as int, child@ as int);
        assert(ISet::empty().insert(child@ as int).contains(child@ as int));
        assert(graph::history(g, ISet::empty().insert(child@ as int)).contains(child@ as int));
    }
    assert(latest.contents().contains_key(0));
    assert(latest.contents()[0]@ == 1);
    let fork = ancestry_cache::append(&mut nodes, &mut ids, &mut tips, &mut tables, &mut owners,
        &mut hashes, &mut authors, &vec![root], Node::new(h2, 300, 1, 0, 2, ()), author);
    proof { storage::labels(nodes@).lemma_cardinality_of_set(); }
    let both = ancestry_cache::clock_for_heads(&nodes, &ids, &vec![child, fork, child]);
    let fork_clock = ancestry_cache::clock(&nodes, &ids, fork);
    assert(fork_clock.valid() && both.valid());
    assert(saved.contents()[0]@ == 0 && latest.contents()[0]@ == 1);
}

// The library boundary may preserve an arbitrary predicate. It must not silently
// grant an application fact, nor allow initializing a false-predicate cell.
pub fn generic_cell_has_no_ancestry_assumption() {
    let cell = crate::stdlib::OnceValue::<u64>::new(Ghost(|value: u64| value == 7));
    let init = || -> (out: u64) ensures out == 7, { 7 };
    let value = cell.get_or_init(init);
    assert(*value == 7);
    let again = cell.get_or_init(init);
    assert(*again == 7);
    let never = crate::stdlib::OnceValue::<u64>::new(Ghost(|_value: u64| false));
    let empty = never.get();
    assert(empty is None);
}

} // verus!
