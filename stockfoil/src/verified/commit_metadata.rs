//! The actual metadata retained between parent validation, hashing and publication.
use super::{
    api_error::Error, author::Author, commit_hash::CommitHash, history::History, ledger::Ledger,
    publication::Prepared,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
#[derive(Debug)]
pub(crate) struct CommitMetadata {
    author: Author,
    parents: Vec<CommitHash>,
}
impl Clone for CommitMetadata {
    fn clone(&self) -> (out:Self)
        ensures out.author_view() == self.author_view(), out.parents_view() == self.parents_view(),
    { Self { author:self.author,parents:self.parents.clone() } }
}
impl CommitMetadata {
    pub closed spec fn author_view(&self) -> Author { self.author }
    pub closed spec fn parents_view(&self) -> Seq<CommitHash> { self.parents@ }
    pub(crate) fn author(&self) -> (out:&Author) ensures *out == self.author_view(), { &self.author }
    pub(crate) fn parents(&self) -> (out:&[CommitHash]) ensures out@ == self.parents_view(), { &self.parents }
    /// Attach the supplied digest without changing author or ordered parents.
    /// This does not assert that the digest authenticates those fields or bytes.
    pub(crate) fn finish(self,hash:CommitHash) -> (out:Prepared)
        ensures out.hash == hash, out.author == self.author_view(), out.parents@ == self.parents_view(),
    { Prepared { hash,author:self.author,parents:self.parents } }
}
impl Prepared {
    pub closed spec fn signing_bytes_view(&self) -> [u8;32] { self.hash.0 }
    pub(crate) proof fn signing_facts(&self) ensures self.signing_bytes_view() == self.hash.0, {}
    /// The bytes the author signs with Ed25519: the raw commit hash.
    pub fn signing_bytes(&self) -> (out:[u8;32]) ensures out == self.signing_bytes_view(), { self.hash.0 }
}
impl History {
    pub(crate) fn prepare_metadata(&self,author:Author,parents:Vec<CommitHash>) -> (out:Result<CommitMetadata,Error>)
        ensures match out {
            Ok(metadata) => metadata.author_view() == author && metadata.parents_view() == parents@
                && forall|i:int| 0 <= i < parents.len() ==> self.known(parents[i]),
            Err(e) => e matches Error::MissingParent(hash) && self.first_missing(parents@,hash),
        },
    {
        self.publication_parents(&parents)?;
        Ok(CommitMetadata { author,parents })
    }
}
impl<O,Q> Ledger<O,Q> {
    pub(crate) open spec fn selected_parents(&self,requested:Option<Vec<CommitHash>>,parents:Seq<CommitHash>) -> bool {
        match requested {
            Some(given) => parents == given@,
            None => parents.to_set() == self.history_view().delivered_heads_set()
                && parents.no_duplicates() && vstd::std_specs::btree::increasing_seq(parents),
        }
    }
    pub(crate) fn prepare_metadata(&self,requested:Option<Vec<CommitHash>>) -> (out:Result<CommitMetadata,Error>)
        ensures match out {
            Ok(metadata) => metadata.author_view() == self.author_view() && self.selected_parents(requested,metadata.parents_view())
                && forall|i:int| 0 <= i < metadata.parents_view().len() ==> self.history_view().known(metadata.parents_view()[i]),
            Err(e) => requested matches Some(parents) && e matches Error::MissingParent(hash)
                && self.history_view().first_missing(parents@,hash),
        },
    {
        proof { self.representation(); self.history.representation(); self.history.head_set_facts(); self.history.topology.head_hash_facts(); }
        let parents = match requested {
            Some(parents) => parents,
            None => self.history.captured_delivery_heads(),
        };
        assert(self.selected_parents(requested,parents@));
        proof {
            if requested is None {
                assert forall|i:int| 0 <= i < parents.len() implies self.history_view().known(parents[i]) by {
                    assert(parents@.to_set().contains(parents[i]));
                }
            }
        }
        self.history.prepare_metadata(self.author(),parents)
    }
}
} // verus!
