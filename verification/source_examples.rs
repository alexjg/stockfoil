//! Actual resume/advance/checkpoint consumers, not a model source.
use vstd::prelude::*;
use crate::{api_error::Error, change_plan, checkpoint::Checkpoint, commit_hash::CommitHash,
    history::History, materialization_source::{self, Source}, view::{MaterializationCursor, ViewPoint}};

verus! {

pub(crate) fn arbitrary_step(source: &mut Source, history: &History, published: &Checkpoint, budget: usize) {
    let ghost before = *source;
    match source.advance(history, budget) {
        Err(e) => { assert(*source == before && (e is Poisoned || e is WrongHistory)); },
        Ok(batch) => {
            assert(source.processed() == before.processed() + batch.records.len());
            assert(source.processed() + source.selected().len() == source.total());
            let next = source.checkpoint(published).unwrap();
            assert((next.progress_view() is None) == source.selected().is_empty());
        },
    }
}

/// Resuming the saved count selects exactly what remains after uninterrupted emission.
pub(crate) fn resume_matches_uninterrupted(history: &History, base: MaterializationCursor,
    target: ViewPoint, delivery: Vec<CommitHash>, budget: usize) -> (result: Result<(), Error>)
    ensures result is Ok || !materialization_source::valid_request(history, &base, &target, delivery@),
{
    let base_copy = base.clone();
    let target_copy = target.clone();
    let delivery_copy = delivery.clone();
    proof { materialization_source::same_request(history, &base_copy, &base, &target_copy, &target, delivery@); }
    let ghost initial = materialization_source::requested(history, &base, &target, delivery@);
    let mut source = match Source::new(history, base, target, delivery, 0, false) { Ok(s) => s, Err(e) => return Err(e) };
    let ghost empty = choose|p: Seq<CommitHash>| p.len() == 0 && change_plan::scheduled(history, initial, p) && source.selected() == initial.difference(p.to_set());
    assert(empty =~= Seq::<CommitHash>::empty());
    assert(source.selected() =~= initial);
    let batch = source.advance(history, budget).unwrap();
    let processed = batch.records.len() as u64;
    let resumed = Source::new(history, base_copy, target_copy, delivery_copy, processed, false).unwrap();
    let ghost prefix = choose|p: Seq<CommitHash>| p.len() == processed && change_plan::scheduled(history, initial, p) && resumed.selected() == initial.difference(p.to_set());
    proof { materialization_source::schedule_prefix(history, initial, prefix, change_plan::hashes(batch.records@), processed as nat); }
    assert(prefix.take(processed as int) =~= prefix);
    assert(change_plan::hashes(batch.records@).take(processed as int) =~= change_plan::hashes(batch.records@));
    assert(resumed.selected() == source.selected());
    assert(resumed.processed() == source.processed());
    Ok(())
}

pub(crate) fn growth_does_not_reset_progress(history: &mut History, source: &mut Source, hash: CommitHash,
    author: crate::author::Author, parents: &[CommitHash]) {
    let ghost before = *source;
    let _inserted = history.topology.insert(hash, author, parents, 42, 0);
    if let Ok(batch) = source.advance(history, 2) {
        assert(source.processed() == before.processed() + batch.records.len());
        assert(source.total() == before.total());
        assert(source.after_members() == before.after_members());
        assert(source.selected().subset_of(before.selected()));
    }
}

pub(crate) fn reset_uses_one_slot(source: &mut Source, history: &History) {
    let ghost before = *source;
    if let Ok(batch) = source.advance(history, 1) {
        if batch.reset { assert(batch.records.len() == 0 && source.processed() == before.processed()); }
        else { assert(batch.records.len() <= 1); }
        let again = source.advance(history, 1).unwrap();
        assert(!again.reset);
    }
}

pub(crate) fn empty_resume_rejects_impossible_count() {
    let history = History::new();
    let base = MaterializationCursor::new();
    let target = ViewPoint::default();
    proof {
        history.representation();
        assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(target.heads_view(), &target.permissions_view()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(base.target_view().heads_view(), &base.target_view().permissions_view()) =~= Set::<CommitHash>::empty());
        assert(materialization_source::requested(&history, &base, &target, Seq::empty()) =~= Set::<CommitHash>::empty());
    }
    let result = Source::new(&history, base, target, Vec::new(), 1, false);
    assert(result matches Err(Error::Malformed(_)));
}

pub(crate) fn nonempty_resume_stays_complete_after_growth() {
    let hash = CommitHash([7; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, crate::author::Author([1; 32]), &vec![]);
    history.topology = topology;
    let delivery = vec![hash];
    let base = MaterializationCursor::new();
    let target = ViewPoint::default();
    proof {
        history.representation(); history.topology.permission_facts();
        crate::graph::unfold_reaches(history.graph_view(), root@ as int, root@ as int);
        let resolved = delivery@.map(|i: int, h: CommitHash| history.read_state().hashes[h]);
        assert(resolved[0] == root);
        assert(crate::storage_refinement::parent_set(resolved).contains(root@ as int));
        assert(history.causal_from(delivery@) =~= Set::empty().insert(hash));
        assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(target.heads_view(), &target.permissions_view()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(base.target_view().heads_view(), &base.target_view().permissions_view()) =~= Set::<CommitHash>::empty());
        assert(materialization_source::requested(&history, &base, &target, delivery@) =~= Set::empty().insert(hash));
    }
    let mut source = Source::new(&history, base, target, delivery, 1, false).unwrap();
    assert(source.total() == 1 && source.processed() == 1);
    let ghost prefix = choose|p: Seq<CommitHash>| p.len() == 1 && change_plan::scheduled(&history, Set::empty().insert(hash), p)
        && source.selected() == Set::empty().insert(hash).difference(p.to_set());
    assert(prefix.take(0) =~= Seq::<CommitHash>::empty());
    assert(prefix[0] == hash);
    assert(prefix.to_set() =~= Set::empty().insert(hash));
    assert(source.selected() =~= Set::<CommitHash>::empty());
    let newer = CommitHash([8; 32]);
    assert(newer.0[0] != hash.0[0]);
    assert(newer != hash);
    let _new = crate::topology_examples::add(&mut history.topology, newer, crate::author::Author([2; 32]), &vec![]);
    proof { history.representation(); }
    let batch = source.advance(&history, 100).unwrap();
    assert(batch.records.len() == 0 && source.processed() == 1);
    let next = source.checkpoint(&Checkpoint::default()).unwrap();
    assert(next.progress_view() is None && next.cursor_view().delivery_view() == seq![hash]);
}

} // verus!
