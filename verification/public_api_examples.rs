//! Consumers call the real exported types/methods, without validity premises.
use vstd::prelude::*;
use crate::{history::{History, Ancestry}, api_error::Error, commit_hash::CommitHash,
    author::Author, topology::Topology, topology_examples::add,
    write_permissions::WritePermissions, write_permission::WritePermission};

verus! {

pub fn arbitrary_public_membership(saved: &Ancestry, history: &History, hash: &CommitHash) {
    let causal = saved.contains(history, hash);
    let authorized = saved.participates(history, hash);
    assert(match causal {
        Ok(found) => saved.history_id() == history.history_id() && found == saved.causal().contains(*hash),
        Err(error) => error is WrongHistory && saved.history_id() != history.history_id(),
    });
    assert(match authorized {
        Ok(found) => saved.history_id() == history.history_id() && found == saved.authorized().contains(*hash),
        Err(error) => error is WrongHistory && saved.history_id() != history.history_id(),
    });
}

pub fn public_capture_contract(history: &History, requested: &[CommitHash],
    permissions: &WritePermissions, query: &CommitHash)
{
    match history.ancestry(requested, permissions) {
        Err(error) => {
            assert(match error {
                Error::UnknownCommit(hash) => requested@.contains(hash) && !history.known(hash),
                _ => false,
            });
        },
        Ok(saved) => {
            let cloned = saved.clone();
            let heads = cloned.heads();
            assert(heads@.to_set() == requested@.to_set());
            assert(heads@.no_duplicates() && Ancestry::normalized(heads@));
            let causal = cloned.contains(history, query);
            let authorized = cloned.participates(history, query);
            assert(causal == Ok::<bool, Error>(history.causal_from(requested@).contains(*query)));
            assert(authorized == Ok::<bool, Error>(history.authorized_from(requested@, permissions).contains(*query)));
        },
    }
}

/// Nonempty fixture uses the checked private insertion path; it does not claim
/// verification of public commit publication or admission/evidence bookkeeping.
pub fn nonempty_public_snapshot() {
    let alice = Author([1u8; 32]);
    let root_hash = CommitHash([0u8; 32]);
    let child_hash = CommitHash([1u8; 32]);
    let unknown = CommitHash([255u8; 32]);
    assert(root_hash.0@[0] == 0 && child_hash.0@[0] == 1 && unknown.0@[0] == 255);
    assert(root_hash != child_hash && root_hash != unknown && child_hash != unknown);
    let mut topology = Topology::new();
    let root = add(&mut topology, root_hash, alice, &vec![]);
    let child = add(&mut topology, child_hash, alice, &vec![root_hash]);
    let mut history = History::new();
    history.topology = topology;
    proof { history.representation(); }
    let requested = vec![child_hash, root_hash, child_hash];
    let mut permissions = WritePermissions::new();
    let captured = history.ancestry(&requested, &permissions).unwrap();
    let cloned = captured.clone();
    let heads = cloned.heads();
    assert(heads@.to_set() == requested@.to_set());
    assert(requested@[1] == root_hash && requested@[0] == child_hash);
    assert(requested@.to_set().contains(root_hash) && requested@.to_set().contains(child_hash));
    assert(heads@.contains(root_hash) && heads@.contains(child_hash));
    assert(heads@.no_duplicates());
    proof {
        let selected = requested@.map(|i: int, h: CommitHash| history.read_state().hashes[h]);
        assert(selected[1] == root);
        assert(crate::storage_refinement::parent_set(selected).contains(root@ as int));
        crate::graph::unfold_reaches(history.graph_view(), root@ as int, root@ as int);
        assert(history.causal_from(requested@).contains(root_hash));
    }
    let included = cloned.contains(&history, &root_hash).unwrap();
    assert(included);
    let forbidden = cloned.participates(&history, &root_hash).unwrap();
    assert(!forbidden);
    let absent = cloned.contains(&history, &unknown).unwrap();
    assert(!absent);
    let invalid = history.ancestry(&vec![unknown], &permissions);
    assert(invalid matches Err(Error::UnknownCommit(h)) && h == unknown);

    // Public capture remains frozen even when the caller later grants permissions.
    permissions.set_unlisted(WritePermission::Unbounded);
    let still_forbidden = cloned.participates(&history, &root_hash).unwrap();
    assert(!still_forbidden);
    let future = CommitHash([2u8; 32]);
    assert(future.0@[0] == 2);
    assert(future != root_hash && future != child_hash);
    assert(!cloned.causal().contains(future));
    proof { history.topology.established(); }
    let added = add(&mut history.topology, future, alice, &vec![child_hash]);
    proof { history.representation(); }
    let future_in_old_capture = cloned.contains(&history, &future).unwrap();
    assert(!future_in_old_capture);
}

} // verus!
