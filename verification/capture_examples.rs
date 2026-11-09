//! Capture and query the actual clocks across checked append and metadata paths.
use vstd::prelude::*;
use crate::{author::Author, commit_hash::CommitHash, topology::Topology, topology_examples::add,
    captured::{self, Captured}, write_permissions::WritePermissions, write_permission::WritePermission,
    graph, storage_refinement::parent_set, permission_refinement::resolved};

verus! {

/// Arbitrary borrowed production values: no caller validity or compatibility premise.
pub(crate) fn arbitrary_membership<P>(saved: &crate::captured::Captured,
    topology: &crate::topology::Topology<P>, hash: &crate::commit_hash::CommitHash)
{
    let contains = saved.contains(topology, hash);
    let participates = saved.participates(topology, hash);
    assert(match contains {
        Ok(found) => saved.history_id() == topology.history_id() && found == saved.causal().contains(*hash),
        Err(()) => saved.history_id() != topology.history_id(),
    });
    assert(match participates {
        Ok(found) => saved.history_id() == topology.history_id() && found == saved.authorized().contains(*hash),
        Err(()) => saved.history_id() != topology.history_id(),
    });
}


pub fn captured_answers_survive_real_appends() {
    let alice = Author([1u8; 32]);
    let bob = Author([2u8; 32]);
    let h0 = CommitHash([0u8; 32]);
    let h1 = CommitHash([1u8; 32]);
    let h2 = CommitHash([2u8; 32]);
    let h3 = CommitHash([3u8; 32]);
    let unknown = CommitHash([255u8; 32]);
    assert(h0.0@[0] == 0 && h1.0@[0] == 1 && h2.0@[0] == 2 && h3.0@[0] == 3 && unknown.0@[0] == 255);
    assert(h0 != h1 && h0 != h2 && h0 != h3 && h1 != h2 && h1 != h3 && h2 != h3);
    assert(unknown != h0 && unknown != h1 && unknown != h2 && unknown != h3);
    let mut t = Topology::new();
    let empty = Captured::new(&t, &vec![], &WritePermissions::new()).unwrap();
    let root = add(&mut t, h0, alice, &vec![]);
    proof { t.permission_facts(); }
    let absent = empty.contains(&t, &h0).unwrap();
    assert(!absent);
    assert(t.graph()[0] =~= ISet::<int>::empty());
    let heads = vec![h0];
    let ghost ids = heads@.map(|i: int, h: CommitHash| t.hashes()[h]);
    assert(ids =~= seq![root]);
    proof { crate::topology_examples::singleton_parent(root); graph::unfold_reaches(t.graph(), 0, 0); }
    assert(parent_set(ids) =~= ISet::empty().insert(0int));
    proof { crate::ancestry_refinement::singleton_history(t.graph(), 0, 0); }
    assert(graph::history(t.graph(), parent_set(ids)).contains(0int));
    let mut permissions = WritePermissions::new().with_unlisted(WritePermission::Unbounded);
    let saved = Captured::new(&t, &heads, &permissions).unwrap();
    assert(saved.causal().contains(h0) && saved.authorized().contains(h0));
    let cloned = saved.clone();
    let missing = cloned.participates(&t, &unknown).unwrap();
    assert(!missing);
    let frontier = WritePermissions::new().with_unlisted(WritePermission::Bounded(vec![h1]));
    assert(resolved(t.hashes(), seq![h1]) =~= ISet::<int>::empty());
    let withheld = Captured::new(&t, &heads, &frontier).unwrap();
    assert(!withheld.authorized().contains(h0));
    permissions.set_unlisted(WritePermission::Denied);

    let child = add(&mut t, h1, bob, &vec![h0]);
    let fork = add(&mut t, h2, alice, &vec![h0]);
    let join = add(&mut t, h3, alice, &vec![h1, h2]);
    t.mark_admitted(root);
    t.mark_in_fragment(child);
    let causal_root = cloned.contains(&t, &h0).unwrap();
    let authorized_root = cloned.participates(&t, &h0).unwrap();
    let later = cloned.contains(&t, &h3).unwrap();
    let later_authorized = cloned.participates(&t, &h2).unwrap();
    let still_withheld = withheld.participates(&t, &h0).unwrap();
    assert(causal_root && authorized_root && !later && !later_authorized && !still_withheld);
    let bad = Captured::new(&t, &vec![unknown], &permissions);
    assert(bad is Err);
}

} // verus!
