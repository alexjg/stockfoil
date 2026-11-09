//! Real head updates and hash-resolved queries, including the duplicate-count trap.
use vstd::prelude::*;
use crate::{author::Author, commit_hash::CommitHash, topology::Topology, topology_examples::add,
    graph, storage_refinement::parent_set, clock::below};

verus! {

pub fn actual_head_queries() {
    let author = Author([1u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let h2 = CommitHash([2u8; 32]);
    let unknown = CommitHash([255u8; 32]);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && h2.0@[0] == 2 && unknown.0@[0] == 255);
    assert(h0 != h1 && h0 != h2 && h1 != h2 && unknown != h0 && unknown != h1 && unknown != h2);
    let mut t = Topology::new();
    let empty = t.history_clock(&vec![]).unwrap();
    assert(!below(empty.lookup(0), 0));
    let root = add(&mut t, h0, author, &vec![]);
    let unrelated = add(&mut t, h1, author, &vec![]);
    let ps = vec![h0, h0];
    let ghost resolved = ps@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(resolved =~= seq![root, root]);
    assert(parent_set(resolved) =~= ISet::empty().insert(0int));
    let child = add(&mut t, h2, author, &ps);
    assert(t.head_hashes() =~= Set::empty().insert(h1).insert(h2));
    let ghost g = t.graph();
    assert(g[0] =~= ISet::<int>::empty());
    assert(g[1] =~= ISet::<int>::empty());
    assert(g[2] =~= ISet::empty().insert(0int));

    let requested = vec![h2, h2];
    let ghost selected = requested@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(selected =~= seq![child, child]);
    assert(parent_set(selected) =~= ISet::empty().insert(2int));
    let subset = t.history_clock(&requested).unwrap();
    proof {
        graph::unfold_reaches(g, 1, 0);
        graph::unfold_reaches(g, 1, 2);
        crate::ancestry_refinement::singleton_history(g, 1, 2);
    }
    assert(!below(subset.lookup(t.nodes_view()[1].chain), 1));

    let requested = vec![h2, h1, h2];
    let ghost selected = requested@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(selected =~= seq![child, unrelated, child]);
    assert(parent_set(selected).contains(1int));
    let all_heads = t.history_clock(&requested).unwrap();
    proof {
        graph::unfold_reaches(g, 1, 1);
        assert(graph::history(g, parent_set(selected)).contains(1int));
    }
    assert(below(all_heads.lookup(t.nodes_view()[1].chain), 1));
    let ghost before = t.head_hashes();
    t.mark_admitted(root);
    t.mark_in_fragment(root);
    assert(t.head_hashes() == before);
    let bad = t.history_clock(&vec![h2, unknown]);
    assert(bad is Err);
}

} // verus!
