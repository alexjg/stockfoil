//! Actual public local-change orchestration through checked conversion, hashing and publication.
use super::{
    document::Document, draft::Draft, drive_error::DriveError,
    materialization_driver::Materialization, materializer::Materializer,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use {
    super::{api_error::Error, commit_hash::CommitHash},
    std::sync::Arc,
    vstd::prelude::*,
};
verus! {
impl Materialization {
    pub closed spec fn edit_admissible<O,Q>(&self,ledger:&super::ledger::Ledger<O,Q>) -> bool { self.edit_ready(ledger) }
    #[verifier::prophetic]
    pub(crate) open spec fn change_trace<C,M:Materializer,E,O,Q,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
        &self,before:&Self,document:&Document<C,O,Q>,old_document:&Document<C,O,Q>,initial:M,state:M,edit:F,
        out:Result<Option<Draft>,DriveError<M::Error,E>>) -> bool {
        exists|edited:Self,result:Result<Option<Vec<u8>>,DriveError<M::Error,E>>|
            edited.begin_edit_outcome(before,&old_document.ledger_view(),initial,state,edit,result)
            && match result {
                Err(e) => *self == edited && *document == *old_document && out == Err(e),
                Ok(None) => *self == edited && *document == *old_document && out == Ok(None),
                Ok(Some(bytes)) => exists|shared:Arc<[u8]>,parents:Vec<CommitHash>,published:Result<Draft,Error>|
                    (*shared)@ == bytes@ && parents@ == edited.checkpoint_view().cursor_view().target_view().heads_view()
                    && document.byte_commit_outcome(old_document,shared,Some(parents),published)
                    && self.finish_edit_outcome(&edited,&document.ledger_view(),published,out),
            }
    }
    #[verifier::prophetic]
    pub closed spec fn change_outcome<C,M:Materializer,E,O,Q,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
        &self,before:&Self,document:&Document<C,O,Q>,old_document:&Document<C,O,Q>,initial:M,state:M,edit:F,
        out:Result<Option<Draft>,DriveError<M::Error,E>>) -> bool {
        self.change_trace(before,document,old_document,initial,state,edit,out)
    }
    pub(crate) proof fn change_facts<C,M:Materializer,E,O,Q,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
        &self,before:&Self,document:&Document<C,O,Q>,old_document:&Document<C,O,Q>,initial:M,state:M,edit:F,
        out:Result<Option<Draft>,DriveError<M::Error,E>>)
        requires self.change_outcome(before,document,old_document,initial,state,edit,out),
        ensures self.change_trace(before,document,old_document,initial,state,edit,out),
    {}
    /// Edit locally without echoing the commit to this state. Preflight errors
    /// do not call the edit; edit and later errors may retain changes and poison
    /// the driver. A successful draft leaves the checkpoint current and dirty.
    pub fn change<C,M:Materializer,E>(&mut self,document:&mut Document<C,impl Sized,impl Sized>,state:&mut M,
        edit:impl FnOnce(&mut M)->Result<Option<Vec<u8>>,E>) -> (out:Result<Option<Draft>,DriveError<M::Error,E>>)
        requires old(self).edit_admissible(&old(document).ledger_view()) ==>
            forall|r:&mut M| *r == *old(state) ==> call_requires(edit,(r,)),
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).change_outcome(old(self),final(document),old(document),*old(state),*final(state),edit,out),
            (out matches Ok(Some(_))) ==> final(self).current(&final(document).ledger_view()) && final(self).dirty_view(),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self; let ghost old_document = *document; let ghost initial = *state;
        let result = self.begin_edit(document.ledger(),state,edit);
        let ghost edited = *self;
        let bytes = match result {
            Err(e) => { assert(self.change_trace(&before,document,&old_document,initial,*state,edit,Err(e))); return Err(e); },
            Ok(None) => { assert(self.change_trace(&before,document,&old_document,initial,*state,edit,Ok(None))); return Ok(None); },
            Ok(Some(bytes)) => bytes,
        };
        let shared = super::stdlib::share_bytes(bytes);
        let parents = super::materialization_update::copy_heads(self.heads());
        let published = document.commit(shared,Some(parents));
        proof { document.committed_bytes(&old_document,shared,Some(parents),published); }
        let out = self.finish_edit(document.ledger(),published);
        proof {
            if out is Ok && out->Ok_0 is Some {
                let draft = out->Ok_0->Some_0;
                self.local_facts(&edited,&document.ledger_view(),&draft,Ok(()));
                self.representation();
            }
        }
        assert(document.byte_commit_outcome(&old_document,shared,Some(parents),published));
        assert(self.change_trace(&before,document,&old_document,initial,*state,edit,out));
        out
    }
}
} // verus!
