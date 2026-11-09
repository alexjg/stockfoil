//! Actual public publication callers, including successful nonempty growth.
use vstd::prelude::*;
use crate::{api_error::Error, author::Author, commit_hash::CommitHash,
    history::History, history_data::PayloadId, ledger::Ledger, publication::Prepared};
verus! {

pub(crate) fn public_publish(history:&mut History,prepared:Prepared,payload:PayloadId) {
    let ghost before = *history;
    let result = history.publish_commit(prepared,payload);
    proof { history.publish_facts(&before,&prepared,payload,result); }
    match result {
        Err(_) => { assert(*history == before); },
        Ok(delivered) => {
            assert(delivered.hash == prepared.hash);
            if delivered.newly_delivered {
                proof { history.published_metadata(&before,&prepared,payload,delivered); }
                assert(history.event_author(prepared.hash) == prepared.author);
                assert(history.event_parents(prepared.hash) == prepared.parents@);
                assert(history.read_state().extends(before.read_state()));
                assert(history.epoch == before.epoch + 1);
            } else { assert(*history == before); }
        },
    }
}

pub(crate) fn public_commit<O,Q>(ledger:&mut Ledger<O,Q>,prepared:Prepared,payload:PayloadId) {
    let ghost before = *ledger;
    let result = ledger.commit(prepared,payload);
    proof { ledger.commit_facts(&before,&prepared,payload,result); }
    match result {
        Err(_) => { assert(*ledger == before); },
        Ok(draft) => {
            assert(draft.hash == prepared.hash && draft.author == prepared.author && draft.parents@ == prepared.parents@);
            assert(ledger.permissions == before.permissions && ledger.policy_epoch == before.policy_epoch);
        },
    }
}

pub(crate) fn public_growth(author:Author,a:CommitHash,b:CommitHash)
    requires a != b,
{
    let mut history = History::new();
    proof { history.empty_facts(); history.representation(); }
    let ghost empty = history;
    let first = Prepared { hash:a,author,parents:Vec::new() };
    let result = history.publish_commit(first,11);
    proof { history.publish_facts(&empty,&first,11,result); }
    assert(result is Ok && result->Ok_0.newly_delivered);
    proof { history.published_metadata(&empty,&first,11,result->Ok_0); }
    assert(history.event_parents(a) =~= Seq::<CommitHash>::empty());
    proof { history.representation(); }
    assert(history.topology.nodes_view().len() == 1);
    assert(history.topology.ids_view().len() == 0);
    assert(history.topology.tips_view().len() <= 1);
    assert(!history.known(b));
    let ghost root = history;
    let second = Prepared { hash:b,author,parents:vec![a,a] };
    let result = history.publish_commit(second,12);
    proof { history.publish_facts(&root,&second,12,result); }
    assert(result is Ok && result->Ok_0.newly_delivered);
    proof { history.published_metadata(&root,&second,12,result->Ok_0); }
    assert(history.event_parents(b) =~= seq![a,a]);
    assert(history.epoch == 2);
    proof { history.representation(); }
    assert(history.known(a));
    assert(history.topology.nodes_view()[history.read_state().hashes[a]@ as int].payload == 11);
    let ghost grown = history;
    // Existing hashes win even over an invalid self-parent and a different payload.
    let duplicate = Prepared { hash:a,author,parents:vec![a] };
    let result = history.publish_commit(duplicate,99);
    proof { history.publish_facts(&grown,&duplicate,99,result); }
    assert(result is Ok && !result->Ok_0.newly_delivered && result->Ok_0.payload == 11);
    assert(history == grown);
    assert(history.event_parents(a) =~= Seq::<CommitHash>::empty());
}

pub(crate) fn wrong_author<O,Q>(ledger:&mut Ledger<O,Q>,prepared:Prepared,payload:PayloadId)
    requires prepared.author != old(ledger).author,
{
    let ghost before = *ledger;
    let result = ledger.commit(prepared,payload);
    proof { ledger.commit_facts(&before,&prepared,payload,result); }
    assert(result == Err(Error::Protocol("prepared commit has a different local author")));
    assert(*ledger == before);
}
}
