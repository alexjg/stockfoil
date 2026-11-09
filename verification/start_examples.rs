//! Real clock comparison, input selection, source installation and batch composition.
use vstd::prelude::*;
use crate::{api_error::Error, checkpoint::{Checkpoint, Follow, InProgress}, commit_hash::CommitHash,
    history::History, materialization_driver::Materialization, materialization_start::{self as start, Inputs},
    materialization_source::{self as source, Source}, view::{MaterializationCursor, ViewPoint},
    write_permissions::WritePermissions, write_permission::WritePermission, tree::Clock, clock::NodeIdx};

verus! {

pub(crate) fn exact_visibility(history: &History, cursor: &MaterializationCursor, target: &ViewPoint) {
    let result = start::visibility_changed(history, cursor, target);
    assert(match result {
        Ok(changed) => changed == start::visibility_differs(history, cursor, target),
        Err(error) => error is UnknownCommit && !history.known(error->UnknownCommit_0),
    });
}

pub(crate) fn partial_inputs_ignore_current_policy(history: &History, checkpoint: &Checkpoint,
    permissions: &WritePermissions, replacement: Follow, changed_permissions: &WritePermissions)
    requires checkpoint.progress_view() is Some,
{
    let first = checkpoint.source_inputs(history, permissions, false).unwrap();
    let mut later = checkpoint.clone();
    proof { later.representation(); }
    later.follow = replacement;
    proof { later.representation(); }
    let second = later.source_inputs(history, changed_permissions, true).unwrap();
    assert(!first.reset && !second.reset && first.processed == second.processed);
    assert(first.delivery@ == second.delivery@);
    assert(first.target.same(&second.target));
    assert(first.base.same(&second.base));
}

pub(crate) fn partial_start_resumes(history: &History, checkpoint: Checkpoint, permissions: &WritePermissions, supports: bool)
    requires checkpoint.progress_view() is Some,
        source::valid_request(history, &checkpoint.cursor_view(), &checkpoint.progress_view().unwrap().target,
            checkpoint.progress_view().unwrap().delivery_heads@),
        checkpoint.progress_view().unwrap().applied <= source::requested(history, &checkpoint.cursor_view(),
            &checkpoint.progress_view().unwrap().target, checkpoint.progress_view().unwrap().delivery_heads@).len(),
{
    let mut driver = Materialization::resume(checkpoint, supports);
    proof { driver.representation(); }
    let result = driver.start_history(history, permissions);
    proof {
        if let Err(error) = result {
            let input = choose|input: Inputs| input.matches(history, &checkpoint, permissions, supports) && input.construction_error(history, error);
            source::same_request(history, &input.base, &checkpoint.cursor_view(), &input.target,
                &checkpoint.progress_view().unwrap().target, input.delivery@);
            assert(false);
        }
    }
    assert(result is Ok);
    assert(driver.checkpoint == checkpoint);
    let ghost input = choose|input: Inputs| input.matches(history, &checkpoint, permissions, supports) && input.started(history, &driver.source.unwrap());
    assert(driver.source.unwrap().ready());
    assert(driver.source.unwrap().processed() == checkpoint.progress_view().unwrap().applied);
    assert(!driver.source.unwrap().reset_pending());
    assert(driver.source.unwrap().delivery() == checkpoint.progress_view().unwrap().delivery_heads@);
    assert(driver.source.unwrap().target().same(&checkpoint.progress_view().unwrap().target));
}

/// The production start transition feeds the actual source/translation/token helpers.
pub(crate) fn start_to_batch(history: &History, checkpoint: Checkpoint, permissions: &WritePermissions, supports: bool, budget: usize)
    -> Result<(), Error>
{
    let mut driver = Materialization::resume(checkpoint, supports);
    proof { driver.initial_facts(); driver.representation(); }
    driver.start_history(history, permissions)?;
    let ghost input = choose|input: Inputs| input.matches(history, &checkpoint, permissions, supports) && input.started(history, &driver.source.unwrap());
    let source = driver.source.as_mut().unwrap();
    let ghost authorized = source.after_members();
    let emitted = source.advance(history, budget).unwrap();
    let (events, summary) = crate::materialization_events::translate(history, &driver.checkpoint, &emitted.records, emitted.reset);
    proof { crate::materialization_events::translation_authorized(history, &driver.checkpoint, emitted.records@, authorized); }
    let next = source.checkpoint(&driver.checkpoint).unwrap();
    let pending = crate::materialization_feedback::Pending::new(next, summary);
    let batch = driver.protocol.install(pending, events).unwrap();
    assert(driver.protocol.accepts(&batch));
    assert(driver.checkpoint == checkpoint);
    assert forall|i: int| #![trigger batch.events_view()[i]] 0 <= i < batch.events_view().len()
        && crate::materialization_events::activation(batch.events_view()[i]) is Some implies
        authorized.contains(crate::materialization_events::activation(batch.events_view()[i]).unwrap()) by {
        let offset = if emitted.reset { 1int } else { 0int };
        if i >= offset {
            assert(batch.events_view()[i] == crate::materialization_events::translated(history, &checkpoint, emitted.records@)[i - offset]);
        }
    }
    Ok(())
}

pub(crate) fn clock_shapes_and_values() {
    let mut a = Clock::default();
    a.include(0, NodeIdx::new(0)); a.include(1, NodeIdx::new(1));
    let mut b = Clock::default();
    b.include(1, NodeIdx::new(1)); b.include(0, NodeIdx::new(0));
    proof { NodeIdx::same_index(a.contents()[0], b.contents()[0]); NodeIdx::same_index(a.contents()[1], b.contents()[1]); }
    assert(a.contents() =~= b.contents());
    let equal = a.same_map(&b);
    assert(equal);
    b.include(0, NodeIdx::new(2));
    let different = a.same_map(&b);
    assert(!different);
}

pub(crate) fn nonempty_reset_and_new_delivery() {
    let hash = CommitHash([7; 32]);
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, crate::author::Author([1; 32]), &vec![]);
    let mut history = History::new(); history.topology = topology;
    let heads = vec![hash];
    let permitted = ViewPoint::from_parts(heads.clone(), WritePermissions::new().with_unlisted(WritePermission::Unbounded));
    let denied = ViewPoint::from_parts(heads.clone(), WritePermissions::new());
    let empty = MaterializationCursor::new();
    proof {
        history.representation(); history.topology.permission_facts();
        crate::graph::unfold_reaches(history.graph_view(), root@ as int, root@ as int);
        let resolved = heads@.map(|i: int, h: CommitHash| history.read_state().hashes[h]);
        assert(resolved[0] == root);
        assert(crate::storage_refinement::parent_set(resolved).contains(root@ as int));
        assert(crate::graph::history(history.graph_view(), crate::storage_refinement::parent_set(resolved)).contains(root@ as int));
        assert(history.causal_from(heads@) =~= Set::empty().insert(hash));
        assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(denied.heads_view(), &denied.permissions_view()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(permitted.heads_view(), &permitted.permissions_view()) =~= Set::empty().insert(hash));
        assert(history.authorized_from(empty.target_view().heads_view(), &empty.target_view().permissions_view()) =~= Set::<CommitHash>::empty());
    }
    assert(empty.delivery_view() =~= Seq::<CommitHash>::empty());
    assert(history.authorized_from(permitted.heads_view(), &permitted.permissions_view()).intersect(history.causal_from(empty.delivery_view())) =~= Set::<CommitHash>::empty());
    assert(!start::visibility_differs(&history, &empty, &permitted));
    let cold = start::visibility_changed(&history, &empty, &permitted).unwrap();
    assert(!cold); // New deliveries alone do not require reset.
    let cursor = MaterializationCursor::from_parts(heads.clone(), permitted);
    let revoked = start::visibility_changed(&history, &cursor, &denied).unwrap();
    assert(revoked);
    let mut checkpoint = Checkpoint::new(Follow::Pinned(heads.clone()));
    proof { checkpoint.representation(); }
    checkpoint.cursor = cursor;
    proof { checkpoint.representation(); }
    let policy = WritePermissions::new();
    let reset = checkpoint.source_inputs(&history, &policy, false).unwrap();
    proof { reset.target.same_authorization(&denied, &history); }
    assert(reset.reset && reset.processed == 0 && start::empty_base(&reset.base));
    let incremental = checkpoint.source_inputs(&history, &policy, true).unwrap();
    assert(!incremental.reset && incremental.base.same(&checkpoint.cursor_view()));
    let partial = checkpoint.prepare_next(&checkpoint.cursor, &denied, &heads, 0, &None);
    proof {
        source::same_request(&history, &partial.cursor_view(), &checkpoint.cursor_view(),
            &partial.progress_view().unwrap().target, &denied, heads@);
    }
    assert(source::valid_request(&history, &partial.cursor_view(), &partial.progress_view().unwrap().target, heads@));
    partial_start_resumes(&history, partial, &policy, false);
}

