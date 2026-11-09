//! Actual public streaming and commit callers under the approved library boundary.
use vstd::prelude::*;
use crate::{api_error::Error, author::Author, commit_builder::{CommitBuilder,identity}, commit_hash::CommitHash,
    document::Document, draft::Draft, history::History, ledger::Ledger, publication::Prepared,
    write_permissions::WritePermissions};
use std::sync::Arc;
verus! {

pub(crate) fn chunking_and_clone(history:&History,author:Author,parents:Vec<CommitHash>,first:&[u8],last:&[u8]) {
    let result = history.begin_commit(author,parents);
    if let Ok(mut builder) = result {
        builder.update(first);
        let mut cloned = builder.clone();
        builder.update(last);
        cloned.update(last);
        let a = builder.finish(); let b = cloned.finish();
        proof {
            a.identity_facts(author,parents@,identity(author,parents@,first@+last@));
            b.identity_facts(author,parents@,identity(author,parents@,first@+last@));
        }
        assert(a.hash == b.hash && a.author == b.author && a.parents@ == b.parents@);
    }
}

pub(crate) fn public_default_builder<O,Q>(ledger:&Ledger<O,Q>) {
    let result = ledger.begin_commit(None);
    proof { ledger.builder_facts(None,result); }
    assert(result is Ok);
    let builder = result.unwrap();
    assert(builder.parents_view().to_set() == ledger.history_view().delivered_heads_set());
}

pub(crate) fn constructed_document_commit(author:Author,canonical:Vec<u8>) {
    let history = History::new();
    proof { history.empty_facts(); history.representation(); }
    let ledger = Ledger { history,author,permissions:WritePermissions::default(),options:(),queue:(),policy_epoch:0 };
    proof { ledger.representation(); ledger.author_facts(); }
    let mut document = Document { ledger,codec:(),payloads:Vec::new(),index:std::collections::HashMap::new(),staged:std::collections::HashMap::new() };
    let bytes = crate::stdlib::share_bytes(canonical);
    let parents = Some(Vec::new());
    let ghost before = document;
    let out = document.commit(bytes,parents);
    proof {
        document.committed_bytes(&before,bytes,parents,out);
        if out is Err && document == before && before.ledger.history_view().preparation_error(Seq::empty(),out->Err_0) {
            before.ledger.history.preparation_error_facts(Seq::empty(),out->Err_0);
            assert(false);
        }
        let prepared = choose|prepared:Prepared| prepared.author == before.ledger.author_view()
            && before.ledger.selected_parents(parents,prepared.parents@)
            && prepared.hash == identity(prepared.author,prepared.parents@,(*bytes)@)
            && (forall|i:int| 0 <= i < prepared.parents.len() ==> before.ledger.history_view().known(prepared.parents[i]))
            && document.publication_outcome(&before,&prepared,bytes,out);
        let id = choose|id:u64| document.stored_for_publication(&before,&prepared,bytes,id)
            && document.ledger.commit_outcome(&before.ledger,&prepared,id,out);
        document.ledger.commit_facts(&before.ledger,&prepared,id,out);
        let published = choose|p:Result<crate::publication::Delivered,Error>| document.ledger.history.publish_outcome(&before.ledger.history,&prepared,id,p)
            && match p { Err(e) => out == Err(e), Ok(_) => out matches Ok(d) && d.hash == prepared.hash && d.author == prepared.author && d.parents@ == prepared.parents@ };
        document.ledger.history.publish_facts(&before.ledger.history,&prepared,id,published);
        assert(published is Ok);
        assert(out is Ok);
        assert(out->Ok_0.hash == identity(author,Seq::empty(),canonical@));
        assert(document.payloads@ == seq![bytes] && document.index@[prepared.hash] == 0);
    }
}
}
