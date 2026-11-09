//! Consumers of the actual planning entry point, without validity premises.
use vstd::prelude::*;
use crate::{api_error::Error, change_plan::{self, Changes}, commit_hash::CommitHash,
    history::History, view::{ViewPoint, MaterializationCursor}, write_permissions::WritePermissions,
    write_permission::WritePermission};

verus! {

pub(crate) fn arbitrary_plan(history: &History, cursor: &MaterializationCursor,
    target: &ViewPoint, delivery: &[CommitHash]) -> (out: Result<Changes, Error>)
    ensures match out {
        Ok(changes) => changes.delivery_view() == delivery@ && changes.target_view().same(target)
            && changes.selected() == change_plan::records(history.causal_from(delivery@), history.causal_from(cursor.delivery_view()),
                history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()),
                history.authorized_from(target.heads_view(), &target.permissions_view()))
            && changes.selected().subset_of(history.causal_from(delivery@)),
        Err(Error::UnknownCommit(h)) => !history.known(h),
        Err(e) => e is InvalidItem,
    },
{
    let out = change_plan::plan_changes_at(history, cursor, target, delivery);
    if let Ok(changes) = &out {
        let remaining = changes.remaining();
        assert(remaining == changes.selected().len());
        assert(changes.delivery_view() == delivery@ && changes.target_view().same(target));
    }
    out
}

pub fn public_view_and_cursor_copy(view: &ViewPoint, cursor: &MaterializationCursor) {
    let view_copy = view.clone();
    let cursor_copy = cursor.clone();
    let heads = view_copy.heads();
    let permissions = view_copy.write_permissions();
    assert(heads@ == view.heads_view() && permissions.same(&view.permissions_view()));
    let delivered = cursor_copy.delivery_heads();
    let saved = cursor_copy.view_point();
    assert(delivered@ == cursor.delivery_view() && saved.same(&cursor.target_view()));
}

pub fn nonempty_delivery_and_permission_retraction() {
    let hash = CommitHash([7; 32]);
    let mut history = History::new();
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, crate::author::Author([1; 32]), &vec![]);
    history.topology = topology;
    let heads = vec![hash];
    let permitted = ViewPoint::from_parts(heads.clone(), WritePermissions::new().with_unlisted(WritePermission::Unbounded));
    let denied = ViewPoint::from_parts(heads.clone(), WritePermissions::new());
    let empty = MaterializationCursor::new();
    proof {
        history.representation();
        history.topology.permission_facts();
        crate::graph::unfold_reaches(history.graph_view(), root@ as int, root@ as int);
        let resolved = heads@.map(|i: int, h: CommitHash| history.read_state().hashes[h]);
        assert(resolved[0] == root);
        assert(crate::storage_refinement::parent_set(resolved).contains(root@ as int));
        assert(crate::graph::history(history.graph_view(), crate::storage_refinement::parent_set(resolved)).contains(root@ as int));
        assert(history.causal_from(heads@) =~= Set::empty().insert(hash));
        assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(denied.heads_view(), &denied.permissions_view()) =~= Set::<CommitHash>::empty());
        assert(history.authorized_from(permitted.heads_view(), &permitted.permissions_view()) =~= Set::empty().insert(hash));
    }
    // A newly delivered record is reported even when permission denies participation.
    let mut cold = change_plan::plan_changes_at(&history, &empty, &denied, &heads).unwrap();
    let cold_count = cold.remaining();
    assert(cold.selected() =~= Set::empty().insert(hash));
    assert(cold_count == 1);
    let cursor = MaterializationCursor::from_parts(heads.clone(), permitted);
    let mut retraction = change_plan::plan_changes_at(&history, &cursor, &denied, &heads).unwrap();
    let count = retraction.remaining();
    assert(retraction.selected() =~= Set::empty().insert(hash));
    assert(count == 1);
    let denied_cursor = MaterializationCursor::from_parts(heads.clone(), denied.clone());
    proof { denied_cursor.target_view().same_authorization(&denied, &history); }
    let unchanged = change_plan::plan_changes_at(&history, &denied_cursor, &denied, &heads).unwrap();
    assert(denied_cursor.delivery_view() == heads@);
    assert(denied_cursor.target_view().heads_view() == heads@);
    assert(history.authorized_from(denied_cursor.target_view().heads_view(), &denied_cursor.target_view().permissions_view()) == Set::<CommitHash>::empty());
    assert(unchanged.selected() =~= Set::<CommitHash>::empty());
    let zero = unchanged.remaining();
    assert(zero == 0);
    // All heads are known, but the captured delivery frontier omits a required head.
    assert(history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
    assert(change_plan::known(&history.topology, denied_cursor.delivery_view()));
    assert(change_plan::known(&history.topology, denied_cursor.target_view().heads_view()));
    assert(change_plan::known(&history.topology, denied.heads_view()));
    assert(denied.heads_view().to_set().contains(hash));
    let no_delivery = Vec::<CommitHash>::new();
    assert(no_delivery@ == Seq::<CommitHash>::empty());
    let invalid = change_plan::plan_changes_at(&history, &denied_cursor, &denied, &no_delivery);
    assert(invalid matches Err(Error::InvalidItem(_)));
    // Resume both captures after a real same-chain append.
    let later_hash = CommitHash([8; 32]);
    assert(later_hash.0[0] != hash.0[0]);
    proof { history.topology.established(); }
    let later = crate::topology_examples::add(&mut history.topology, later_hash, crate::author::Author([1; 32]), &heads);
    proof { history.representation(); }
    let mut delivered = Vec::new();
    let batch = change_plan::emit_next_changes(&history, &mut cold, &mut delivered, 1).unwrap();
    assert(batch is Complete && delivered.len() == 1);
    assert(delivered[0].hash == hash);
    assert(delivered[0].payload == 42);
    assert(delivered[0].newly_delivered);
    assert(delivered[0].visibility is Neither);
    let mut revoked = Vec::new();
    let revoked_batch = change_plan::emit_next_changes(&history, &mut retraction, &mut revoked, usize::MAX).unwrap();
    assert(revoked_batch is Complete && revoked.len() == 1);
    assert(revoked[0].hash == hash && revoked[0].payload == 42 && !revoked[0].newly_delivered);
    assert(revoked[0].visibility is Before);

}

pub(crate) fn unknown_delivery_is_rejected(history: &History, unknown: CommitHash)
    requires !history.known(unknown),
{
    proof { history.representation(); }
    let cursor = MaterializationCursor::new();
    let target = ViewPoint::default();
    let result = change_plan::plan_changes_at(history, &cursor, &target, &vec![unknown]);
    assert(result matches Err(Error::UnknownCommit(h)) && h == unknown);
}

} // verus!