pub(crate) fn invalid_partial_count_preserves_existing_source(driver: &mut Materialization, history: &History, permissions: &WritePermissions)
    requires old(driver).checkpoint.progress_view() is Some,
        source::valid_request(history, &old(driver).checkpoint.cursor_view(), &old(driver).checkpoint.progress_view().unwrap().target,
            old(driver).checkpoint.progress_view().unwrap().delivery_heads@),
        old(driver).checkpoint.progress_view().unwrap().applied > source::requested(history, &old(driver).checkpoint.cursor_view(),
            &old(driver).checkpoint.progress_view().unwrap().target, old(driver).checkpoint.progress_view().unwrap().delivery_heads@).len(),
{
    let ghost before = *driver;
    let ghost checkpoint = driver.checkpoint;
    let result = driver.start_history(history, permissions);
    proof {
        if result is Ok {
            let input = choose|input: Inputs| input.matches(history, &checkpoint, permissions, before.supports_visibility_changes)
                && input.started(history, &driver.source.unwrap());
            source::same_request(history, &input.base, &checkpoint.cursor_view(), &input.target,
                &checkpoint.progress_view().unwrap().target, input.delivery@);
            assert(false);
        }
        let input = choose|input: Inputs| input.matches(history, &checkpoint, permissions, before.supports_visibility_changes)
            && input.construction_error(history, result->Err_0);
        source::same_request(history, &input.base, &checkpoint.cursor_view(), &input.target,
            &checkpoint.progress_view().unwrap().target, input.delivery@);
    }
    assert(result is Err && result->Err_0 is Malformed);
    assert(*driver == before);
}

pub(crate) fn unknown_visibility_head(history: &History, unknown: CommitHash)
    requires !history.known(unknown),
{
    let base = MaterializationCursor::from_parts(vec![unknown], ViewPoint::default());
    let target = ViewPoint::default();
    proof { history.representation(); }
    assert(base.delivery_view() =~= seq![unknown]);
    assert(base.target_view().heads_view() =~= Seq::<CommitHash>::empty());
    assert(target.heads_view() =~= Seq::<CommitHash>::empty());
    let result = start::visibility_changed(history, &base, &target);
    assert(result is Err && result->Err_0 is UnknownCommit && result->Err_0->UnknownCommit_0 == unknown);
}

} // verus!
