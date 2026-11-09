//! Actual local-edit preflight, callback invocation and post-publication acknowledgement.
use super::{
    api_error::Error, checkpoint::Follow, draft::Draft, drive_error::DriveError, ledger::Ledger,
    materialization_driver::Materialization, materializer::Materializer,
    write_permission::WritePermission,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
#[verifier::prophetic]
#[verifier::deprecated_postcondition_mut_ref_style(false)]
pub(crate) open spec fn edit_effect<M,E,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
    edit:F,before:M,after:M,out:Result<Option<Vec<u8>>,E>) -> bool {
    exists|r:&mut M| *r == before && *final(r) == after && call_ensures(edit,(r,),out)
}

impl Materialization {
    pub(crate) open spec fn edit_ready<O,Q>(&self,ledger:&Ledger<O,Q>) -> bool {
        self.guard_valid(&ledger.history_view()) && self.current(ledger)
            && !(self.checkpoint.follow_view() is Signed)
            && ledger.permissions_view().rule(ledger.author_view()) is Unbounded
    }
    pub(crate) open spec fn edit_preflight_error<O,Q>(&self,ledger:&Ledger<O,Q>,e:Error) -> bool {
        if !self.guard_valid(&ledger.history_view()) { self.guard_error(&ledger.history_view(),e) }
        else if !self.current(ledger) { e == Error::Stale }
        else if self.checkpoint.follow_view() is Signed { e == Error::Protocol("cannot edit a signed-only view") }
        else { !(ledger.permissions_view().rule(ledger.author_view()) is Unbounded)
            && e == Error::Protocol("local author requires unbounded write permission to edit") }
    }
    pub(crate) open spec fn edit_frame(&self,before:&Self) -> bool {
        self.checkpoint == before.checkpoint && self.source == before.source && self.protocol == before.protocol
            && self.dirty == before.dirty && self.last == before.last
            && self.supports_visibility_changes == before.supports_visibility_changes
    }
    #[verifier::prophetic]
    pub(crate) open spec fn begin_edit_outcome<M:Materializer,E,O,Q,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
        &self,before:&Self,ledger:&Ledger<O,Q>,initial:M,state:M,edit:F,out:Result<Option<Vec<u8>>,DriveError<M::Error,E>>) -> bool {
        self.edit_frame(before)
            && self.owner == if before.guard_valid(&ledger.history_view()) { Some(ledger.history_view().history_id()) } else { before.owner }
            && match out {
                Ok(bytes) => before.edit_ready(ledger) && !self.poisoned && edit_effect(edit,initial,state,Ok(bytes)),
                Err(DriveError::Edit(e)) => before.edit_ready(ledger) && self.poisoned && edit_effect(edit,initial,state,Err(e)),
                Err(DriveError::Stockfoil(e)) => !before.edit_ready(ledger) && before.edit_preflight_error(ledger,e)
                    && state == initial && self.poisoned == before.poisoned,
                _ => false,
            }
    }

    /// Run the edit only after all guards pass. A returned edit error poisons the
    /// driver; None or Some bytes preserve it apart from initial history binding.
    pub(crate) fn begin_edit<M:Materializer,E,O,Q,F:FnOnce(&mut M)->Result<Option<Vec<u8>>,E>>(
        &mut self,ledger:&Ledger<O,Q>,state:&mut M,edit:F) -> (out:Result<Option<Vec<u8>>,DriveError<M::Error,E>>)
        requires old(self).edit_ready(ledger) ==> forall|r:&mut M| *r == *old(state) ==> call_requires(edit,(r,)),
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).begin_edit_outcome(old(self),ledger,*old(state),*final(state),edit,out),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self;
        proof { self.representation(); self.checkpoint.representation(); self.current_facts(ledger); }
        self.check_history(ledger.history(),ledger.permissions())?;
        proof { self.representation(); self.checkpoint.representation(); self.current_facts(ledger); }
        if !self.is_current(ledger) { return Err(DriveError::Stockfoil(Error::Stale)); }
        if let Follow::Signed = self.follow() { return Err(DriveError::Stockfoil(Error::Protocol("cannot edit a signed-only view"))); }
        if let WritePermission::Unbounded = ledger.permissions().permission(&ledger.author()) {} else {
            return Err(DriveError::Stockfoil(Error::Protocol("local author requires unbounded write permission to edit")));
        }
        let result = edit(state);
        match result {
            Ok(bytes) => Ok(bytes),
            Err(error) => { self.poisoned = true; Err(DriveError::Edit(error)) },
        }
    }

    pub(crate) open spec fn poisoned_from(&self,before:&Self) -> bool {
        self.edit_frame(before) && self.owner == before.owner && self.poisoned
    }
    pub(crate) open spec fn finish_edit_outcome<A,E,O,Q>(&self,before:&Self,ledger:&Ledger<O,Q>,
        published:Result<Draft,Error>,out:Result<Option<Draft>,DriveError<A,E>>) -> bool {
        match published {
            Err(e) => out == Err(DriveError::Stockfoil(e)) && self.poisoned_from(before),
            Ok(draft) => match out {
                Ok(Some(d)) => d == draft && self.local_outcome(before,ledger,&draft,Ok(())),
                Err(DriveError::Stockfoil(e)) => exists|ack:Self| ack.local_outcome(before,ledger,&draft,Err(e)) && self.poisoned_from(&ack),
                _ => false,
            },
        }
    }
    /// A failure after editing poisons the driver, including a failed local
    /// acknowledgement after the document has already published the commit.
    pub(crate) fn finish_edit<A,E,O,Q>(&mut self,ledger:&Ledger<O,Q>,published:Result<Draft,Error>)
        -> (out:Result<Option<Draft>,DriveError<A,E>>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).finish_edit_outcome(old(self),ledger,published,out),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self;
        let draft = match published {
            Ok(draft) => draft,
            Err(e) => { self.poisoned = true; return Err(DriveError::Stockfoil(e)); },
        };
        let result = self.mark_local(ledger,&draft);
        let ghost ack = *self;
        match result {
            Ok(()) => {
                proof { self.local_success(&before,ledger,&draft,result); }
                Ok(Some(draft))
            },
            Err(e) => {
                self.poisoned = true;
                assert(ack.local_outcome(&before,ledger,&draft,Err(e)) && self.poisoned_from(&ack));
                assert(exists|a:Self| a.local_outcome(&before,ledger,&draft,Err(e)) && self.poisoned_from(&a));
                Err(DriveError::Stockfoil(e))
            },
        }
    }
}
} // verus!
