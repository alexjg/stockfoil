//! Nonempty consumers of the actual head-resolution and permission compiler.
use vstd::prelude::*;
use crate::{author::Author, commit_hash::CommitHash, topology::Topology, topology_examples::add,
    write_permission::WritePermission, write_permissions::WritePermissions, permissions::capture_clocks, permission_refinement::resolved,
    clock::below, graph, storage_refinement::parent_set};

verus! {

pub fn actual_permission_capture() {
    let alice = Author([1u8; 32]);
    let bob = Author([2u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let unknown = CommitHash([255u8; 32]);
    assert(alice.0@[0] == 1 && bob.0@[0] == 2 && alice != bob);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && unknown.0@[0] == 255);
    assert(h0 != h1 && unknown != h0 && unknown != h1);
    let mut t = Topology::new();
    let root = add(&mut t, h0, alice, &vec![]);
    proof { t.permission_facts(); }
    let ghost root_chain = t.nodes_view()[0].chain;
    let ghost owners = t.owners_view();
    assert(root@ == 0);
    assert(t.labels().len() == 1);
    assert(t.labels()[0] == root_chain);
    assert(root_chain < owners.len());
    assert(owners[root_chain as int] == alice);
    let ps = vec![h0];
    let ghost parents = ps@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(parents =~= seq![root]);
    proof { crate::topology_examples::singleton_parent(root); }
    assert(parent_set(parents) =~= ISet::empty().insert(0int));
    let child = add(&mut t, h1, bob, &ps);
    proof { t.permission_facts(); }
    let ghost g = t.graph();
    assert(g[0] =~= ISet::<int>::empty());
    assert(g[1] =~= ISet::empty().insert(0int));
    assert(t.nodes_view()[0].chain == root_chain);
    assert(t.owners_view().subrange(0, owners.len() as int)[root_chain as int] == owners[root_chain as int]);
    assert(t.owners_view()[t.labels()[0] as int] == alice);
    assert(t.owners_view()[t.labels()[1] as int] == bob);
    proof {
        graph::unfold_reaches(g, 0, 0);
        graph::unfold_reaches(g, 0, 1);
        graph::unfold_reaches(g, 1, 0);
        graph::unfold_reaches(g, 1, 1);
    }

    let frontier = vec![h1, unknown, h1];
    assert(resolved(t.hashes(), frontier@) =~= ISet::empty().insert(1int));
    let mut permissions = WritePermissions::new();
    proof { crate::stdlib::author_key_model(); }
    permissions.insert(alice, WritePermission::Bounded(frontier));
    let first_heads = vec![h0];
    let ghost first = first_heads@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(first =~= seq![root]);
    assert(parent_set(first) =~= ISet::empty().insert(0int));
    proof {
        assert forall|n: int| graph::history(g, parent_set(first)).contains(n) == (n == 0) by {
            crate::ancestry_refinement::singleton_history(g, n, 0);
            graph::unfold_reaches(g, n, 0);
        }
        assert(graph::history(g, parent_set(first)) =~= ISet::empty().insert(0int));
    }
    let (selected, denied) = capture_clocks(&t, &first_heads, &permissions).unwrap();
    assert(below(selected.lookup(t.labels()[0]), 0));
    // A known but unavailable descendant must not authorize its selected ancestor.
    assert(!below(denied.lookup(t.labels()[0]), 0));
    assert(!below(denied.lookup(t.labels()[1]), 1));

    let next_heads = vec![h1, h1];
    let ghost next = next_heads@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(next =~= seq![child, child]);
    assert(parent_set(next) =~= ISet::empty().insert(1int));
    proof {
        crate::ancestry_refinement::singleton_history(g, 0, 1);
        crate::ancestry_refinement::singleton_history(g, 1, 1);
    }
    let (_, allowed) = capture_clocks(&t, &next_heads, &permissions).unwrap();
    // An available frontier can authorize another author's ancestor; missing Bob is denied.
    assert(below(allowed.lookup(t.labels()[0]), 0));
    assert(!below(allowed.lookup(t.labels()[1]), 1));
    permissions.insert(bob, WritePermission::Bounded(vec![h1]));
    proof { crate::permission_refinement::resolved_singleton(t.hashes(), h1); }
    assert(resolved(t.hashes(), seq![h1]) =~= ISet::empty().insert(1int));
    let (_, inclusive) = capture_clocks(&t, &next_heads, &permissions).unwrap();
    assert(below(inclusive.lookup(t.labels()[1]), 1));
    permissions.insert(alice, WritePermission::Denied);
    permissions.insert(bob, WritePermission::Unbounded);
    let (_, mixed) = capture_clocks(&t, &next_heads, &permissions).unwrap();
    assert(!below(mixed.lookup(t.labels()[0]), 0));
    assert(below(mixed.lookup(t.labels()[1]), 1));
    let bad = capture_clocks(&t, &vec![unknown], &permissions);
    assert(bad is Err);

    let mut fallback = WritePermissions::new().with_unlisted(WritePermission::Bounded(vec![h1, unknown, h1]));
    assert(resolved(t.hashes(), seq![h1, unknown, h1]) =~= ISet::empty().insert(1int));
    let (_, shared) = capture_clocks(&t, &next_heads, &fallback).unwrap();
    assert(below(shared.lookup(t.labels()[0]), 0));
    assert(below(shared.lookup(t.labels()[1]), 1));
    let (_, unavailable) = capture_clocks(&t, &first_heads, &fallback).unwrap();
    assert(!below(unavailable.lookup(t.labels()[0]), 0));
    fallback.insert(alice, WritePermission::Denied);
    let (_, overridden) = capture_clocks(&t, &next_heads, &fallback).unwrap();
    assert(!below(overridden.lookup(t.labels()[0]), 0));
    assert(below(overridden.lookup(t.labels()[1]), 1));
    fallback.remove(&alice);
    let (_, restored) = capture_clocks(&t, &next_heads, &fallback).unwrap();
    assert(below(restored.lookup(t.labels()[0]), 0));
    fallback.set_unlisted(WritePermission::Unbounded);
    fallback.insert(alice, WritePermission::Denied);
    let (_, unbounded) = capture_clocks(&t, &next_heads, &fallback).unwrap();
    assert(!below(unbounded.lookup(t.labels()[0]), 0));
    assert(below(unbounded.lookup(t.labels()[1]), 1));
}

} // verus!
