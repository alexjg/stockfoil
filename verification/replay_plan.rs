//! Relational consumers of the actual public resume and planning APIs.
use vstd::prelude::*;
use crate::{author::Author, batch_protocol::{Batch,EventView}, change_plan::{self,Change},
    checkpoint::Checkpoint, commit_hash::CommitHash, history::History, history_equivalence as eq,
    ledger::Ledger, materialization_driver::{self,Materialization}, materialization_events as events,
    materialization_source::{self,Source}, materialization_start::Inputs,
    write_permissions::WritePermissions};

verus! {

/// Event sequence/order and semantic metadata, with parent order and local handles explicit below.
pub(crate) enum Observation {
    Reset,
    Deliver { hash: CommitHash, author: Author, parents: Set<CommitHash>, active: bool },
    Activate(CommitHash), Retract(CommitHash), Payload(CommitHash),
}
pub(crate) open spec fn observe(event: EventView<u64>) -> Observation {
    match event {
        EventView::Reset => Observation::Reset,
        EventView::Deliver {hash,author,parents,active,..} => Observation::Deliver {hash,author,parents:parents.to_set(),active},
        EventView::Activate {hash} => Observation::Activate(hash),
        EventView::Retract {hash} => Observation::Retract(hash),
        EventView::Payload {hash,..} => Observation::Payload(hash),
    }
}
pub(crate) open spec fn observations(events: Seq<EventView<u64>>) -> Seq<Observation> {
    events.map(|i: int,e: EventView<u64>| observe(e))
}

#[verifier::spinoff_prover]
#[verifier::rlimit(20)]
proof fn parent_members(history: &History,hash: CommitHash)
    requires history.topology.valid(),history.known(hash),
    ensures history.event_parents(hash).to_set() == history.read_state().hashes.dom().filter(|p: CommitHash| eq::edge(history,p,hash)),
{
    history.representation(); history.topology.parent_graph_facts(); history.topology.hash_facts();
    let nodes = history.topology.nodes_view(); let ids = history.topology.ids_view();
    let node = nodes[eq::index(history,hash)];
    let parents = crate::storage::parents(node,ids);
    let hashes = history.event_parents(hash);
    assert forall|p: CommitHash| hashes.to_set().contains(p) == eq::edge(history,p,hash) by {
        if hashes.to_set().contains(p) {
            let i = choose|i: int| 0 <= i < hashes.len() && hashes[i] == p;
            let n = parents[i]@ as int;
            assert(ids[node.parents_start + i]@ == n);
            assert(nodes[n].hash == p);
            assert(history.known(p) && eq::index(history,p) == n);
            assert(history.graph_view()[eq::index(history,hash)].contains(n));
        }
        if eq::edge(history,p,hash) {
            let j = choose|j: int| node.parents_start <= j < node.parents_start+node.parents_len && ids[j]@ == eq::index(history,p);
            assert(parents[j-node.parents_start] == ids[j]);
            assert(hashes[j-node.parents_start] == p);
            assert(hashes.to_set().contains(p));
        }
    }
    assert(hashes.to_set() =~= history.read_state().hashes.dom().filter(|p: CommitHash| eq::edge(history,p,hash)));
}

pub(crate) proof fn rejection_same(a: &History,b: &History,x: &Checkpoint,y: &Checkpoint,hash: CommitHash)
    requires eq::equivalent(a,b),x.rejected_view() == y.rejected_view(),
    ensures events::rejected(x,a,hash) == events::rejected(y,b,hash),
{
    assert forall|p: CommitHash| a.strict_ancestor(p,hash) == b.strict_ancestor(p,hash) by { eq::ancestry(a,b,p,hash); }
}

proof fn record_same(a: &History,b: &History,x: &Checkpoint,y: &Checkpoint,left: Change,right: Change)
    requires eq::equivalent(a,b),x.rejected_view() == y.rejected_view(),a.known(left.hash),
        left.hash == right.hash,left.newly_delivered == right.newly_delivered,left.visibility == right.visibility,
    ensures observations(events::record_event(a,x,left)) == observations(events::record_event(b,y,right)),
{
    rejection_same(a,b,x,y,left.hash);
    parent_members(a,left.hash); parent_members(b,right.hash);
    assert(a.event_parents(left.hash).to_set() =~= b.event_parents(right.hash).to_set());
    assert(observations(events::record_event(a,x,left)) =~= observations(events::record_event(b,y,right)));
}

pub(crate) proof fn translation_same(a: &History,b: &History,x: &Checkpoint,y: &Checkpoint,left: Seq<Change>,right: Seq<Change>)
    requires eq::equivalent(a,b),x.rejected_view() == y.rejected_view(),eq::logical(left) == eq::logical(right),
        forall|i: int| 0 <= i < left.len() ==> a.known(left[i].hash),
    ensures observations(events::translated(a,x,left)) == observations(events::translated(b,y,right)),
    decreases left.len(),
{
    assert(eq::logical(left).len() == left.len());
    assert(eq::logical(right).len() == right.len());
    if left.len() > 0 {
        assert forall|i: int| 0 <= i < left.len()-1 implies eq::logical(left.drop_last())[i] == eq::logical(right.drop_last())[i] by {
            assert(eq::logical(left)[i] == eq::logical(right)[i]);
            assert(left.drop_last()[i] == left[i]); assert(right.drop_last()[i] == right[i]);
        }
        assert(eq::logical(left.drop_last()) =~= eq::logical(right.drop_last()));
        translation_same(a,b,x,y,left.drop_last(),right.drop_last());
        assert(eq::logical(left)[left.len() as int-1] == eq::logical(right)[right.len() as int-1]);
        record_same(a,b,x,y,left.last(),right.last());
        let p = events::translated(a,x,left.drop_last()); let q = events::translated(b,y,right.drop_last());
        let l = events::record_event(a,x,left.last()); let r = events::record_event(b,y,right.last());
        assert(observations(p+l) =~= observations(p)+observations(l));
        assert(observations(q+r) =~= observations(q)+observations(r));
    } else { assert(left =~= Seq::<Change>::empty()); assert(right =~= Seq::<Change>::empty()); }
}


pub(crate) open spec fn payload_bound(history: &History,event: EventView<u64>) -> bool {
    match event {
        EventView::Deliver {hash,payload,..} | EventView::Payload {hash,payload} => history.known(hash) && payload == eq::payload(history,hash),
        _ => true,
    }
}

proof fn translation_payloads(history: &History,checkpoint: &Checkpoint,records: Seq<Change>)
    requires forall|i: int| 0 <= i < records.len() ==> history.known(records[i].hash) && records[i].payload == eq::payload(history,records[i].hash),
    ensures forall|i: int| 0 <= i < events::translated(history,checkpoint,records).len()
        ==> payload_bound(history,events::translated(history,checkpoint,records)[i]),
    decreases records.len(),
{
    if records.len() > 0 {
        translation_payloads(history,checkpoint,records.drop_last());
        assert(history.known(records.last().hash) && records.last().payload == eq::payload(history,records.last().hash));
        let prefix = events::translated(history,checkpoint,records.drop_last());
        assert forall|i: int| 0 <= i < events::translated(history,checkpoint,records).len()
            implies payload_bound(history,events::translated(history,checkpoint,records)[i]) by {
            if i < prefix.len() { assert(events::translated(history,checkpoint,records)[i] == prefix[i]); }
        }
    }
}

/// A valid saved partial traversal, including the binding references checked by resume/plan.
pub(crate) open spec fn resumable(history: &History,checkpoint: &Checkpoint) -> bool {
    let progress = checkpoint.progress_view().unwrap();
    &&& checkpoint.progress_view() is Some
    &&& materialization_driver::binding_known(history,checkpoint)
    &&& materialization_source::valid_request(history,&checkpoint.cursor_view(),&progress.target,progress.delivery_heads@)
    &&& progress.applied <= materialization_source::requested(history,&checkpoint.cursor_view(),&progress.target,progress.delivery_heads@).len()
}

proof fn copied_request(history: &History,checkpoint: &Checkpoint,input: &Inputs,permissions: &WritePermissions,supports: bool)
    requires resumable(history,checkpoint),input.matches(history,checkpoint,permissions,supports),
    ensures materialization_source::valid_request(history,&input.base,&input.target,input.delivery@),
        input.processed <= materialization_source::requested(history,&input.base,&input.target,input.delivery@).len(),
        materialization_source::requested(history,&input.base,&input.target,input.delivery@)
            == materialization_source::requested(history,&checkpoint.cursor_view(),&checkpoint.progress_view().unwrap().target,checkpoint.progress_view().unwrap().delivery_heads@),
{
    let progress = checkpoint.progress_view().unwrap();
    materialization_source::same_request(history,&checkpoint.cursor_view(),&input.base,&progress.target,&input.target,progress.delivery_heads@);
}

pub(crate) proof fn resumable_same(a: &History,b: &History,x: &Checkpoint,y: &Checkpoint)
    requires eq::equivalent(a,b),x.same(y),resumable(a,x),
    ensures resumable(b,y),
{
    assert forall|h: CommitHash| a.known(h) == b.known(h) by {};
    let px = x.progress_view().unwrap(); let py = y.progress_view().unwrap();
    eq::request(a,b,&x.cursor_view(),&px.target,px.delivery_heads@);
    materialization_source::same_request(b,&x.cursor_view(),&y.cursor_view(),&px.target,&py.target,px.delivery_heads@);
}

/// Actual public calls: validity is about saved heads/count, never assumed success.
pub(crate) fn resumed_plan<O,Q>(ledger: &Ledger<O,Q>,checkpoint: Checkpoint,supports: bool,budget: usize)
    -> (out: (Materialization,Batch))
    requires resumable(&ledger.history_view(),&checkpoint),budget > 0,
    ensures out.0.checkpoint_view() == checkpoint,
        out.0.protocol.accepts(&out.1),out.0.pending_checkpoint_matches_source(),
        exists|before: Materialization| before.fresh() && before.checkpoint_view() == checkpoint && before.supports_visibility() == supports
            && out.0.plan_outcome(&before,ledger,budget,Ok(Some(out.1))) && out.0.plan_errors(&before,ledger,budget,Ok(Some(out.1))),
{
    let mut driver = Materialization::resume(checkpoint,supports);
    proof { driver.initial_facts(); driver.representation(); }
    let ghost before = driver;
    let result = driver.plan(ledger,budget);
    proof {
        driver.plan_facts(&before,ledger,budget,result); driver.representation(); driver.current_facts(ledger);
        if result is Err {
            let e = result->Err_0;
            assert(!crate::materialization_start::input_error(&ledger.history_view(),&checkpoint,supports,e));
            let input = choose|input: Inputs| input.matches(&ledger.history_view(),&checkpoint,&ledger.permissions_view(),supports)
                && input.construction_error(&ledger.history_view(),e);
            copied_request(&ledger.history_view(),&checkpoint,&input,&ledger.permissions_view(),supports);
            assert(false);
        }
        assert(result is Ok && result->Ok_0 is Some);
    }
    let batch = result.unwrap().unwrap();
    assert(exists|prior: Materialization| prior.fresh() && prior.checkpoint_view() == checkpoint && prior.supports_visibility() == supports
        && driver.plan_outcome(&prior,ledger,budget,Ok(Some(batch))) && driver.plan_errors(&prior,ledger,budget,Ok(Some(batch))));
    (driver,batch)
}


proof fn origins_same(a: &History,b: &History,x: &Checkpoint,y: &Checkpoint,pa: &WritePermissions,pb: &WritePermissions,
    sa: bool,sb: bool,ia: &Inputs,ib: &Inputs,left: &Source,right: &Source)
    requires eq::equivalent(a,b),x.same(y),x.progress_view() is Some,
        ia.matches(a,x,pa,sa),ib.matches(b,y,pb,sb),ia.started(a,left),ib.started(b,right),
    ensures crate::replay_examples::related(left,right),
{
    eq::request(a,b,&ia.base,&ia.target,ia.delivery@);
    materialization_source::same_request(b,&ia.base,&ib.base,&ia.target,&ib.target,ia.delivery@);
    ia.base.target_view().same_authorization(&ib.base.target_view(),b);
    ia.target.same_authorization(&ib.target,b);
    let initial = materialization_source::requested(a,&ia.base,&ia.target,ia.delivery@);
    let p = choose|p: Seq<CommitHash>| p.len() == ia.processed && change_plan::scheduled(a,initial,p)
        && left.selected() == initial.difference(p.to_set());
    let q = choose|q: Seq<CommitHash>| q.len() == ib.processed && change_plan::scheduled(b,initial,q)
        && right.selected() == initial.difference(q.to_set());
    eq::schedule(a,b,initial,p);
    materialization_source::schedule_prefix(b,initial,p,q,p.len());
    assert(p.take(p.len() as int) =~= p); assert(q.take(p.len() as int) =~= q);
}


pub(crate) open spec fn same_counts(a: events::Counts,b: events::Counts) -> bool {
    a.delivered == b.delivered && a.activated == b.activated && a.retracted == b.retracted && a.rejected == b.rejected && a.rebuilt == b.rebuilt
}

proof fn count_same(left: Seq<EventView<u64>>,right: Seq<EventView<u64>>,tag: int)
    requires observations(left) == observations(right),
    ensures events::count(left,tag) == events::count(right,tag),
    decreases left.len(),
{
    assert(observations(left).len() == left.len()); assert(observations(right).len() == right.len());
    if left.len() > 0 {
        assert(observations(left.drop_last()) =~= observations(right.drop_last())) by {
            assert forall|i: int| 0 <= i < left.len()-1 implies observations(left.drop_last())[i] == observations(right.drop_last())[i] by {
                assert(observations(left)[i] == observations(right)[i]);
                assert(left.drop_last()[i] == left[i]); assert(right.drop_last()[i] == right[i]);
                assert(observations(left.drop_last())[i] == observe(left[i]));
                assert(observations(right.drop_last())[i] == observe(right[i]));
                assert(observations(left)[i] == observe(left[i]));
                assert(observations(right)[i] == observe(right[i]));
            }
        }
        count_same(left.drop_last(),right.drop_last(),tag);
        assert(observations(left)[left.len() as int-1] == observations(right)[right.len() as int-1]);
        assert(events::kind(left.last()) == events::kind(right.last()));
    }
}

#[verifier::spinoff_prover]
#[verifier::rlimit(20)]
pub(crate) proof fn plans_same<OA,QA,OB,QB>(left: &Materialization,right: &Materialization,before_a: &Materialization,before_b: &Materialization,
    a: &Ledger<OA,QA>,b: &Ledger<OB,QB>,x: &Batch,y: &Batch,budget: usize)
    requires eq::equivalent(&a.history_view(),&b.history_view()),before_a.fresh(),before_b.fresh(),
        before_a.checkpoint_view().same(&before_b.checkpoint_view()),before_a.checkpoint_view().progress_view() is Some,
        left.plan_outcome(before_a,a,budget,Ok(Some(*x))),left.plan_errors(before_a,a,budget,Ok(Some(*x))),
        right.plan_outcome(before_b,b,budget,Ok(Some(*y))),right.plan_errors(before_b,b,budget,Ok(Some(*y))),
    ensures observations(x.events_view()) == observations(y.events_view()),
        forall|i: int| 0 <= i < x.events_view().len() ==> payload_bound(&a.history_view(),x.events_view()[i]),
        forall|i: int| 0 <= i < y.events_view().len() ==> payload_bound(&b.history_view(),y.events_view()[i]),
        crate::replay_examples::related(&left.source.unwrap(),&right.source.unwrap()),
        left.protocol.pending_value().unwrap().next.same(&right.protocol.pending_value().unwrap().next),
        same_counts(left.protocol.pending_value().unwrap().counts,right.protocol.pending_value().unwrap().counts),
{
    before_a.initial_facts(); before_b.initial_facts(); before_a.representation(); before_b.representation();
    left.plan_facts(before_a,a,budget,Ok(Some(*x))); right.plan_facts(before_b,b,budget,Ok(Some(*y)));
    let (sa,ra) = choose|s: Source,r: Seq<Change>| left.issued_from(before_a,&a.history_view(),&a.permissions_view(),budget,x,s,r);
    let (sb,rb) = choose|s: Source,r: Seq<Change>| right.issued_from(before_b,&b.history_view(),&b.permissions_view(),budget,y,s,r);
    let ia = choose|i: Inputs| i.matches(&a.history_view(),&before_a.checkpoint,&a.permissions_view(),before_a.supports_visibility_changes)
        && i.started(&a.history_view(),&sa);
    let ib = choose|i: Inputs| i.matches(&b.history_view(),&before_b.checkpoint,&b.permissions_view(),before_b.supports_visibility_changes)
        && i.started(&b.history_view(),&sb);
    origins_same(&a.history_view(),&b.history_view(),&before_a.checkpoint,&before_b.checkpoint,&a.permissions_view(),&b.permissions_view(),
        before_a.supports_visibility_changes,before_b.supports_visibility_changes,&ia,&ib,&sa,&sb);
    eq::records_equivalent(&a.history_view(),&b.history_view(),sa.selected(),ra,rb,sa.delivered_members(),sa.before_members(),sa.after_members());
    translation_same(&a.history_view(),&b.history_view(),&before_a.checkpoint,&before_b.checkpoint,ra,rb);
    a.history_view().representation(); b.history_view().representation();
    translation_payloads(&a.history_view(),&before_a.checkpoint,ra);
    translation_payloads(&b.history_view(),&before_b.checkpoint,rb);
    assert(change_plan::hashes(ra) =~= change_plan::hashes(rb)) by {
        assert forall|i: int| 0 <= i < ra.len() implies change_plan::hashes(ra)[i] == change_plan::hashes(rb)[i] by {
            assert(eq::logical(ra)[i] == eq::logical(rb)[i]);
        }
    }
    assert(crate::replay_examples::related(&left.source.unwrap(),&right.source.unwrap()));
    count_same(x.events_view(),y.events_view(),0); count_same(x.events_view(),y.events_view(),1);
    count_same(x.events_view(),y.events_view(),2); count_same(x.events_view(),y.events_view(),3);
}

/// Public serialized checkpoint -> resume -> plan -> complete against independent Ledgers.
/// Current ledger policy and visibility-change support may differ: partial replay retains its capture.
pub(crate) fn public_partial_replay<OA,QA,OB,QB>(a: &Ledger<OA,QA>,b: &Ledger<OB,QB>,checkpoint: &Checkpoint,
    supports_a: bool,supports_b: bool,budget: usize)
    requires eq::equivalent(&a.history_view(),&b.history_view()),resumable(&a.history_view(),checkpoint),budget > 0,
{
    let copied = checkpoint.clone();
    let bytes = checkpoint.to_bytes(); let decoded = Checkpoint::from_bytes(&bytes).unwrap();
    proof {
        decoded.same_codec_data(checkpoint);
    }
    proof { resumable_same(&a.history_view(),&b.history_view(),checkpoint,&decoded); }
    // The first clone also preserves the semantic checkpoint fields used by validation.
    proof { materialization_source::same_request(&a.history_view(),&checkpoint.cursor_view(),&copied.cursor_view(),
        &checkpoint.progress_view().unwrap().target,&copied.progress_view().unwrap().target,checkpoint.progress_view().unwrap().delivery_heads@); }
    let (mut left,x) = resumed_plan(a,copied,supports_a,budget);
    let (mut right,y) = resumed_plan(b,decoded,supports_b,budget);
    proof {
        let before_a = choose|prior: Materialization| prior.fresh() && prior.checkpoint_view() == copied && prior.supports_visibility() == supports_a
            && left.plan_outcome(&prior,a,budget,Ok(Some(x))) && left.plan_errors(&prior,a,budget,Ok(Some(x)));
        let before_b = choose|prior: Materialization| prior.fresh() && prior.checkpoint_view() == decoded && prior.supports_visibility() == supports_b
            && right.plan_outcome(&prior,b,budget,Ok(Some(y))) && right.plan_errors(&prior,b,budget,Ok(Some(y)));
        plans_same(&left,&right,&before_a,&before_b,a,b,&x,&y,budget);
        before_a.initial_facts(); before_b.initial_facts();
        left.plan_facts(&before_a,a,budget,Ok(Some(x))); right.plan_facts(&before_b,b,budget,Ok(Some(y)));
        crate::replay_ack::active_safe(&a.history_view(),&left.protocol.pending_value().unwrap().next,x.events_view());
        crate::replay_ack::active_safe(&b.history_view(),&right.protocol.pending_value().unwrap().next,y.events_view());
    }
    assert(observations(x.events_view()) == observations(y.events_view()));
    assert(left.protocol.pending_value().unwrap().next.same(&right.protocol.pending_value().unwrap().next));
    crate::replay_ack::acknowledge(&mut left,a,x);
    crate::replay_ack::acknowledge(&mut right,b,y);
    assert(left.checkpoint.same(&right.checkpoint));
    assert(same_counts(left.last,right.last));
    proof { left.representation(); right.representation(); }
    let first = left.checkpoint().to_bytes(); let second = right.checkpoint().to_bytes();
    proof { left.checkpoint.encoding_deterministic(&right.checkpoint,first@,second@); }
    assert(first@ == second@);
}


/// The comparison retains author, parent membership, visibility and event order.
pub(crate) fn observation_fields() {
    let hash = CommitHash([7u8;32]); let alice = Author([1u8;32]); let bob = Author([2u8;32]);
    proof {
    assert(alice.0[0] != bob.0[0]);
    let parents = seq![hash];
    assert(parents[0] == hash); assert(parents.contains(hash));
    assert(parents.to_set().contains(hash));
    let event = EventView::Deliver { hash,author:alice,parents,payload:10u64,active:true };
    assert(observe(event) != observe(EventView::Deliver { hash,author:bob,parents,payload:10u64,active:true }));
    assert(observe(event) != observe(EventView::Deliver { hash,author:alice,parents:Seq::empty(),payload:10u64,active:true }));
    assert(observe(event) != observe(EventView::Deliver { hash,author:alice,parents,payload:10u64,active:false }));
    assert(observe(event) == observe(EventView::Deliver { hash,author:alice,parents,payload:100u64,active:true }));
    let first = seq![EventView::<u64>::Activate {hash},EventView::<u64>::Retract {hash}];
    let second = seq![EventView::<u64>::Retract {hash},EventView::<u64>::Activate {hash}];
    assert(observations(first)[0] != observations(second)[0]);
    assert(observations(first) != observations(second));
    }
}

} // verus!
