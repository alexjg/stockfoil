//! Actual planning-to-emission consumers, with no caller validity premises.
use vstd::prelude::*;
use crate::{api_error::Error, change_plan::{self, Changes, Change, Batch}, commit_hash::CommitHash,
    history::History, view::{ViewPoint, MaterializationCursor}};

verus! {

pub(crate) fn arbitrary_batch(history: &History, changes: &mut Changes, out: &mut Vec<Change>, max: usize)
    ensures final(out)@.take(old(out)@.len() as int) == old(out)@,
        final(changes).selected().subset_of(old(changes).selected()),
        forall|i: int| old(out)@.len() <= i < final(out)@.len() ==>
            crate::events::after(final(out)@[i].visibility) == old(changes).after_members().contains(final(out)@[i].hash),
{
    let ghost initial = *changes;
    let ghost prefix = out@;
    let result = change_plan::emit_next_changes(history, changes, out, max);
    match result {
        Err(e) => { assert(e is WrongHistory && *changes == initial && out@ == prefix); },
        Ok(_) => {
            let ghost added = out@.skip(prefix.len() as int);
            assert forall|i: int| prefix.len() <= i < out@.len() implies
                crate::events::after(out@[i].visibility) == initial.after_members().contains(out@[i].hash) by {
                assert(out@[i] == added[i - prefix.len()]);
            }
        },
    }
}

/// A full budget drains precisely the planner's set and returns its captured cursor.
pub(crate) fn plan_and_drain(history: &History, cursor: &MaterializationCursor, target: &ViewPoint,
    delivery: &[CommitHash]) -> (result: Result<(Vec<Change>, MaterializationCursor), Error>)
    ensures match result {
        Err(e) => e is UnknownCommit || e is InvalidItem,
        Ok((records, completed)) => {
            let expected = change_plan::records(history.causal_from(delivery@), history.causal_from(cursor.delivery_view()),
                history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()),
                history.authorized_from(target.heads_view(), &target.permissions_view()));
            &&& change_plan::hashes(records@).to_set() == expected
            &&& change_plan::hashes(records@).no_duplicates()
            &&& completed.delivery_view() == delivery@ && completed.target_view().same(target)
            &&& forall|i: int| 0 <= i < records.len() ==> change_plan::mapped(history, records[i], history.causal_from(cursor.delivery_view()),
                history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()),
                history.authorized_from(target.heads_view(), &target.permissions_view()))
        },
    },
{
    let mut changes = match change_plan::plan_changes_at(history, cursor, target, delivery) { Ok(c) => c, Err(e) => return Err(e) };
    let ghost selected = changes.selected();
    let count = changes.remaining();
    let mut out = Vec::new();
    let batch = change_plan::emit_next_changes(history, &mut changes, &mut out, usize::MAX).unwrap();
    assert(changes.selected().is_empty());
    assert(out@.skip(0) =~= out@);
    proof { change_plan::schedule_facts(history, selected, change_plan::hashes(out@)); }
    assert(changes.selected() =~= Set::<CommitHash>::empty());
    assert(changes.selected() == selected.difference(change_plan::hashes(out@).to_set()));
    assert forall|h: CommitHash| selected.contains(h) implies change_plan::hashes(out@).to_set().contains(h) by {
        assert(!changes.selected().contains(h));
    }
    assert(change_plan::hashes(out@).to_set() =~= selected);
    let Batch::Complete(completed) = batch else { assert(false); return Err(Error::Stale); };
    assert(out.len() == count);
    assert(completed.delivery_view() == delivery@);
    assert(completed.target_view().same(target));
    let ghost first_output = out@;
    let ghost finished = *&changes;
    let again = change_plan::emit_next_changes(history, &mut changes, &mut out, 1).unwrap();
    assert(again is Complete && changes == finished && out.len() == count);
    assert(out@ =~= first_output);
    Ok((out, completed))
}

/// Failed inserts are allowed too; successful appends exercise the real mutation path.
pub(crate) fn grow_then_emit(history: &mut History, changes: &mut Changes, hash: CommitHash,
    author: crate::author::Author, parents: &[CommitHash], out: &mut Vec<Change>)
    ensures final(changes).selected().subset_of(old(changes).selected()),
        final(changes).before_members() == old(changes).before_members(), final(changes).after_members() == old(changes).after_members(),
{
    let _inserted = history.topology.insert(hash, author, parents, 42, 0);
    let _batch = change_plan::emit_next_changes(history, changes, out, 1);
}

pub(crate) fn zero_budget_preserves_state(history: &History, changes: &mut Changes, out: &mut Vec<Change>)
    ensures *final(changes) == *old(changes), final(out)@ == old(out)@,
{
    let _result = change_plan::emit_next_changes(history, changes, out, 0);
    assert(out@.take(out@.len() as int) =~= out@);
}

} // verus!
