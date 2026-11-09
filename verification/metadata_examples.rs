//! Checked preparation consumers; supplied digests carry no authentication premise.
use vstd::prelude::*;
use crate::{api_error::Error, author::Author, commit_hash::CommitHash, history::History,
    ledger::Ledger, publication::Prepared, write_permissions::WritePermissions};
verus! {

pub(crate) fn arbitrary_preparation(history:&History,author:Author,parents:Vec<CommitHash>,digest:CommitHash) {
    let result = history.prepare_metadata(author,parents);
    match result {
        Err(e) => { assert(e matches Error::MissingParent(h) && history.first_missing(parents@,h)); },
        Ok(metadata) => {
            let copy = metadata.clone();
            let a = copy.author(); let p = copy.parents();
            assert(*a == author && p@ == parents@);
            let prepared = copy.finish(digest);
            assert(prepared.hash == digest && prepared.author == author && prepared.parents@ == parents@);
        },
    }
}

pub(crate) fn default_parents_always_exist<O,Q>(ledger:&Ledger<O,Q>) {
    let result = ledger.prepare_metadata(None);
    assert(result is Ok);
    let metadata = result.unwrap();
    assert(metadata.author_view() == ledger.author_view());
    assert(metadata.parents_view().to_set() == ledger.history_view().delivered_heads_set());
    assert(metadata.parents_view().no_duplicates());
    assert(vstd::std_specs::btree::increasing_seq(metadata.parents_view()));
}

pub(crate) fn explicit_empty_is_a_root<O,Q>(ledger:&Ledger<O,Q>,hash:CommitHash) {
    let metadata = ledger.prepare_metadata(Some(Vec::new())).unwrap();
    let prepared = metadata.finish(hash);
    assert(prepared.parents@ =~= Seq::empty());
    assert(prepared.author == ledger.author_view());
}

pub(crate) fn signing_tracks_public_hash(mut prepared:Prepared,replacement:CommitHash) {
    prepared.hash = replacement;
    let bytes = prepared.signing_bytes();
    proof { prepared.signing_facts(); }
    assert(bytes == replacement.0);
}

pub(crate) fn constructed_parent_selection(author:Author,root:CommitHash,digest:CommitHash) {
    let mut history = History::new();
    proof { history.empty_facts(); history.representation(); history.head_set_facts(); }
    let metadata = history.prepare_metadata(author,Vec::new()).unwrap();
    let prepared = metadata.finish(root);
    let ghost empty = history;
    let result = history.publish_commit(prepared,31);
    proof { history.publish_facts(&empty,&prepared,31,result); }
    assert(result is Ok && result->Ok_0.newly_delivered);
    proof { history.representation(); history.head_set_facts(); }
    assert(history.known(root));
    assert(history.delivered_heads_set() =~= Set::empty().insert(root));
    let ledger = Ledger { history,author,permissions:WritePermissions::default(),options:(),queue:(),policy_epoch:0 };
    proof { ledger.representation(); ledger.author_facts(); }
    let selected = ledger.prepare_metadata(None).unwrap();
    assert(selected.parents_view().to_set() =~= Set::empty().insert(root));
    let repeated = ledger.prepare_metadata(Some(vec![root,root])).unwrap();
    let repeated = repeated.finish(digest);
    assert(repeated.parents@ =~= seq![root,root]);
    assert(repeated.hash == digest && repeated.author == author);
    let fresh = ledger.prepare_metadata(Some(Vec::new())).unwrap();
    assert(fresh.parents_view().len() == 0);
}
}
