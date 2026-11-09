//! Callers of the actual edit phases, using checked native callback contracts.
use vstd::prelude::*;
use crate::{api_error::Error, author::Author, checkpoint::{Checkpoint,Follow},
    draft::Draft, drive_error::DriveError, history::History, ledger::Ledger,
    materialization_driver::Materialization, step_examples::Recording,
    view::{MaterializationCursor,ViewPoint}, write_permission::WritePermission,
    write_permissions::WritePermissions};

verus! {
fn none_edit(state:&mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures final(state).applied, out == Ok(None),
{
    state.applied = true;
    Ok(None)
}
fn bytes_edit(state:&mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures final(state).applied, out matches Ok(Some(bytes)) && bytes@ == seq![7u8,9u8],
{
    state.applied = true;
    Ok(Some(vec![7,9]))
}
fn failed_edit(state:&mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures final(state).applied, out == Err(17),
{
    state.applied = true;
    Err(17)
}

fn forbidden_edit(_: &mut Recording) -> Result<Option<Vec<u8>>,u8>
    requires false,
{ Err(99) }

pub(crate) fn rejected_before_edit<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,state:&mut Recording)
    requires !old(driver).edit_ready(ledger),
{
    let ghost before = *driver; let ghost initial = *state;
    let out = driver.begin_edit(ledger,state,forbidden_edit);
    assert(out matches Err(DriveError::Stockfoil(e)) && before.edit_preflight_error(ledger,e));
    assert(*state == initial && driver.same_except_owner(&before));
}

pub(crate) fn ready_none<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,state:&mut Recording)
    requires old(driver).edit_ready(ledger),
{
    let ghost before = *driver; let ghost initial = *state;
    let out = driver.begin_edit(ledger,state,none_edit);
    assert(out == Ok(None));
    assert(state.applied && driver.same_except_owner(&before));
    assert(!driver.poisoned && driver.dirty == before.dirty);
}

pub(crate) fn ready_bytes<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,state:&mut Recording)
    requires old(driver).edit_ready(ledger),
{
    let ghost before = *driver;
    let out = driver.begin_edit(ledger,state,bytes_edit);
    assert(out matches Ok(Some(bytes)) && bytes@ == seq![7u8,9u8]);
    assert(state.applied && driver.same_except_owner(&before));
}

pub(crate) fn ready_failure<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,state:&mut Recording)
    requires old(driver).edit_ready(ledger),
{
    let ghost before = *driver;
    let out = driver.begin_edit(ledger,state,failed_edit);
    assert(out == Err(DriveError::Edit(17)));
    assert(state.applied && driver.poisoned && driver.edit_frame(&before));
}

/// Construct a ready driver from actual empty public values; no edit-ready premise.
pub(crate) fn constructed_edit() {
    let history = History::new();
    proof { history.empty_facts(); history.representation(); history.head_set_facts(); }
    let delivery = history.captured_delivery_heads();
    assert(history.delivered_heads_set().is_empty());
    if delivery.len() > 0 { assert(delivery@.to_set().contains(delivery[0])); }
    assert(delivery@ =~= Seq::empty());
    let permissions = WritePermissions::new().with_unlisted(WritePermission::Unbounded);
    let mut cp = Checkpoint::new(Follow::Pinned(Vec::new()));
    proof { cp.representation(); }
    cp.cursor = MaterializationCursor::from_parts(delivery,ViewPoint::from_parts(Vec::new(),permissions.clone()));
    proof { cp.representation(); }
    let ledger = Ledger { history,author:Author([1;32]),permissions,options:(),queue:(),policy_epoch:0 };
    let mut driver = Materialization::resume(cp,true);
    proof {
        ledger.representation(); driver.representation(); driver.initial_facts(); driver.current_facts(&ledger);
        driver.checkpoint.representation();
        assert(driver.guard_valid(&ledger.history_view()));
        assert(driver.checkpoint.cursor_view().delivery_view().len() == 0);
        assert(ledger.history_view().delivered_heads_set().is_empty());
        assert(driver.checkpoint.cursor_view().delivery_view().to_set() =~= ledger.history_view().delivered_heads_set());
        assert(ledger.history_view().follow_target(&driver.checkpoint.follow_view(),&ledger.permissions_view(),&driver.checkpoint.cursor_view().target_view()));
        assert(driver.checkpoint.cursor_view().delivery_view().no_duplicates());
        assert(vstd::std_specs::btree::increasing_seq(driver.checkpoint.cursor_view().delivery_view()));
        assert(driver.checkpoint.progress_view() is None);
        assert(driver.protocol.pending_value() is None && !driver.poisoned && driver.owner is None);
        assert(driver.current(&ledger));
        assert(driver.edit_ready(&ledger));
    }
    let mut state = Recording { applied:false,saved:None,fail_apply:false,fail_checkpoint:false };
    let out = driver.begin_edit(&ledger,&mut state,bytes_edit);
    assert(out matches Ok(Some(bytes)) && bytes@ == seq![7u8,9u8]);
    assert(state.applied && !driver.poisoned);
}

pub(crate) fn failed_publication<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,error:Error) {
    let ghost before = *driver;
    let out:Result<Option<Draft>,DriveError<u8,u8>> = driver.finish_edit(ledger,Err(error));
    assert(out == Err(DriveError::Stockfoil(error)));
    assert(driver.poisoned_from(&before));
}

pub(crate) fn acknowledge_published<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,draft:Draft) {
    let ghost before = *driver;
    let out:Result<Option<Draft>,DriveError<u8,u8>> = driver.finish_edit(ledger,Ok(draft));
    match out {
        Ok(Some(returned)) => {
            assert(returned == draft);
            proof { driver.local_facts(&before,ledger,&draft,Ok(())); }
            assert(driver.current(ledger) && driver.dirty && !driver.poisoned);
            assert(Materialization::draft_matches(ledger,&draft) && before.local_participates(ledger,&draft));
        },
        Err(DriveError::Stockfoil(e)) => {
            let ghost ack = choose|ack:Materialization| ack.local_outcome(&before,ledger,&draft,Err(e)) && driver.poisoned_from(&ack);
            proof { ack.local_facts(&before,ledger,&draft,Err(e)); }
            assert(driver.poisoned && driver.edit_frame(&before));
            assert(before.local_error(ledger,&draft,e));
        },
        _ => { assert(false); },
    }
}

}
