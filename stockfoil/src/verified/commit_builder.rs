//! The actual public streaming builder, checked against the approved BLAKE3 boundary.
use super::{
    api_error::Error, author::Author, blake3_boundary::Blake3, commit_hash::CommitHash,
    commit_metadata::CommitMetadata, history::History, ledger::Ledger, publication::Prepared,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {
pub(crate) open spec fn parent_bytes(parents:Seq<CommitHash>) -> Seq<u8>
    decreases parents.len(),
{
    if parents.len() == 0 { Seq::empty() }
    else { parent_bytes(parents.drop_last()) + parents.last().0@ }
}
pub closed spec fn identity(author:Author,parents:Seq<CommitHash>,payload:Seq<u8>) -> CommitHash {
    CommitHash(super::blake3_boundary::digest(super::commit_encoding::encoded(parents.len() as u64)
        + parent_bytes(parents) + author.0@ + super::blake3_boundary::digest(payload)@))
}
fn hash_metadata(parents:&[CommitHash],author:&Author,payload:&[u8;32]) -> (out:CommitHash)
    ensures out.0 == super::blake3_boundary::digest(super::commit_encoding::encoded(parents.len() as u64)
        + parent_bytes(parents@) + author.0@ + payload@),
{
    let mut hash = Blake3::new();
    let prefix = super::commit_encoding::encode(parents.len() as u64);
    hash.update(&prefix);
    let mut i = 0;
    while i < parents.len()
        invariant i <= parents.len(), prefix@ == super::commit_encoding::encoded(parents.len() as u64),
            hash.bytes() == prefix@ + parent_bytes(parents@.subrange(0,i as int)),
        decreases parents.len()-i,
    {
        hash.update(&parents[i].0);
        assert(parents@.subrange(0,i+1).drop_last() =~= parents@.subrange(0,i as int));
        assert(parents@.subrange(0,i+1).last() == parents[i as int]);
        i += 1;
    }
    assert(parents@.subrange(0,i as int) =~= parents@);
    hash.update(&author.0);
    hash.update(payload);
    CommitHash(hash.finish())
}

/// Hashes a local commit's canonical payload, preserving its author and ordered parents.
/// Created after parents have been validated, before any payload is read.
#[derive(Debug)]
pub struct CommitBuilder { metadata:CommitMetadata, payload:Blake3 }
impl Clone for CommitBuilder {
    fn clone(&self) -> (out:Self)
        ensures out.author_view() == self.author_view(), out.parents_view() == self.parents_view(),
            out.payload_view() == self.payload_view(),
    { Self { metadata:self.metadata.clone(),payload:self.payload.clone() } }
}
impl CommitBuilder {
    pub closed spec fn author_view(&self) -> Author { self.metadata.author_view() }
    pub closed spec fn parents_view(&self) -> Seq<CommitHash> { self.metadata.parents_view() }
    pub closed spec fn payload_view(&self) -> Seq<u8> { self.payload.bytes() }
    pub(crate) fn new(metadata:CommitMetadata) -> (out:Self)
        ensures out.author_view() == metadata.author_view(), out.parents_view() == metadata.parents_view(), out.payload_view().len() == 0,
    { Self { metadata,payload:Blake3::new() } }
    /// Append canonical payload bytes. The caller chooses the chunking.
    pub fn update(&mut self,canonical:&[u8])
        ensures final(self).author_view() == old(self).author_view(), final(self).parents_view() == old(self).parents_view(),
            final(self).payload_view() == old(self).payload_view() + canonical@,
    { self.payload.update(canonical); }
    pub fn finish(self) -> (out:Prepared)
        ensures out.matches_identity(self.author_view(),self.parents_view(),identity(self.author_view(),self.parents_view(),self.payload_view())),
    {
        let payload = self.payload.finish();
        let hash = hash_metadata(self.metadata.parents(),self.metadata.author(),&payload);
        self.metadata.finish(hash)
    }
}
impl Prepared {
    pub closed spec fn matches_identity(&self,author:Author,parents:Seq<CommitHash>,hash:CommitHash) -> bool {
        self.author == author && self.parents@ == parents && self.hash == hash
    }
    pub(crate) proof fn identity_facts(&self,author:Author,parents:Seq<CommitHash>,hash:CommitHash)
        requires self.matches_identity(author,parents,hash),
        ensures self.author == author, self.parents@ == parents, self.hash == hash,
    {}
}
impl History {
    pub closed spec fn preparation_error(&self,parents:Seq<CommitHash>,e:Error) -> bool {
        e matches Error::MissingParent(h) && self.first_missing(parents,h)
    }
    pub(crate) proof fn preparation_error_facts(&self,parents:Seq<CommitHash>,e:Error)
        requires self.preparation_error(parents,e),
        ensures e matches Error::MissingParent(h) && self.first_missing(parents,h),
    {}
    /// Every parent must be delivered. Parent order and repetitions are part of identity.
    pub fn begin_commit(&self,author:Author,parents:Vec<CommitHash>) -> (out:Result<CommitBuilder,Error>)
        ensures match out {
            Ok(builder) => builder.author_view() == author && builder.parents_view() == parents@ && builder.payload_view().len() == 0
                && forall|i:int| 0 <= i < parents.len() ==> self.known(parents[i]),
            Err(e) => self.preparation_error(parents@,e),
        },
    { Ok(CommitBuilder::new(self.prepare_metadata(author,parents)?)) }
}
impl<O,Q> Ledger<O,Q> {
    pub closed spec fn builder_outcome(&self,parents:Option<Vec<CommitHash>>,out:Result<CommitBuilder,Error>) -> bool {
        match out {
            Ok(builder) => builder.author_view() == self.author_view() && builder.payload_view().len() == 0
                && self.selected_parents(parents,builder.parents_view())
                && forall|i:int| 0 <= i < builder.parents_view().len() ==> self.history_view().known(builder.parents_view()[i]),
            Err(e) => parents matches Some(ps) && self.history_view().preparation_error(ps@,e),
        }
    }
    pub(crate) proof fn builder_facts(&self,parents:Option<Vec<CommitHash>>,out:Result<CommitBuilder,Error>)
        requires self.builder_outcome(parents,out),
        ensures match out {
            Ok(builder) => builder.author_view() == self.author_view() && builder.payload_view().len() == 0
                && self.selected_parents(parents,builder.parents_view())
                && forall|i:int| 0 <= i < builder.parents_view().len() ==> self.history_view().known(builder.parents_view()[i]),
            Err(e) => parents matches Some(ps) && self.history_view().preparation_error(ps@,e),
        },
    {}
    /// Feed canonical bytes to this builder and your storage in any chunking.
    pub fn begin_commit(&self,parents:Option<Vec<CommitHash>>) -> (out:Result<CommitBuilder,Error>)
        ensures self.builder_outcome(parents,out),
    { Ok(CommitBuilder::new(self.prepare_metadata(parents)?)) }
}
} // verus!
