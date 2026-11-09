//! Actual metadata and causal-difference queries used by local acknowledgement.
use super::{
    api_error::Error,
    author::Author,
    commit_hash::CommitHash,
    commits::Commits,
    history::{Ancestry, History},
    ledger::Ledger,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
impl History {
    /// Delivered commits outside the causal past of `heads`, drafts included,
    /// regardless of permissions. Unknown heads are errors. Results are lazy,
    /// exact-sized and in canonical ancestor-first, smallest-ready-hash order.
    pub fn commits_since(&self, heads:&[CommitHash]) -> (out:Result<Commits<'_>,Error>)
        ensures match out {
            Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !self.known(h),
            Ok(iter) => (forall|i:int| 0 <= i < heads.len() ==> self.known(heads[i]))
                && iter.history_view() == self
                && iter.remaining_hashes() == self.read_state().hashes.dom().difference(self.causal_from(heads@)),
        },
    {
        proof { self.representation(); self.topology.established(); }
        let before = super::change_plan::delivery(&self.topology,heads)?;
        let after = self.topology.delivered_clock();
        proof { super::change_plan::causal_coordinates(&self.topology,&after,crate::head_refinement::all(self.graph_view())); }
        let iter = Commits::new(self,&after,&before);
        proof {
            self.topology.hash_facts(); self.topology.permission_facts(); self.topology.traversal_facts(); self.topology.metadata_facts();
            assert forall|h:CommitHash| iter.remaining_hashes().contains(h) ==
                self.read_state().hashes.dom().difference(self.causal_from(heads@)).contains(h) by {
                if self.known(h) {
                    let n = self.read_state().hashes[h];
                    assert(crate::head_refinement::all(self.graph_view()).contains(n@ as int));
                    assert(after.contents().contains_key(self.read_state().labels[n@ as int]));
                    assert(super::clock::below(after.lookup(self.read_state().labels[n@ as int]),n@ as usize));
                    super::commits::delta_at(&self.topology,&after,&before,self.read_state().hashes[h]);
                }
            }
            assert(iter.remaining_hashes() =~= self.read_state().hashes.dom().difference(self.causal_from(heads@)));
        }
        Ok(iter)
    }
}

impl<O,Q> Ledger<O,Q> {
    pub closed spec fn author_at(&self,hash:CommitHash) -> Option<Author> {
        if self.history_view().known(hash) { Some(self.history_view().event_author(hash)) } else { None }
    }
    pub closed spec fn parents_at(&self,hash:CommitHash) -> Option<Seq<CommitHash>> {
        if self.history_view().known(hash) { Some(self.history_view().event_parents(hash)) } else { None }
    }
    pub(crate) proof fn metadata_facts(&self,hash:CommitHash)
        ensures self.author_at(hash) == if self.history_view().known(hash) { Some(self.history_view().event_author(hash)) } else { None },
            self.parents_at(hash) == if self.history_view().known(hash) { Some(self.history_view().event_parents(hash)) } else { None },
    {}

    pub fn author_of(&self, hash:&CommitHash) -> (out:Option<Author>)
        ensures out == self.author_at(*hash),
    {
        proof { self.representation(); self.history.representation(); self.history.topology.established(); self.history.topology.metadata_facts(); }
        let idx = match self.history.topology.by_hash().get(hash) { Some(idx) => idx, None => return None };
        let node = &self.history.topology.nodes()[idx.index()];
        Some(self.history.topology.chain_authors()[node.chain as usize])
    }
    /// Ordered parents, preserving order and repeated entries.
    pub fn parents(&self, hash:&CommitHash) -> (out:Option<Vec<CommitHash>>)
        ensures match out { None => self.parents_at(*hash) is None,
            Some(parents) => self.parents_at(*hash) == Some(parents@) },
    {
        proof { self.representation(); self.history.representation(); }
        if !self.history.topology.by_hash().contains_key(hash) { return None; }
        let (_,parents) = self.history.event_metadata(hash);
        Some(parents)
    }
    pub fn ancestry(&self, heads:&[CommitHash]) -> (out:Result<Ancestry,Error>)
        ensures match out {
            Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !self.history_view().known(h),
            Ok(saved) => saved.history_id() == self.history_view().history_id()
                && (forall|i:int| 0 <= i < heads.len() ==> self.history_view().known(heads[i]))
                && saved.causal() == self.history_view().causal_from(heads@)
                && saved.authorized() == self.history_view().authorized_from(heads@,&self.permissions_view()),
        },
    { self.history().ancestry(heads,self.permissions()) }
    pub fn commits_since(&self, heads:&[CommitHash]) -> (out:Result<Commits<'_>,Error>)
        ensures match out {
            Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !self.history_view().known(h),
            Ok(iter) => (forall|i:int| 0 <= i < heads.len() ==> self.history_view().known(heads[i]))
                && *iter.history_view() == self.history_view()
                && iter.remaining_hashes() == self.history_view().read_state().hashes.dom().difference(self.history_view().causal_from(heads@)),
        },
    { self.history().commits_since(heads) }
}
} // verus!
