//! Consumers of the actual public Materialization value and history-binding guard.
use vstd::prelude::*;
use crate::{api_error::Error, checkpoint::{Checkpoint, Follow, InProgress}, commit_hash::CommitHash,
    history::History, materialization_driver::{Materialization, binding_known, binding_references},
    write_permissions::WritePermissions, view::{MaterializationCursor, ViewPoint}, batch_protocol::Batch};

verus! {

pub(crate) fn arbitrary_binding(m: &mut Materialization, history: &History, permissions: &WritePermissions) {
    let ghost before = *m;
    match m.check_history(history, permissions) {
        Err(error) => {
            assert(*m == before);
            proof { if before.poisoned { assert(error is Poisoned); }
            else if before.owner is Some && before.owner.unwrap() != history.history_id() { assert(error is WrongHistory); }
            else {
                assert(error is UnknownCommit && !history.known(error->UnknownCommit_0));
                assert(binding_references(&before.checkpoint).contains(error->UnknownCommit_0));
            }
            }
        },
        Ok(()) => {
            assert(m.owner == Some(history.history_id()));
            assert(m.same_except_owner(&before));
            proof { if before.owner is None { assert(binding_known(history, &before.checkpoint)); } }
        },
    }
}

/// Successful initial binding plus repeated checks retain pending work and all published values.
pub(crate) fn valid_resume(history: &History, checkpoint: Checkpoint, permissions: &WritePermissions, supports: bool)
    requires binding_known(history, &checkpoint),
{
    let mut m = Materialization::resume(checkpoint, supports);
    proof { m.initial_facts(); m.representation(); }
    m.check_history(history, permissions).unwrap();
    let ghost bound = m;
    m.check_history(history, permissions).unwrap();
    assert(m == bound);
    proof { m.representation(); }
    let saved = m.checkpoint();
    assert(*saved == checkpoint);
    let dirty = m.is_dirty();
    assert(!dirty);
    let heads = m.heads();
    assert(heads@ == checkpoint.cursor_view().target_view().heads_view());
}

pub(crate) fn follow_preserves_pending(m: &mut Materialization, batch: &Batch, follow: Follow)
    requires old(m).protocol.accepts(batch),
{
    let ghost before = *m;
    m.set_follow(follow);
    proof { m.control_facts(&before); }
    assert(m.protocol.accepts(batch));
    assert(m.checkpoint_view().progress_view() == before.checkpoint_view().progress_view());
    assert(m.checkpoint_view().rejected_view() == before.checkpoint_view().rejected_view());
    let actual_follow = m.follow();
    assert(*actual_follow == follow);
    let heads = m.heads();
    assert(heads@ == before.checkpoint_view().cursor_view().target_view().heads_view());
}

pub(crate) fn fresh_resume_rejects_old_token(checkpoint: Checkpoint, old: &Batch) {
    let mut m = Materialization::resume(checkpoint, true);
    proof { m.initial_facts(); m.representation(); m.protocol.fresh_distinct(old); }
    let result = m.protocol.check(old);
    assert(result is Err && (result->Err_0) is StaleBatch);
}

/// Every binding role can fail independently, including a durable rejection absent from all heads.
pub(crate) fn missing_reference_roles() {
    let mut history = History::new();
    history.topology = crate::topology::Topology::new();
    let permissions = WritePermissions::new();
    let hash = CommitHash([9; 32]);
    proof { history.representation(); }
    assert(!history.known(hash));
    let mut i = 0usize;
    while i < 4
        invariant i <= 4, !history.known(hash),
        decreases 4 - i,
    {
        let mut checkpoint = Checkpoint::default();
        proof { checkpoint.representation(); }
        match i {
            0 => { checkpoint.cursor = MaterializationCursor::from_parts(vec![hash], ViewPoint::default()); },
            1 => { checkpoint.cursor = MaterializationCursor::from_parts(vec![], ViewPoint::from_parts(vec![hash], WritePermissions::new())); },
            2 => { checkpoint.follow = Follow::Pinned(vec![hash]); },
            _ => { checkpoint.reject(hash, true); },
        }
        proof { checkpoint.representation(); }
        assert(!binding_known(&history, &checkpoint));
        let mut m = Materialization::resume(checkpoint, true);
        proof { m.initial_facts(); m.representation(); }
        let ghost before = m;
        let result = m.check_history(&history, &permissions);
        assert(result is Err && (result->Err_0) is UnknownCommit);
        assert(m == before && m.owner is None);
        i += 1;
    }
}

/// Binding does not validate interrupted traversal metadata; Source construction owns that check.
pub(crate) fn partial_validation_is_deferred() {
    let mut history = History::new();
    history.topology = crate::topology::Topology::new();
    let permissions = WritePermissions::new();
    let hash = CommitHash([9; 32]);
    let mut checkpoint = Checkpoint::default();
        proof { checkpoint.representation(); }
    checkpoint.in_progress = Some(InProgress {
        target: ViewPoint::from_parts(vec![hash], WritePermissions::new()),
        delivery_heads: vec![hash], applied: u64::MAX,
    });
    proof { checkpoint.representation(); }
    assert(binding_known(&history, &checkpoint));
    let mut m = Materialization::resume(checkpoint, false);
    proof { m.initial_facts(); m.representation(); }
    m.check_history(&history, &permissions).unwrap();
    proof { m.representation(); }
    let saved = m.checkpoint();
    assert(*saved == checkpoint);
    let heads = m.heads();
    assert(heads.len() == 0);
    let partial = m.checkpoint().is_partial();
    assert(partial);
}

pub(crate) fn failed_binding_can_retry(m: &mut Materialization, first: &History, second: &History, permissions: &WritePermissions)
    requires old(m).owner is None, !old(m).poisoned, binding_known(second, &old(m).checkpoint),
{
    let ghost before = *m;
    match m.check_history(first, permissions) {
        Err(_) => {
            assert(*m == before);
            m.check_history(second, permissions).unwrap();
            assert(m.owner == Some(second.history_id()));
            assert(m.same_except_owner(&before));
        },
        Ok(()) => {},
    }
}

pub(crate) fn nonempty_binding() {
    let hash = CommitHash([7; 32]);
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, crate::author::Author([0; 32]), &vec![]);
    let mut history = History::new();
    history.topology = topology;
    proof { history.representation(); }
    assert(history.known(hash));
    let mut checkpoint = Checkpoint::new(Follow::Pinned(vec![hash, hash]));
    proof { checkpoint.representation(); }
    checkpoint.cursor = MaterializationCursor::from_parts(vec![hash], ViewPoint::from_parts(vec![hash], WritePermissions::new()));
    proof { checkpoint.representation(); }
    checkpoint.reject(hash, true);
    proof { checkpoint.representation(); }
    assert(binding_known(&history, &checkpoint));
    let permissions = WritePermissions::new();
    valid_resume(&history, checkpoint, &permissions, true);
}

pub(crate) fn poison_precedes_foreign_or_missing(m: &mut Materialization, history: &History, permissions: &WritePermissions)
    requires old(m).poisoned,
{
    let ghost before = *m;
    let result = m.check_history(history, permissions);
    assert(result is Err && (result->Err_0) is Poisoned && *m == before);
}

pub(crate) fn foreign_precedes_missing(m: &mut Materialization, history: &History, permissions: &WritePermissions)
    requires !old(m).poisoned, old(m).owner is Some, old(m).owner.unwrap() != history.history_id(),
{
    let ghost before = *m;
    let result = m.check_history(history, permissions);
    assert(result is Err && (result->Err_0) is WrongHistory && *m == before);
}

} // verus!
