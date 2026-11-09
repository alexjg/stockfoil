//! Callers of the actual public planning API, generic over the real Ledger's untouched fields.
use vstd::prelude::*;
use crate::{author::Author, api_error::Error, batch_protocol::EventView,
    checkpoint::{Checkpoint, Follow}, commit_hash::CommitHash, history::History,
    ledger::Ledger, materialization_driver::Materialization, materialization_events as events,
    write_permissions::WritePermissions, write_permission::WritePermission};

verus! {

pub(crate) fn public_plan<Options, Queue>(driver: &mut Materialization, ledger: &Ledger<Options, Queue>, budget: usize) {
    let ghost before = *driver;
    let out = driver.plan(ledger, budget);
    proof { driver.plan_facts(&before, ledger, budget, out); }
    assert(driver.checkpoint == before.checkpoint);
    assert(driver.dirty == before.dirty && driver.last == before.last);
    match out {
        Err(_) => { assert(driver.protocol == before.protocol); },
        Ok(None) => { assert(driver.current(ledger) && driver.source is None); },
        Ok(Some(batch)) => {
            assert(budget > 0 && driver.protocol.accepts(&batch));
            assert(batch.events_view().len() <= budget);
            assert forall|i: int| #![trigger batch.events_view()[i]] 0 <= i < batch.events_view().len()
                && events::activation(batch.events_view()[i]) is Some implies
                driver.source.unwrap().after_members().contains(events::activation(batch.events_view()[i]).unwrap())
                    && !events::rejected(&before.checkpoint, &ledger.history_view(), events::activation(batch.events_view()[i]).unwrap()) by {}
        },
    }
}

pub(crate) fn zero_budget_binds<Options, Queue>(driver: &mut Materialization, ledger: &Ledger<Options, Queue>)
    requires old(driver).guard_valid(&ledger.history_view()),
{
    let ghost before = *driver;
    let out = driver.plan(ledger, 0);
    proof { driver.plan_facts(&before, ledger, 0, out); }
    assert(out is Err && out->Err_0 is Limit);
    assert(driver.owner == Some(ledger.history_view().history_id()));
    assert(driver.same_except_owner(&before));
}

pub(crate) fn pending_blocks_plan<Options, Queue>(driver: &mut Materialization, ledger: &Ledger<Options, Queue>, budget: usize)
    requires old(driver).guard_valid(&ledger.history_view()), old(driver).protocol.pending_value() is Some, budget > 0,
{
    let ghost before = *driver;
    let out = driver.plan(ledger, budget);
    proof { driver.plan_facts(&before, ledger, budget, out); }
    assert(out is Err && out->Err_0 is Protocol);
    assert(driver.same_except_owner(&before));
}

pub(crate) fn current_is_exact<Options, Queue>(driver: &Materialization, ledger: &Ledger<Options, Queue>) {
    let result = driver.is_current(ledger);
    proof { driver.current_facts(ledger); }
    assert(result ==> !driver.poisoned && driver.protocol.pending_value() is None && driver.checkpoint.progress_view() is None);
    assert(result ==> driver.checkpoint.cursor_view().delivery_view().to_set() == ledger.history_view().delivered_heads_set());
    assert(result ==> ledger.history_view().follow_target(&driver.checkpoint.follow_view(), &ledger.permissions_view(), &driver.checkpoint.cursor_view().target_view()));
}

pub(crate) fn public_equalities(a: &WritePermissions, b: &WritePermissions, view: &crate::view::ViewPoint, cursor: &crate::view::MaterializationCursor) {
    let cloned = a.clone();
    let equal = a == &cloned;
    proof { a.same_comparison(&cloned, a); }
    assert(equal);
    let compared = a == b;
    assert(compared == a.same(b));
    let vc = view.clone(); let cc = cursor.clone();
    let ve = view == &vc; let ce = cursor == &cc;
    assert(ve && ce);
}

/// A genuinely nonempty History; admission correctness is not a premise of planning.
pub(crate) fn nonempty_public_plan() {
    let hash = CommitHash([7; 32]); let author = Author([1; 32]);
    let mut topology = crate::topology::Topology::new();
    let root = crate::topology_examples::add(&mut topology, hash, author, &vec![]);
    let mut history = History::new(); history.topology = topology;
    let ledger = Ledger { history, author, options: (), permissions: WritePermissions::new().with_unlisted(WritePermission::Unbounded), policy_epoch: 0, queue: () };
    let mut driver = Materialization::new(Follow::Pinned(vec![hash]), true);
    proof {
        ledger.representation();
        driver.initial_facts(); driver.representation(); ledger.history.representation(); ledger.history.head_set_facts();
        assert(ledger.history.delivered_heads_set() =~= Set::empty().insert(hash));
        crate::graph::unfold_reaches(ledger.history.graph_view(), root@ as int, root@ as int);
        let selected = seq![hash];
        let resolved = selected.map(|i: int, h: CommitHash| ledger.history.read_state().hashes[h]);
        assert(resolved[0] == root);
        assert(crate::storage_refinement::parent_set(resolved).contains(root@ as int));
        assert(crate::graph::history(ledger.history.graph_view(), crate::storage_refinement::parent_set(resolved)).contains(root@ as int));
        assert(ledger.history.causal_from(selected) =~= Set::empty().insert(hash));
        ledger.history.topology.permission_facts();
        assert(ledger.history.authorized_from(selected, &ledger.permissions) =~= Set::empty().insert(hash));
        assert(ledger.history.causal_from(Seq::empty()) =~= Set::<CommitHash>::empty());
        assert(ledger.history.authorized_from(Seq::empty(), &driver.checkpoint.cursor_view().target_view().permissions_view()) =~= Set::<CommitHash>::empty());
        assert(ledger.history.known(hash));
        assert(crate::materialization_driver::binding_known(&ledger.history, &driver.checkpoint));
    }
    let ghost before = driver;
    let result = driver.plan(&ledger, 1);
    proof {
        driver.plan_facts(&before, &ledger, 1, result); driver.current_facts(&ledger);
    }
    // Error origins are precise enough to rule out binding, count, source and token errors.
    proof {
        if result is Err {
            let e = result->Err_0;
            assert(driver.source is None);
            assert(!crate::materialization_start::input_error(&ledger.history, &driver.checkpoint, true, e));
            let input = choose|input: crate::materialization_start::Inputs|
                input.matches(&ledger.history, &driver.checkpoint, &ledger.permissions, true) && input.construction_error(&ledger.history, e);
            ledger.history.representation();
            assert(driver.checkpoint.progress_view() is None);
            assert(input.matches(&ledger.history, &driver.checkpoint, &ledger.permissions, true));
            assert(input.delivery@.no_duplicates());
            input.delivery@.unique_seq_to_set();
            assert(input.delivery@.len() == 1);
            assert(input.delivery@.to_set().contains(input.delivery@[0]));
            assert(input.delivery@ =~= seq![hash]);
            assert(input.target.heads_view() =~= seq![hash]);
            assert(input.base.delivery_view() =~= Seq::<CommitHash>::empty());
            assert(input.base.target_view().heads_view() =~= Seq::<CommitHash>::empty());
            assert(crate::materialization_source::valid_request(&ledger.history, &input.base, &input.target, input.delivery@));
            assert(false);
        }
    }
    assert(result is Ok);
    assert(result->Ok_0 is Some);
    if let Some(batch) = result.unwrap() {
        assert(driver.protocol.accepts(&batch));
        assert(batch.events_view().len() <= 1);
        proof {
            assert(exists|origin: crate::materialization_source::Source, records: Seq<crate::change_plan::Change>|
                driver.issued_from(&before, &ledger.history, &ledger.permissions, 1, &batch, origin, records));
            let (origin, records) = choose|origin: crate::materialization_source::Source, records: Seq<crate::change_plan::Change>|
                driver.issued_from(&before, &ledger.history, &ledger.permissions, 1, &batch, origin, records);
            assert(before.chosen_source(&ledger.history, &ledger.permissions, &origin));
            let input = choose|input: crate::materialization_start::Inputs|
                input.matches(&ledger.history, &before.checkpoint, &ledger.permissions, true) && input.started(&ledger.history, &origin);
            assert(input.delivery@.no_duplicates()); input.delivery@.unique_seq_to_set();
            assert(input.delivery@.len() == 1);
            assert(input.delivery@.to_set().contains(input.delivery@[0]));
            assert(input.delivery@ =~= seq![hash]);
            assert(input.target.heads_view() =~= seq![hash]);
            assert(input.base.delivery_view() =~= Seq::<CommitHash>::empty());
            assert(input.base.target_view().heads_view() =~= Seq::<CommitHash>::empty());
            crate::materialization_source::same_request(&ledger.history, &input.base, &before.checkpoint.cursor_view(),
                &input.target, &input.target, input.delivery@);
            let requested = crate::materialization_source::requested(&ledger.history, &input.base, &input.target, input.delivery@);
            assert(requested =~= Set::empty().insert(hash));
            assert(origin.processed() == 0 && origin.total() == 1 && !origin.reset_pending());
            assert(records.len() == 1);
            assert(crate::change_plan::mapped(&ledger.history, records[0], origin.delivered_members(), origin.before_members(), origin.after_members()));
            assert(records[0].hash == hash && records[0].newly_delivered);
            assert(records[0].visibility is After);
            assert(records.drop_last() =~= Seq::empty());
            assert(batch.events_view().len() == 1);
            assert(batch.events_view()[0] is Deliver && batch.events_view()[0]->Deliver_hash == hash && batch.events_view()[0]->Deliver_active);
            assert(driver.source.unwrap().processed() == 1 && driver.source.unwrap().selected().is_empty());
            assert(driver.source.unwrap().finished());
            assert(driver.protocol.pending_value().unwrap().next.cursor_view().target_view().heads_view() =~= seq![hash]);
            assert(batch.events_view() =~= seq![batch.events_view()[0]]);
            assert(driver.pending_checkpoint_matches_source());
            assert(driver.protocol.pending_events() == batch.events_view());
            assert(batch.events_view().drop_last() =~= Seq::empty());
            assert(events::kind(batch.events_view().last()) == 1);
            reveal_with_fuel(events::count, 2);
            assert(events::count(batch.events_view(), 1) == 1);
            assert(driver.protocol.pending_value().unwrap().counts.delivered == 1);
        }
        crate::complete_examples::acknowledge_unrejected(&mut driver, &ledger, batch);
        let heads = driver.heads();
        proof { driver.representation(); }
        assert(heads@ =~= seq![hash]);
        assert(driver.last.delivered == 1 && driver.last.rejected == 0);
        let current = driver.is_current(&ledger);
        proof { driver.current_facts(&ledger); }
        assert(current);
        let document = crate::document::Document { ledger,codec:(),payloads:Vec::new(),index:std::collections::HashMap::new(),staged:std::collections::HashMap::new() };
        proof { document.representation(); }
        crate::step_examples::current_checkpoint(&mut driver,&document,false);

    }
}

} // verus!
