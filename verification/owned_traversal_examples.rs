//! Owned consumers use the maintained invariant, including after real appends.
use vstd::prelude::*;
use crate::{api_error::Error, author::Author, clock::NodeIdx, commit_hash::CommitHash,
    history::History, owned_traversal::OwnedTraversal, tree::Clock, traversal_init as init};

verus! {

pub(crate) fn arbitrary_resume(cursor: &mut OwnedTraversal, history: &History)
    ensures final(cursor).remaining_hashes().subset_of(old(cursor).remaining_hashes()),
{
    let ghost captured = cursor.remaining_hashes();
    let count = cursor.remaining();
    let result = cursor.next(history);
    let remaining = cursor.remaining();
    match result {
        Err(e) => { assert(e is WrongHistory && remaining == count); },
        Ok(None) => {
            assert(count == 0 && remaining == 0);
            let exhausted = cursor.next(history);
            assert(exhausted matches Ok(None));
        },
        Ok(Some(n)) => {
            let hash = history.topology.nodes()[n.index()].hash;
            assert(captured.contains(hash) && !cursor.remaining_hashes().contains(hash));
            assert(remaining + 1 == count);
        },
    }
}

/// No validity, capacity, or clock premises: failed inserts and wrong-history
/// resumes are covered too. The successful branch uses the actual append body.
pub(crate) fn insert_then_resume(cursor: &mut OwnedTraversal, history: &mut History,
    hash: CommitHash, author: Author, parents: &[CommitHash])
    ensures final(cursor).remaining_hashes().subset_of(old(cursor).remaining_hashes()),
{
    let ghost captured = cursor.remaining_hashes();
    let _inserted = history.topology.insert(hash, author, parents, 42, 0);
    arbitrary_resume(cursor, history);
    assert(cursor.remaining_hashes().subset_of(captured));
}

pub fn captured_root_survives_multiple_appends() {
    let root_hash = CommitHash([7; 32]);
    let later_hash = CommitHash([8; 32]);
    let independent_hash = CommitHash([1; 32]);
    let merge_hash = CommitHash([0; 32]);
    assert(root_hash.0[0] != later_hash.0[0]);
    assert(root_hash.0[0] != independent_hash.0[0] && later_hash.0[0] != independent_hash.0[0]);
    assert(root_hash.0[0] != merge_hash.0[0] && later_hash.0[0] != merge_hash.0[0] && independent_hash.0[0] != merge_hash.0[0]);
    let alice = Author([1; 32]);
    let bob = Author([2; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, root_hash, alice, &vec![]);
    history.topology = topology;
    let chain = history.topology.nodes()[root.index()].chain;
    let mut after = Clock::default();
    after.include(chain, root);
    let empty = Clock::default();
    let differences = vec![(&after, &empty)];
    proof {
        history.representation(); history.topology.established(); history.topology.traversal_facts(); history.topology.traversal_node(root);
        assert(init::inputs(history.topology.tables_view(), differences@));
        let p = choose|p: int| 0 <= p < history.topology.tables_view()[chain as int].len()
            && history.topology.tables_view()[chain as int][p] == root;
        assert(init::delta(history.topology.tables_view(), &after, &empty, chain, p));
        assert(init::union(history.topology.tables_view(), differences@, chain, p));
        assert(init::requested(&history.topology, differences@, root));
    }
    let mut cursor = OwnedTraversal::new(&history, &differences);
    assert(cursor.remaining_hashes() =~= Set::empty().insert(root_hash));
    let captured_count = cursor.remaining();
    assert(captured_count == 1);
    let later = crate::topology_examples::add(&mut history.topology, later_hash, alice, &vec![root_hash]);
    let independent = crate::topology_examples::add(&mut history.topology, independent_hash, bob, &vec![]);
    let merge = crate::topology_examples::add(&mut history.topology, merge_hash, alice, &vec![later_hash, independent_hash]);
    proof { history.representation(); }
    let emitted = cursor.next(&history).unwrap().unwrap();
    proof {
        history.topology.traversal_facts();
        assert(history.topology.nodes_view()[emitted@ as int].hash == root_hash);
        NodeIdx::same_index(emitted, root);
    }
    assert(emitted == root);
    let remaining = cursor.remaining();
    let exhausted = cursor.next(&history).unwrap();
    assert(remaining == 0 && exhausted is None);
    assert(emitted != later && emitted != independent && emitted != merge);
}

pub(crate) fn wrong_history_preserves_cursor(cursor: &mut OwnedTraversal, history: &History)
    requires old(cursor).history_id() != history.history_id(),
    ensures *final(cursor) == *old(cursor),
{
    let result = cursor.next(history);
    assert(result matches Err(Error::WrongHistory));
}

} // verus!
