//! Actual public in-memory commit, including native input conversion and hashing.
use super::{api_error::Error, commit_hash::CommitHash, document::Document, draft::Draft};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use {super::publication::Prepared, vstd::prelude::*};
verus! {
impl<C,O,Q> Document<C,O,Q> {
    pub(crate) open spec fn byte_commit_outcome(&self,before:&Self,bytes:Arc<[u8]>,parents:Option<Vec<CommitHash>>,out:Result<Draft,Error>) -> bool {
        (out matches Err(e) && *self == *before && (parents matches Some(ps)
            && before.ledger.history_view().preparation_error(ps@,e)))
        || exists|prepared:Prepared| prepared.author == before.ledger.author_view()
            && before.ledger.selected_parents(parents,prepared.parents@)
            && prepared.hash == super::commit_builder::identity(prepared.author,prepared.parents@,(*bytes)@)
            && (forall|i:int| 0 <= i < prepared.parents.len() ==> before.ledger.history_view().known(prepared.parents[i]))
            && self.publication_outcome(before,&prepared,bytes,out)
    }
    pub closed spec fn document_commit_outcome<T:Into<Arc<[u8]>>>(&self,before:&Self,canonical:T,parents:Option<Vec<CommitHash>>,out:Result<Draft,Error>) -> bool {
        exists|bytes:Arc<[u8]>| call_ensures(T::into,(canonical,),bytes) && self.byte_commit_outcome(before,bytes,parents,out)
    }
    pub(crate) proof fn committed_bytes(&self,before:&Self,bytes:Arc<[u8]>,parents:Option<Vec<CommitHash>>,out:Result<Draft,Error>)
        requires self.document_commit_outcome(before,bytes,parents,out),
        ensures self.byte_commit_outcome(before,bytes,parents,out),
    {
        let converted = choose|converted:Arc<[u8]>| call_ensures(<Arc<[u8]> as Into<Arc<[u8]>>>::into,(bytes,),converted)
            && self.byte_commit_outcome(before,converted,parents,out);
        assert(converted == bytes);
    }
    pub fn commit<T:Into<Arc<[u8]>>>(&mut self,canonical:T,parents:Option<Vec<CommitHash>>) -> (out:Result<Draft,Error>)
        ensures final(self).document_commit_outcome(old(self),canonical,parents,out),
    {
        let ghost before = *self;
        let convert = T::into;
        let bytes = convert(canonical);
        let result = self.ledger.begin_commit(parents);
        proof { self.ledger.builder_facts(parents,result); }
        let mut builder = match result {
            Ok(builder) => builder,
            Err(e) => {
                assert(call_ensures(T::into,(canonical,),bytes));
                assert(self.byte_commit_outcome(&before,bytes,parents,Err(e)));
                return Err(e);
            },
        };
        builder.update(&bytes);
        let prepared = builder.finish();
        proof { prepared.identity_facts(builder.author_view(),builder.parents_view(),super::commit_builder::identity(builder.author_view(),builder.parents_view(),builder.payload_view())); }
        let result = self.publish_prepared(prepared,bytes);
        assert(self.byte_commit_outcome(&before,bytes,parents,result));
        result
    }
}
} // verus!
