//! Consumers exercise the actual public difference and standard iterator methods.
use vstd::prelude::*;
use crate::{history::{History, Ancestry}, commits::Commits, commit_hash::CommitHash,
    api_error::Error, write_permissions::WritePermissions};
use vstd::std_specs::iter::IteratorSpec;

verus! {

pub fn collect_public_difference(after: &Ancestry, before: &Ancestry, history: &History) -> (out: Result<Vec<CommitHash>, Error>)
    ensures match out {
        Err(e) => e is WrongHistory && (after.history_id() != history.history_id() || before.history_id() != history.history_id()),
        Ok(hashes) => after.history_id() == history.history_id() && before.history_id() == history.history_id()
            && hashes@.to_set() == after.causal().difference(before.causal()) && hashes@.no_duplicates()
            && (forall|i: int, j: int| 0 <= i < j < hashes.len() ==> !history.strict_ancestor(hashes[j], hashes[i])),
    },
{
    let iter = match after.difference(before, history) { Ok(iter) => iter, Err(e) => return Err(e) };
    proof { iter.sequence_facts(); }
    let ghost expected = iter.future();
    let size = iter.len();
    let hint = iter.size_hint();
    assert(hint == (size, Some(size)));
    let out: Vec<CommitHash> = iter.collect();
    assert(out@ == expected && out.len() == size);
    Ok(out)
}

pub fn arbitrary_public_next(iter: &mut Commits<'_>)
    ensures final(iter).history_view() == old(iter).history_view(),
        final(iter).remaining_hashes().subset_of(old(iter).remaining_hashes()),
{
    let ghost initial = iter.remaining_hashes();
    let size = iter.len();
    let out = iter.next();
    let after_size = iter.len();
    match out {
        None => {
            assert(size == 0 && after_size == 0);
            let again = iter.next();
            assert(again is None);
        },
        Some(hash) => {
            assert(initial.contains(hash) && !iter.remaining_hashes().contains(hash));
            assert(after_size + 1 == size);
        },
    }
}

pub fn both_wrong_history_guards(history: &History, unrelated: &History)
    requires history.history_id() != unrelated.history_id(),
{
    let permissions = WritePermissions::new();
    let here = history.ancestry(&[], &permissions).unwrap();
    let there = unrelated.ancestry(&[], &permissions).unwrap();
    {
        let wrong_after = there.difference(&here, &history);
        let wrong_before = here.difference(&there, &history);
        assert(wrong_after matches Err(Error::WrongHistory));
        assert(wrong_before matches Err(Error::WrongHistory));
    }
    let mut empty = here.difference(&here, &history).unwrap();
    let len = empty.len();
    let hint = empty.size_hint();
    let first = empty.next();
    let again = empty.next();
    assert(len == 0 && hint == (0usize, Some(0usize)) && first is None && again is None);
}

pub fn nonempty_public_difference() {
    let hash = CommitHash([7; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, crate::author::Author([1; 32]), &vec![]);
    history.topology = topology;
    proof { history.representation(); assert(history.known(hash)); }
    let permissions = WritePermissions::new();
    let before = history.ancestry(&[], &permissions).unwrap();
    let heads = vec![hash];
    let after = history.ancestry(&heads, &permissions).unwrap();
    let contains = after.contains(&history, &hash).unwrap();
    let permitted = after.participates(&history, &hash).unwrap();
    proof {
        history.representation();
        crate::graph::unfold_reaches(history.graph_view(), root@ as int, root@ as int);
        let selected = heads@.map(|i: int, h: CommitHash| history.read_state().hashes[h]);
        assert(selected[0] == root);
        assert(crate::storage_refinement::parent_set(selected).contains(root@ as int));
        assert(crate::graph::history(history.graph_view(), crate::storage_refinement::parent_set(selected)).contains(root@ as int));
        assert(history.causal_from(heads@).contains(hash));
        assert(history.read_state().hashes.dom() =~= Set::empty().insert(hash));
        assert(after.causal().subset_of(history.read_state().hashes.dom()));
        assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(after.causal() =~= Set::empty().insert(hash));
    }
    assert(contains && !permitted);
    let mut iter = after.difference(&before, &history).unwrap();
    assert(before.causal() =~= Set::<CommitHash>::empty());
    assert(iter.remaining_hashes() =~= Set::empty().insert(hash));
    let size = iter.len();
    assert(size == 1);
    let emitted = iter.next();
    assert(emitted == Some(hash));
    assert(iter.remaining_hashes().is_empty());
    let exhausted = iter.next();
    assert(size == 1 && emitted == Some(hash) && exhausted is None);
}

}
