//! Acknowledge one authorized local delivery without echoing it to the application.
use super::{
    api_error::Error, checkpoint::Follow, commit_hash::CommitHash, commits::Commits, draft::Draft,
    ledger::Ledger, materialization_driver::Materialization, materialization_update::copy_heads,
    view::MaterializationCursor,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

proof fn normalized_unique(a:Seq<CommitHash>,b:Seq<CommitHash>)
    requires super::stdlib::hashes_sorted(a), super::stdlib::hashes_sorted(b),
        a.no_duplicates(), b.no_duplicates(), a.to_set() == b.to_set(),
    ensures a == b,
    decreases a.len(),
{
    if a.len() == 0 {
        if b.len() > 0 { assert(b.to_set().contains(b[0])); }
        assert(a =~= b);
    } else {
        assert(a.to_set().contains(a[0]));
        let j = choose|j:int| 0 <= j < b.len() && b[j] == a[0];
        assert(b.to_set().contains(b[0]));
        let i = choose|i:int| 0 <= i < a.len() && a[i] == b[0];
        if a[0] != b[0] {
            assert(i > 0 && j > 0);
            super::canonical_order::hash_asymmetric(a[0],b[0]);
        }
        assert(a[0] == b[0]);
        super::materialization_plan::tail_set(a); super::materialization_plan::tail_set(b);
        assert(a.drop_first().to_set() =~= b.drop_first().to_set());
        assert(super::stdlib::hashes_sorted(a.drop_first()));
        assert(super::stdlib::hashes_sorted(b.drop_first()));
        normalized_unique(a.drop_first(),b.drop_first());
        assert forall|k:int| 0 <= k < a.len() implies a[k] == b[k] by {
            if k > 0 { assert(a[k] == a.drop_first()[k-1]); assert(b[k] == b.drop_first()[k-1]); }
        }
        assert(a =~= b);
    }
}

fn same_head_set(a:&[CommitHash],b:&[CommitHash]) -> (out:bool)
    ensures out == (a@.to_set() == b@.to_set()),
{
    let mut left = copy_heads(a); let mut right = copy_heads(b);
    super::stdlib::normalize_hashes(&mut left); super::stdlib::normalize_hashes(&mut right);
    proof { if left@.to_set() == right@.to_set() { normalized_unique(left@,right@); } }
    super::write_permission::same_heads(&left,&right)
}

/// Two real iterator steps suffice to distinguish zero, one and many deliveries.
fn only_commit(input:Commits<'_>,hash:&CommitHash) -> (out:bool)
    ensures out == (input.remaining_hashes() == Set::empty().insert(*hash)),
{
    let mut iter = input;
    let ghost members = iter.remaining_hashes();
    let first = iter.next(); let second = iter.next();
    match (first,second) {
        (Some(found),None) => {
            assert(members =~= Set::empty().insert(found));
            if found.0 != hash.0 { return false; }
            assert(found.0 =~= hash.0);
            assert(found == *hash);
            true
        },
        _ => {
            assert(members != Set::empty().insert(*hash));
            false
        },
    }
}

impl Materialization {
    pub(crate) open spec fn draft_matches<O,Q>(ledger:&Ledger<O,Q>,draft:&Draft) -> bool {
        ledger.parents_at(draft.hash) == Some(draft.parents@) && ledger.author_at(draft.hash) == Some(draft.author)
    }
    pub(crate) open spec fn sole_local<O,Q>(&self,ledger:&Ledger<O,Q>,draft:&Draft) -> bool {
        draft.parents@.to_set() == self.checkpoint.cursor_view().target_view().heads_view().to_set()
        && ledger.history_view().read_state().hashes.dom().difference(ledger.history_view().causal_from(self.checkpoint.cursor_view().delivery_view()))
            == Set::empty().insert(draft.hash)
        && self.checkpoint.cursor_view().target_view().permissions_view().same(&ledger.permissions_view())
    }
    pub(crate) open spec fn local_participates<O,Q>(&self,ledger:&Ledger<O,Q>,draft:&Draft) -> bool {
        ledger.history_view().authorized_from(seq![draft.hash],&ledger.permissions_view()).contains(draft.hash)
        && !super::materialization_events::rejected(&self.checkpoint,&ledger.history_view(),draft.hash)
    }
    pub(crate) open spec fn local_error<O,Q>(&self,ledger:&Ledger<O,Q>,draft:&Draft,e:Error) -> bool {
        if !self.guard_valid(&ledger.history_view()) { self.guard_error(&ledger.history_view(),e) }
        else if self.protocol.pending_value() is Some || self.checkpoint.progress_view() is Some { e == Error::Stale }
        else if self.checkpoint.follow_view() is Signed { e == Error::Protocol("cannot edit a signed-only view") }
        else if !Self::draft_matches(ledger,draft) { e == Error::Protocol("draft metadata does not match the ledger") }
        else if !super::materialization_driver::heads_known(&ledger.history_view(),self.checkpoint.cursor_view().delivery_view()) {
            e matches Error::UnknownCommit(h) && !ledger.history_view().known(h) && self.checkpoint.cursor_view().delivery_view().contains(h)
        }
        else if !self.sole_local(ledger,draft) { e == Error::Stale }
        else { !self.local_participates(ledger,draft) && e == Error::Protocol("local draft does not participate") }
    }
    pub closed spec fn local_outcome<O,Q>(&self,before:&Self,ledger:&Ledger<O,Q>,draft:&Draft,out:Result<(),Error>) -> bool {
        self.local_details(before,ledger,draft,out)
    }
    pub(crate) open spec fn local_details<O,Q>(&self,before:&Self,ledger:&Ledger<O,Q>,draft:&Draft,out:Result<(),Error>) -> bool {
        match out {
            Err(e) => before.local_error(ledger,draft,e) && self.same_except_owner(before)
                && self.owner == if before.guard_valid(&ledger.history_view()) { Some(ledger.history_view().history_id()) } else { before.owner },
            Ok(()) => before.guard_valid(&ledger.history_view()) && before.protocol.pending_value() is None
                && before.checkpoint.progress_view() is None && !(before.checkpoint.follow_view() is Signed)
                && Self::draft_matches(ledger,draft)
                && super::materialization_driver::heads_known(&ledger.history_view(),before.checkpoint.cursor_view().delivery_view())
                && before.sole_local(ledger,draft) && before.local_participates(ledger,draft)
                && self.current(ledger) && self.source is None && self.dirty && !self.poisoned
                && self.owner == Some(ledger.history_view().history_id()) && self.protocol == before.protocol
                && self.last == before.last && self.supports_visibility_changes == before.supports_visibility_changes
                && self.checkpoint.rejected_view() == before.checkpoint.rejected_view()
                && match before.checkpoint.follow_view() {
                    Follow::Pinned(_) => self.checkpoint.follow_view() matches Follow::Pinned(heads) && heads@ == seq![draft.hash],
                    _ => self.checkpoint.follow_view() == before.checkpoint.follow_view(),
                },
        }
    }
    pub(crate) proof fn local_facts<O,Q>(&self,before:&Self,ledger:&Ledger<O,Q>,draft:&Draft,out:Result<(),Error>)
        requires self.local_outcome(before,ledger,draft,out),
        ensures self.local_details(before,ledger,draft,out),
    {}

    pub(crate) proof fn local_success<O,Q>(&self,before:&Self,ledger:&Ledger<O,Q>,draft:&Draft,out:Result<(),Error>)
        requires self.local_outcome(before,ledger,draft,out), out is Ok,
        ensures self.local_outcome(before,ledger,draft,Ok(())),
    {}

    /// State already reflects this local draft. Validate its exact stored metadata,
    /// parent-head set, unchanged policy, sole new delivery and authorization.
    /// Success records current progress without applying events and leaves the
    /// checkpoint dirty. Errors preserve the driver apart from initial history binding.
    pub fn mark_local(&mut self,ledger:&Ledger<impl Sized,impl Sized>,draft:&Draft) -> (out:Result<(),Error>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            final(self).local_outcome(old(self),ledger,draft,out),
            out is Ok ==> final(self).current(ledger) && final(self).dirty_view(),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self;
        proof { self.representation(); self.checkpoint.representation(); ledger.metadata_facts(draft.hash); }
        self.check_history(ledger.history(),ledger.permissions())?;
        proof { self.representation(); self.checkpoint.representation(); }
        if self.protocol.has_pending() || self.checkpoint.is_partial() { return Err(Error::Stale); }
        if let Follow::Signed = self.follow() { return Err(Error::Protocol("cannot edit a signed-only view")); }
        let parents = match ledger.parents(&draft.hash) { Some(parents) => parents, None => return Err(Error::Protocol("draft metadata does not match the ledger")) };
        let author = match ledger.author_of(&draft.hash) { Some(author) => author, None => return Err(Error::Protocol("draft metadata does not match the ledger")) };
        if !super::write_permission::same_heads(&parents,&draft.parents) || !author.same_as(&draft.author) {
            return Err(Error::Protocol("draft metadata does not match the ledger"));
        }
        let parents_match = same_head_set(&draft.parents,self.heads());
        let since = ledger.commits_since(self.checkpoint.cursor.delivery_heads())?;
        let sole = only_commit(since,&draft.hash);
        if !parents_match || !sole || self.checkpoint.cursor.view_point().write_permissions() != ledger.permissions() {
            return Err(Error::Stale);
        }
        let head = [draft.hash];
        assert(head@ =~= seq![draft.hash]);
        let capture = ledger.ancestry(&head)?;
        if !capture.participates(ledger.history(),&draft.hash)?
            || super::materialization_events::is_rejected(&self.checkpoint,ledger.history(),&draft.hash) {
            return Err(Error::Protocol("local draft does not participate"));
        }
        assert(before.sole_local(ledger,draft));
        assert(before.local_participates(ledger,draft));
        if let Follow::Pinned(_) = self.follow() { self.checkpoint.follow = Follow::Pinned(vec![draft.hash]); }
        proof { self.representation(); self.checkpoint.representation(); }
        self.checkpoint.cursor = MaterializationCursor::from_parts(ledger.history().captured_delivery_heads(),ledger.view(self.follow()));
        self.source = None;
        self.dirty = true;
        proof {
            self.representation(); self.checkpoint.representation(); self.current_facts(ledger);
            assert(self.current_details(ledger));
            assert(self.current(ledger));
            assert(self.local_details(&before,ledger,draft,Ok(())));
        }
        Ok(())
    }
}
} // verus!
