//! Checked callers of the actual public local-change API.
use vstd::prelude::*;
use crate::{api_error::Error, document::Document, drive_error::DriveError, materialization_driver::Materialization,
    materializer::Materializer, step_examples::Recording};
verus! {
fn no_commit(state:&mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures out == Ok(None), final(state).applied,
{ state.applied = true; Ok(None) }
fn edit_error(state:&mut Recording) -> (out:Result<Option<Vec<u8>>,u8>)
    ensures out == Err(23), final(state).applied,
{ state.applied = true; Err(23) }

pub(crate) fn denied_change<C,O,Q>(driver:&mut Materialization,document:&mut Document<C,O,Q>,state:&mut Recording)
    requires !old(driver).edit_ready(&old(document).ledger_view()),
{
    let ghost before = *driver; let ghost old_doc = *document; let ghost initial = *state;
    let out = driver.change(document,state,edit_error);
    proof {
        driver.change_facts(&before,document,&old_doc,initial,*state,edit_error,out);
        let (edited,result) = choose|edited:Materialization,result:Result<Option<Vec<u8>>,DriveError<u8,u8>>|
            edited.begin_edit_outcome(&before,&old_doc.ledger_view(),initial,*state,edit_error,result)
            && match result {
                Err(e) => *driver == edited && *document == old_doc && out == Err(e),
                Ok(None) => *driver == edited && *document == old_doc && out == Ok(None),
                Ok(Some(bytes)) => exists|shared:std::sync::Arc<[u8]>,parents:Vec<crate::commit_hash::CommitHash>,published:Result<crate::draft::Draft,Error>|
                    (*shared)@ == bytes@ && parents@ == edited.checkpoint_view().cursor_view().target_view().heads_view()
                    && document.byte_commit_outcome(&old_doc,shared,Some(parents),published)
                    && driver.finish_edit_outcome(&edited,&document.ledger_view(),published,out),
            };
        assert(out matches Err(DriveError::Stockfoil(e)) && before.edit_preflight_error(&old_doc.ledger_view(),e));
        assert(*document == old_doc && *state == initial && driver.same_except_owner(&before));
    }
}

pub(crate) fn no_commit_change<C,O,Q>(driver:&mut Materialization,document:&mut Document<C,O,Q>,state:&mut Recording)
    requires old(driver).edit_ready(&old(document).ledger_view()),
{
    let ghost before = *driver; let ghost old_doc = *document; let ghost initial = *state;
    let out = driver.change(document,state,no_commit);
    proof { driver.change_facts(&before,document,&old_doc,initial,*state,no_commit,out); }
    assert(out == Ok(None));
    assert(state.applied && *document == old_doc && driver.same_except_owner(&before));
}

pub(crate) fn failing_change<C,O,Q>(driver:&mut Materialization,document:&mut Document<C,O,Q>,state:&mut Recording)
    requires old(driver).edit_ready(&old(document).ledger_view()),
{
    let ghost before = *driver; let ghost old_doc = *document; let ghost initial = *state;
    let out = driver.change(document,state,edit_error);
    proof { driver.change_facts(&before,document,&old_doc,initial,*state,edit_error,out); }
    assert(out == Err(DriveError::Edit(23)));
    assert(state.applied && *document == old_doc && driver.poisoned && driver.edit_frame(&before));
}
}
