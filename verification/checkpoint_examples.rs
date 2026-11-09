//! Actual checkpoint transitions composed with planning, emission and batch tokens.
use vstd::prelude::*;
use crate::{api_error::Error, batch_protocol::{Event, Protocol}, checkpoint::{Checkpoint, Follow},
    change_plan::{self, Batch as ChangeBatch}, commit_hash::CommitHash, history::History,
    history_data::PayloadId, view::{MaterializationCursor, ViewPoint}};

verus! {

pub fn public_observation(checkpoint: &Checkpoint) {
    let copy = checkpoint.clone();
    let heads = copy.heads();
    let follow = copy.follow();
    let partial = copy.is_partial();
    assert(copy.same(checkpoint));
    assert(heads@ == checkpoint.cursor_view().target_view().heads_view());
    assert(follow.same(&checkpoint.follow_view()));
    assert(partial == (checkpoint.progress_view() is Some));
}

/// No validity premise: the actual planner validates arbitrary persisted metadata.
pub(crate) fn plan_emit_stage(history: &History, checkpoint: &Checkpoint, target: &ViewPoint,
    delivery: &[CommitHash], budget: usize) -> (result: Result<Checkpoint, Error>)
    ensures match result {
        Err(e) => e is UnknownCommit || e is InvalidItem,
        Ok(next) => next.follow_view().same(&checkpoint.follow_view()) && next.rejected_view() == checkpoint.rejected_view()
            && match next.progress_view() {
                None => next.cursor_view().delivery_view() == delivery@ && next.cursor_view().target_view().same(target),
                Some(p) => next.cursor_view().same(&checkpoint.cursor_view()) && p.target.same(target)
                    && p.delivery_heads@ == delivery@ && p.applied <= budget,
            },
    },
{
    proof { checkpoint.representation(); }
    let mut changes = match change_plan::plan_changes_at(history, &checkpoint.cursor, target, delivery) {
        Ok(c) => c, Err(e) => return Err(e),
    };
    let count = changes.remaining();
    let mut records = Vec::new();
    let status = change_plan::emit_next_changes(history, &mut changes, &mut records, budget).unwrap();
    let done = match status { ChangeBatch::More => None, ChangeBatch::Complete(cursor) => Some(cursor) };
    assert(records.len() <= budget && records.len() <= count);
    let processed = records.len() as u64;
    assert(processed == records.len());
    let next = checkpoint.prepare_next(&checkpoint.cursor, target, delivery, processed, &done);
    let remaining = changes.remaining();
    assert((next.progress_view() is None) == (remaining == 0));
    Ok(next)
}

/// The handshake retains the exact prepared value until its matching acknowledgment.
/// This is a composition witness, not the outer Materialization::complete body.
pub(crate) fn acknowledgment_preserves_candidate(published: &mut Checkpoint, target: &ViewPoint,
    delivery: &[CommitHash], processed: u64, events: Vec<Event<PayloadId>>, latest_follow: Follow)
    ensures final(published).follow_view().same(&latest_follow),
        final(published).rejected_view() == old(published).rejected_view(),
        final(published).cursor_view().same(&old(published).cursor_view()),
        final(published).progress_view() is Some && final(published).progress_view().unwrap().target.same(target)
            && final(published).progress_view().unwrap().delivery_heads@ == delivery@
            && final(published).progress_view().unwrap().applied == processed,
{
    proof { published.representation(); }
    let ghost original = *published;
    let next = published.prepare_next(&published.cursor, target, delivery, processed, &None);
    let ghost candidate = next;
    let mut protocol = Protocol::new();
    let batch = protocol.install(next, events).unwrap();
    assert(*published == original);
    let clone = batch.clone();
    let waiting = protocol.check(&clone).unwrap();
    assert(*waiting == candidate);
    published.follow = latest_follow;
    proof { published.representation(); }
    let mut next = protocol.take(&clone).unwrap();
    proof { next.representation(); }
    next.follow = published.follow.clone();
    proof { next.representation(); }
    *published = next;
    let stale = protocol.check(&batch);
    assert(stale is Err);
}

pub fn rejection_and_restart(checkpoint: &mut Checkpoint, hash: CommitHash, target: &ViewPoint, delivery: &[CommitHash])
    ensures final(checkpoint).rejected_view() == old(checkpoint).rejected_view().insert(hash, true),
        final(checkpoint).follow_view() == old(checkpoint).follow_view(),
        final(checkpoint).cursor_view().delivery_view().len() == 0,
        final(checkpoint).cursor_view().target_view().heads_view().len() == 0,
        final(checkpoint).progress_view() is Some && final(checkpoint).progress_view().unwrap().applied == 0
            && final(checkpoint).progress_view().unwrap().delivery_heads@ == delivery@
            && final(checkpoint).progress_view().unwrap().target.same(target),
{
    let _first = checkpoint.reject(hash, true);
    let weakened = checkpoint.reject(hash, false);
    assert(!weakened && checkpoint.rejected_view()[hash]);
    checkpoint.restart(target, delivery);
}

pub fn nonempty_partial_and_completed_heads() {
    let base_hash = CommitHash([3; 32]);
    let target_hash = CommitHash([7; 32]);
    let base_view = ViewPoint::from_parts(vec![base_hash], crate::write_permissions::WritePermissions::new());
    let base = MaterializationCursor::from_parts(vec![base_hash], base_view);
    let target = ViewPoint::from_parts(vec![target_hash], crate::write_permissions::WritePermissions::new());
    let delivery = vec![target_hash, base_hash];
    let mut checkpoint = Checkpoint::new(Follow::Signed);
    let fresh = checkpoint.reject(base_hash, true);
    assert(fresh);
    let partial = checkpoint.prepare_next(&base, &target, &delivery, 1, &None);
    let partial_heads = partial.heads();
    let is_partial = partial.is_partial();
    assert(is_partial && partial_heads@ == seq![base_hash]);
    assert(partial.progress_view().unwrap().applied == 1);
    let completed = MaterializationCursor::from_parts(delivery.clone(), target.clone());
    let next = partial.prepare_next(&base, &target, &delivery, 2, &Some(completed));
    let heads = next.heads();
    let is_partial = next.is_partial();
    assert(!is_partial && heads@ == seq![target_hash]);
    assert(next.rejected_view()[base_hash] && next.follow_view() is Signed);
    let mut restarted = next.clone();
    restarted.restart(&target, &delivery);
    let heads = restarted.heads();
    assert(heads.len() == 0 && restarted.rejected_view()[base_hash]);
}

} // verus!
