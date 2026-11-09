//! Actual source consumers across independently reconstructed histories.
use vstd::prelude::*;
use crate::{topology::Topology,clock::NodeIdx,author::Author,storage_refinement::parent_set};
use crate::{api_error::Error, change_plan, checkpoint::Checkpoint, commit_hash::CommitHash,
    history::History, history_equivalence as eq, materialization_source::{self,Source,SourceBatch},
    view::{MaterializationCursor,ViewPoint}};

verus! {

/// Corresponding live captures may belong to different History identities.
pub(crate) open spec fn related(a: &Source,b: &Source) -> bool {
    &&& a.ready() && b.ready()
    &&& a.base().same(&b.base()) && a.target().same(&b.target()) && a.delivery() == b.delivery()
    &&& a.selected() == b.selected() && a.processed() == b.processed() && a.total() == b.total()
    &&& a.delivered_members() == b.delivered_members() && a.before_members() == b.before_members() && a.after_members() == b.after_members()
    &&& a.reset_pending() == b.reset_pending() && a.finished() == b.finished()
}

/// The real constructors skip the same canonical prefix, even after reindexing.
pub(crate) fn reconstruct(a: &History,b: &History,base: MaterializationCursor,target: ViewPoint,
    delivery: Vec<CommitHash>,processed: u64,reset: bool) -> (result: Result<(Source,Source),Error>)
    requires eq::equivalent(a,b), materialization_source::valid_request(a,&base,&target,delivery@),
    ensures match result {
        Ok((left,right)) => related(&left,&right) && left.history_id() == a.history_id() && right.history_id() == b.history_id(),
        Err(e) => e is Malformed && processed > materialization_source::requested(a,&base,&target,delivery@).len(),
    },
{
    proof { eq::request(a,b,&base,&target,delivery@); }
    let base_copy = base.clone(); let target_copy = target.clone(); let delivery_copy = delivery.clone();
    proof { materialization_source::same_request(b,&base,&base_copy,&target,&target_copy,delivery@); }
    let ghost initial = materialization_source::requested(a,&base,&target,delivery@);
    let first = Source::new(a,base,target,delivery,processed,reset);
    let second = Source::new(b,base_copy,target_copy,delivery_copy,processed,reset);
    match first {
        Err(e) => { assert(second is Err); return Err(e); },
        Ok(left) => {
            let right = second.unwrap();
            let ghost p = choose|p: Seq<CommitHash>| p.len() == processed && change_plan::scheduled(a,initial,p)
                && left.selected() == initial.difference(p.to_set());
            let ghost q = choose|q: Seq<CommitHash>| q.len() == processed && change_plan::scheduled(b,initial,q)
                && right.selected() == initial.difference(q.to_set());
            proof {
                eq::schedule(a,b,initial,p);
                materialization_source::schedule_prefix(b,initial,p,q,processed as nat);
                assert(p.take(processed as int) =~= p); assert(q.take(processed as int) =~= q);
                base_copy.target_view().same_authorization(&base.target_view(),b);
                target_copy.same_authorization(&target,b);
            }
            assert(left.selected() == right.selected());
            Ok((left,right))
        },
    }
}

/// This transition preserves the relation, so it can be repeated for arbitrary budgets.
pub(crate) fn corresponding_batch(a: &History,b: &History,left: &mut Source,right: &mut Source,budget: usize)
    -> (out: (SourceBatch,SourceBatch))
    requires eq::equivalent(a,b), related(old(left),old(right)),
        old(left).history_id() == a.history_id(), old(right).history_id() == b.history_id(),
    ensures related(final(left),final(right)),
        final(left).history_id() == a.history_id(), final(right).history_id() == b.history_id(),
        out.0.reset == out.1.reset,
        eq::logical(out.0.records@) == eq::logical(out.1.records@),
        forall|i: int| 0 <= i < out.0.records.len() ==> out.0.records[i].payload == eq::payload(a,out.0.records[i].hash)
            && out.1.records[i].payload == eq::payload(b,out.1.records[i].hash),
{
    let ghost initial = left.selected();
    let ghost delivered = left.delivered_members(); let ghost before = left.before_members(); let ghost after = left.after_members();
    let x = left.advance(a,budget).unwrap(); let y = right.advance(b,budget).unwrap();
    proof {
        eq::records_equivalent(a,b,initial,x.records@,y.records@,delivered,before,after);
        assert forall|i: int| 0 <= i < x.records.len() implies change_plan::hashes(x.records@)[i] == change_plan::hashes(y.records@)[i] by {
            assert(eq::logical(x.records@)[i] == eq::logical(y.records@)[i]);
        }
        assert(change_plan::hashes(x.records@) =~= change_plan::hashes(y.records@));
    }
    (x,y)
}

/// Corresponding progress produces equal durable bytes despite different local payload IDs.
pub(crate) fn corresponding_checkpoints(left: &Source,right: &Source,published: &Checkpoint) -> (out: (Checkpoint,Checkpoint))
    requires related(left,right),
    ensures out.0.same(&out.1),
{
    let x = left.checkpoint(published).unwrap(); let y = right.checkpoint(published).unwrap();
    assert(x.same(&y));
    let first = x.to_bytes(); let second = y.to_bytes();
    proof { x.encoding_deterministic(&y,first@,second@); }
    assert(first@ == second@);
    let decoded = Checkpoint::from_bytes(&second).unwrap();
    proof { decoded.same_codec_data(&y); }
    assert(decoded.same(&x));
    (x,decoded)
}


/// A real partial checkpoint crosses the public codec and rebuilds the remaining
/// source in another history. The next batch and newly staged checkpoint agree.
pub(crate) fn persisted_partial_matches_uninterrupted(a: &History,b: &History,base: MaterializationCursor,
    target: ViewPoint,delivery: Vec<CommitHash>,published: &Checkpoint,first_budget: usize,next_budget: usize)
    requires eq::equivalent(a,b), materialization_source::valid_request(a,&base,&target,delivery@),
{
    proof { eq::request(a,b,&base,&target,delivery@); }
    let ghost initial = materialization_source::requested(a,&base,&target,delivery@);
    let mut live = Source::new(a,base,target,delivery,0,false).unwrap();
    proof {
        let p = choose|p: Seq<CommitHash>| p.len() == 0 && change_plan::scheduled(a,initial,p)
            && live.selected() == initial.difference(p.to_set());
        assert(p =~= Seq::<CommitHash>::empty());
        assert(live.selected() =~= initial);
    }
    let consumed = live.advance(a,first_budget).unwrap();
    let checkpoint = live.checkpoint(published).unwrap();
    let bytes = checkpoint.to_bytes();
    let decoded = Checkpoint::from_bytes(&bytes).unwrap();
    proof { decoded.same_codec_data(&checkpoint); decoded.representation(); }
    if !live.is_done() {
        let progress = decoded.in_progress.unwrap();
        proof {
            materialization_source::same_request(b,&base,&decoded.cursor,&target,&progress.target,delivery@);
            decoded.cursor.target_view().same_authorization(&base.target_view(),b);
            progress.target.same_authorization(&target,b);
        }
        let mut resumed = Source::new(b,decoded.cursor,progress.target,progress.delivery_heads,progress.applied,false).unwrap();
        proof {
            let p = choose|p: Seq<CommitHash>| p.len() == consumed.records.len() && change_plan::scheduled(b,initial,p)
                && resumed.selected() == initial.difference(p.to_set());
            eq::schedule(a,b,initial,change_plan::hashes(consumed.records@));
            materialization_source::schedule_prefix(b,initial,p,change_plan::hashes(consumed.records@),p.len());
            assert(p.take(p.len() as int) =~= p);
            assert(change_plan::hashes(consumed.records@).take(p.len() as int) =~= change_plan::hashes(consumed.records@));
            assert(related(&live,&resumed));
        }
        let pair = corresponding_batch(a,b,&mut live,&mut resumed,next_budget);
        assert(eq::logical(pair.0.records@) == eq::logical(pair.1.records@));
        let checkpoints = corresponding_checkpoints(&live,&resumed,published);
        assert(checkpoints.0.same(&checkpoints.1));
    } else {
        assert(decoded.progress_view() is None);
        assert(decoded.cursor_view().delivery_view() == delivery@);
        assert(decoded.cursor_view().target_view().same(&target));
    }
}

fn add(t: &mut Topology<u64>, hash: CommitHash, author: Author, parents: &[CommitHash],payload: u64) -> (idx: NodeIdx)
    requires old(t).valid(), old(t).nodes_view().len() < 100, old(t).ids_view().len() < 100,
        old(t).tips_view().len() < 100, parents.len() < 10, !old(t).hashes().contains_key(hash),
        forall|i: int| 0 <= i < parents.len() ==> old(t).hashes().contains_key(parents[i]),
    ensures final(t).valid(), final(t).lineage_id() == old(t).lineage_id(),
        final(t).history_id() == old(t).history_id(), idx@ == old(t).nodes_view().len(),
        final(t).nodes_view()[idx@ as int].payload == payload,
        final(t).read_state().extends(old(t).read_state()),
        final(t).head_hashes() == old(t).head_hashes().difference(parents@.to_set()).insert(hash),
        final(t).nodes_view().len() == old(t).nodes_view().len() + 1,
        final(t).tips_view().len() <= old(t).tips_view().len() + 1,
        final(t).ids_view().len() == old(t).ids_view().len() + parents.len(),
        final(t).hashes() == old(t).hashes().insert(hash, idx),
        final(t).graph() == old(t).graph().push(parent_set(parents@.map(|i: int, h: CommitHash| old(t).hashes()[h]))),
        final(t).nodes_view().subrange(0, old(t).nodes_view().len() as int) == old(t).nodes_view(),
        final(t).owners_view().subrange(0, old(t).owners_view().len() as int) == old(t).owners_view(),
        old(t).owners_view().len() <= final(t).owners_view().len() <= old(t).owners_view().len() + 1,
        final(t).nodes_view()[idx@ as int].chain < final(t).owners_view().len(),
        final(t).owners_view()[final(t).nodes_view()[idx@ as int].chain as int] == author,
{
    let idx = t.insert(hash, author, parents, payload, 0).unwrap();
    proof { t.permission_facts(); assert(idx@ < t.labels().len()); assert(t.nodes_view()[idx@ as int].chain == t.labels()[idx@ as int]); }
    idx
}


/// A concrete fork/join reconstruction swaps node/chain assignment and payload handles.
pub(crate) fn reordered_fork_join_witness() {
    let x = CommitHash([1;32]); let y = CommitHash([2;32]); let z = CommitHash([3;32]);
    let alice = Author([1;32]); let bob = Author([2;32]);
    assert(x.0[0] != y.0[0] && x.0[0] != z.0[0] && y.0[0] != z.0[0]);
    assert(x != y && x != z && y != z);
    let mut ta = Topology::new(); let mut tb = Topology::new();
    let ax = add(&mut ta,x,alice,&vec![],10); let ay = add(&mut ta,y,bob,&vec![],20);
    let pa = vec![x,y];
    let ghost ra = pa@.map(|i: int,h: CommitHash| ta.hashes()[h]);
    assert(ra =~= seq![ax,ay]);
    assert(parent_set(ra) =~= ISet::empty().insert(0int).insert(1int));
    let az = add(&mut ta,z,alice,&pa,30);
    let by = add(&mut tb,y,bob,&vec![],200); let bx = add(&mut tb,x,alice,&vec![],100);
    let pb = vec![y,x];
    let ghost rb = pb@.map(|i: int,h: CommitHash| tb.hashes()[h]);
    assert(rb =~= seq![by,bx]);
    assert(parent_set(rb) =~= ISet::empty().insert(0int).insert(1int));
    let bz = add(&mut tb,z,alice,&pb,300);
    let mut a = History::new(); let mut b = History::new();
    a.topology = ta; b.topology = tb;
    proof {
        a.representation(); b.representation(); a.topology.established(); b.topology.established();
        a.topology.permission_facts(); b.topology.permission_facts();
        assert(a.graph_view()[0] =~= ISet::<int>::empty()); assert(a.graph_view()[1] =~= ISet::<int>::empty());
        assert(b.graph_view()[0] =~= ISet::<int>::empty()); assert(b.graph_view()[1] =~= ISet::<int>::empty());
        assert(a.graph_view()[2] =~= ISet::empty().insert(0int).insert(1int));
        assert(b.graph_view()[2] =~= ISet::empty().insert(0int).insert(1int));
        assert forall|p: CommitHash,c: CommitHash| eq::edge(&a,p,c) == eq::edge(&b,p,c) by {};
        assert forall|h: CommitHash| a.known(h) implies eq::author(&a,h) == eq::author(&b,h) by {};
        assert(a.topology.valid() && b.topology.valid());
        assert(a.read_state().hashes.dom() =~= b.read_state().hashes.dom());
        assert(eq::equivalent(&a,&b));
        assert(eq::index(&a,x) != eq::index(&b,x));
        assert(eq::payload(&a,x) != eq::payload(&b,x));
    }
    let delivery = vec![z];
    let base = MaterializationCursor::new();
    let target = ViewPoint::from_parts(vec![z],crate::write_permissions::WritePermissions::default());
    proof {
        crate::graph::unfold_reaches(a.graph_view(),2,2);
        let resolved = delivery@.map(|i: int,h: CommitHash| a.read_state().hashes[h]);
        assert(resolved[0] == az);
        assert(parent_set(resolved).contains(2));
        assert(a.causal_from(delivery@).contains(z));
        assert(materialization_source::valid_request(&a,&base,&target,delivery@));
    }
    let checkpoint = Checkpoint::default().prepare_next(&base,&target,&delivery,0,&None);
    proof {
        materialization_source::same_request(&a,&base,&checkpoint.cursor_view(),&target,&checkpoint.progress_view().unwrap().target,delivery@);
        assert(crate::replay_plan::resumable(&a,&checkpoint));
    }
    persisted_partial_matches_uninterrupted(&a,&b,base,target,delivery,&Checkpoint::default(),1,2);
    let left = crate::ledger::Ledger { history:a,author:alice,options:(),permissions:crate::write_permissions::WritePermissions::default(),policy_epoch:0,queue:() };
    let right = crate::ledger::Ledger { history:b,author:bob,options:(),permissions:crate::write_permissions::WritePermissions::default().with_unlisted(crate::write_permission::WritePermission::Unbounded),policy_epoch:7,queue:() };
    proof { left.representation(); right.representation(); }
    crate::replay_plan::public_partial_replay(&left,&right,&checkpoint,true,false,1);
}

} // verus!
