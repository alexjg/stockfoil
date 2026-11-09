//! Checked users of local acknowledgement and its public query dependencies.
use vstd::prelude::*;
use vstd::std_specs::iter::IteratorSpec;
use crate::{api_error::Error, author::Author, checkpoint::{Checkpoint,Follow}, commit_hash::CommitHash,
    draft::Draft, history::History, ledger::Ledger, materialization_driver::Materialization,
    view::{MaterializationCursor,ViewPoint}, write_permission::WritePermission, write_permissions::WritePermissions};

verus! {

pub(crate) fn public_local<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,draft:&Draft) {
    let ghost before = *driver;
    let out = driver.mark_local(ledger,draft);
    proof { driver.local_facts(&before,ledger,draft,out); }
    match out {
        Ok(()) => {
            let current = driver.is_current(ledger);
            let dirty = driver.is_dirty();
            assert(current && dirty);
            assert(Materialization::draft_matches(ledger,draft) && before.sole_local(ledger,draft));
            assert(before.local_participates(ledger,draft));
            assert(driver.source is None && driver.protocol == before.protocol);
            assert(driver.checkpoint.rejected_view() == before.checkpoint.rejected_view());
        },
        Err(e) => {
            assert(driver.same_except_owner(&before));
            assert(driver.checkpoint == before.checkpoint && driver.poisoned == before.poisoned);
            assert(before.local_error(ledger,draft,e));
        },
    }
}

pub(crate) fn eligible_local<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,draft:&Draft)
    requires old(driver).guard_valid(&ledger.history_view()), old(driver).protocol.pending_value() is None,
        old(driver).checkpoint.progress_view() is None, !(old(driver).checkpoint.follow_view() is Signed),
        Materialization::draft_matches(ledger,draft), old(driver).sole_local(ledger,draft), old(driver).local_participates(ledger,draft),
        crate::materialization_driver::heads_known(&ledger.history_view(),old(driver).checkpoint.cursor_view().delivery_view()),
    ensures final(driver).current(ledger), final(driver).dirty_view(), final(driver).source is None,
        final(driver).guard_valid(&ledger.history_view()),
{
    let ghost before = *driver;
    let out = driver.mark_local(ledger,draft);
    proof { driver.local_facts(&before,ledger,draft,out); }
    assert(out is Ok);
}

pub(crate) fn collect_since<O,Q>(ledger:&Ledger<O,Q>,heads:&[CommitHash]) -> (out:Result<Vec<CommitHash>,Error>)
    ensures match out {
        Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !ledger.history_view().known(h),
        Ok(hashes) => hashes@.to_set() == ledger.history_view().read_state().hashes.dom().difference(ledger.history_view().causal_from(heads@))
            && hashes@.no_duplicates()
            && (forall|i:int,j:int| 0 <= i < j < hashes.len() ==> !ledger.history_view().strict_ancestor(hashes[j],hashes[i])),
    },
{
    let iter = ledger.commits_since(heads)?;
    proof { iter.sequence_facts(); }
    let hashes:Vec<CommitHash> = iter.collect();
    Ok(hashes)
}

/// No publication assumption: the fixture uses the actual public Ledger::commit.
/// The actual public acknowledgement then makes a nonempty pinned view current.
pub(crate) fn nonempty_local() {
    let hash = CommitHash([7;32]); let author = Author([1;32]);
    let history = History::new();
    proof { history.empty_facts(); history.representation(); }
    let permissions = WritePermissions::new().with_unlisted(WritePermission::Unbounded);
    let mut checkpoint = Checkpoint::new(Follow::Pinned(vec![]));
    proof { checkpoint.representation(); }
    checkpoint.cursor = MaterializationCursor::from_parts(Vec::new(),ViewPoint::from_parts(Vec::new(),permissions.clone()));
    proof { checkpoint.representation(); }
    let mut ledger = Ledger { history,author,permissions,options:(),policy_epoch:0,queue:() };
    let ghost empty = ledger;
    let prepared = crate::publication::Prepared { hash,author,parents:Vec::new() };
    let result = ledger.commit(prepared,42);
    proof {
        ledger.commit_facts(&empty,&prepared,42,result);
        let published = choose|published:Result<crate::publication::Delivered,Error>| ledger.history.publish_outcome(&empty.history,&prepared,42,published)
            && match published {
                Err(e) => result == Err(e),
                Ok(_) => result matches Ok(draft) && draft.hash == prepared.hash && draft.author == prepared.author && draft.parents@ == prepared.parents@,
            };
        ledger.history.publish_facts(&empty.history,&prepared,42,published);
        assert(published is Ok && published->Ok_0.newly_delivered);
        ledger.history.published_metadata(&empty.history,&prepared,42,published->Ok_0);
    }
    let draft = result.unwrap();
    let ghost root = ledger.history.read_state().hashes[hash];
    let mut driver = Materialization::resume(checkpoint,true);
    proof {
        ledger.representation(); ledger.history.representation(); ledger.history.topology.metadata_facts();
        driver.initial_facts(); driver.representation(); driver.checkpoint.representation();
        ledger.metadata_facts(hash);
        assert(ledger.history.known(hash));
        assert(ledger.history.event_parents(hash) =~= Seq::empty());
        assert(ledger.parents_at(hash) == Some(draft.parents@));
        assert(ledger.author_at(hash) == Some(author));
        assert(ledger.history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        crate::graph::unfold_reaches(ledger.history.graph_view(),root@ as int,root@ as int);
        let resolved = seq![hash].map(|i:int,h:CommitHash| ledger.history.read_state().hashes[h]);
        assert(resolved[0] == root);
        assert(crate::storage_refinement::parent_set(resolved).contains(root@ as int));
        assert(crate::graph::history(ledger.history.graph_view(),crate::storage_refinement::parent_set(resolved)).contains(root@ as int));
        ledger.history.topology.permission_facts();
        assert(ledger.history.authorized_from(seq![hash],&ledger.permissions) =~= Set::empty().insert(hash));
        assert(driver.guard_valid(&ledger.history_view()));
        assert(draft.parents@.to_set() == driver.checkpoint.cursor_view().target_view().heads_view().to_set());
        assert(ledger.history_view().read_state().hashes.dom().difference(ledger.history_view().causal_from(driver.checkpoint.cursor_view().delivery_view())) =~= Set::empty().insert(hash));
        assert(driver.checkpoint.cursor_view().target_view().permissions_view().same(&ledger.permissions_view()));
        assert(driver.sole_local(&ledger,&draft));
        assert(driver.local_participates(&ledger,&draft));
    }
    let ghost before = driver;
    let result = driver.mark_local(&ledger,&draft);
    proof { driver.local_facts(&before,&ledger,&draft,result); driver.current_facts(&ledger); driver.representation(); }
    assert(result is Ok);
    let current = driver.is_current(&ledger);
    assert(current);
    let heads = driver.heads();
    assert(heads@ =~= seq![hash]);
    let document = crate::document::Document { ledger,codec:(),payloads:Vec::new(),index:std::collections::HashMap::new(),staged:std::collections::HashMap::new() };
    proof { document.representation(); }
    crate::step_examples::current_checkpoint(&mut driver,&document,false);
}

pub(crate) fn poisoned_local<O,Q>(driver:&mut Materialization,ledger:&Ledger<O,Q>,draft:&Draft)
    requires old(driver).poisoned,
{
    let ghost before = *driver;
    let out = driver.mark_local(ledger,draft);
    proof { driver.local_facts(&before,ledger,draft,out); }
    assert(out == Err(Error::Poisoned));
    assert(*driver == before);
}

} // verus!
