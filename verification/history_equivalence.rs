//! Replay depends on hash-identified parent edges and metadata, not storage layout.
//! These are relational theorems over actual History values, not new trust contracts.
use vstd::prelude::*;
use crate::{author::Author, commit_hash::CommitHash, history::History, graph,
    storage_refinement::parent_set, write_permission::WritePermission,
    write_permissions::WritePermissions, change_plan, materialization_source,
    view::{MaterializationCursor, ViewPoint}};

verus! {

pub(crate) open spec fn index(h: &History, hash: CommitHash) -> int { h.read_state().hashes[hash]@ as int }
pub(crate) open spec fn author(h: &History, hash: CommitHash) -> Author {
    h.owners_view()[h.read_state().labels[index(h,hash)] as int]
}
pub(crate) open spec fn payload(h: &History, hash: CommitHash) -> u64 {
    h.topology.nodes_view()[index(h,hash)].payload
}
pub(crate) open spec fn edge(h: &History, parent: CommitHash, child: CommitHash) -> bool {
    h.known(parent) && h.known(child) && h.graph_view()[index(h,child)].contains(index(h,parent))
}
/// Equal logical commits; node indices, chains, caches and History identities may differ.
pub(crate) open spec fn equivalent(a: &History, b: &History) -> bool {
    &&& a.topology.valid() && b.topology.valid()
    &&& a.read_state().hashes.dom() == b.read_state().hashes.dom()
    &&& forall|p: CommitHash,c: CommitHash| #[trigger] edge(a,p,c) == edge(b,p,c)
    &&& forall|h: CommitHash| a.known(h) ==> author(a,h) == author(b,h)
}

proof fn layout(h: &History)
    requires h.topology.valid(),
    ensures graph::dag(h.graph_view()), h.graph_view().len() == h.topology.nodes_view().len(),
        forall|x: CommitHash| h.known(x) ==> 0 <= index(h,x) < h.graph_view().len()
            && h.topology.nodes_view()[index(h,x)].hash == x,
        forall|i: int| 0 <= i < h.graph_view().len() ==> h.known(h.topology.nodes_view()[i].hash)
            && index(h,h.topology.nodes_view()[i].hash) == i,
{
    h.representation(); h.topology.hash_facts(); h.topology.traversal_facts();
}

pub(crate) proof fn symmetric(a: &History,b: &History)
    requires equivalent(a,b), ensures equivalent(b,a),
{}

/// Transfer parent paths by induction on the original node, without equating indices.
proof fn path(a: &History,b: &History, ancestor: CommitHash, descendant: CommitHash)
    requires equivalent(a,b), a.known(ancestor), a.known(descendant),
        graph::reaches(a.graph_view(),index(a,ancestor),index(a,descendant)),
    ensures graph::reaches(b.graph_view(),index(b,ancestor),index(b,descendant)),
    decreases index(a,descendant),
{
    assert(b.known(ancestor) && b.known(descendant));
    layout(a); layout(b);
    graph::unfold_reaches(a.graph_view(),index(a,ancestor),index(a,descendant));
    graph::unfold_reaches(b.graph_view(),index(b,ancestor),index(b,descendant));
    if ancestor != descendant {
        let p = choose|p: int| 0 <= p < index(a,descendant)
            && a.graph_view()[index(a,descendant)].contains(p)
            && graph::reaches(a.graph_view(),index(a,ancestor),p);
        let parent = a.topology.nodes_view()[p].hash;
        assert(edge(a,parent,descendant));
        path(a,b,ancestor,parent);
        // The mapped parent is earlier in the reconstructed DAG as well.
        assert(0 <= index(b,parent) < index(b,descendant));
    }
}

pub(crate) proof fn ancestry(a: &History,b: &History,x: CommitHash,y: CommitHash)
    requires equivalent(a,b),
    ensures a.strict_ancestor(x,y) == b.strict_ancestor(x,y),
        a.known(x) && a.known(y) ==> (graph::reaches(a.graph_view(),index(a,x),index(a,y))
            == graph::reaches(b.graph_view(),index(b,x),index(b,y))),
{
    layout(a); layout(b); symmetric(a,b);
    if a.known(x) && a.known(y) {
        if graph::reaches(a.graph_view(),index(a,x),index(a,y)) { path(a,b,x,y); }
        if graph::reaches(b.graph_view(),index(b,x),index(b,y)) { path(b,a,x,y); }
        if a.strict_ancestor(x,y) {
            graph::reaches_order(b.graph_view(),index(b,x),index(b,y));
        }
        if b.strict_ancestor(x,y) {
            graph::reaches_order(a.graph_view(),index(a,x),index(a,y));
        }
    }
}


proof fn causal_member(a: &History,b: &History,heads: Seq<CommitHash>,hash: CommitHash)
    requires equivalent(a,b), change_plan::known(&a.topology,heads), a.causal_from(heads).contains(hash),
    ensures b.causal_from(heads).contains(hash),
{
    a.representation(); b.representation();
    let ids = heads.map(|i: int,h: CommitHash| a.read_state().hashes[h]);
    let head = choose|head: int| parent_set(ids).contains(head)
        && graph::reaches(a.graph_view(),index(a,hash),head);
    let i = choose|i: int| 0 <= i < ids.len() && ids[i]@ == head;
    ancestry(a,b,hash,heads[i]);
    let other = heads.map(|i: int,h: CommitHash| b.read_state().hashes[h]);
    assert(other[i]@ == index(b,heads[i]));
    assert(parent_set(other).contains(index(b,heads[i])));
}

pub(crate) proof fn causal(a: &History,b: &History,heads: Seq<CommitHash>)
    requires equivalent(a,b), change_plan::known(&a.topology,heads),
    ensures change_plan::known(&b.topology,heads), a.causal_from(heads) == b.causal_from(heads),
{
    a.representation(); b.representation(); symmetric(a,b);
    assert forall|h: CommitHash| a.causal_from(heads).contains(h) == b.causal_from(heads).contains(h) by {
        if a.causal_from(heads).contains(h) { causal_member(a,b,heads,h); }
        if b.causal_from(heads).contains(h) { causal_member(b,a,heads,h); }
    }
    assert(a.causal_from(heads) =~= b.causal_from(heads));
}

proof fn authorized_member(a: &History,b: &History,heads: Seq<CommitHash>,permissions: &WritePermissions,hash: CommitHash)
    requires equivalent(a,b), change_plan::known(&a.topology,heads), a.authorized_from(heads,permissions).contains(hash),
    ensures b.authorized_from(heads,permissions).contains(hash),
{
    causal(a,b,heads);
    let selected_a = graph::history(a.graph_view(),parent_set(heads.map(|i: int,h: CommitHash| a.read_state().hashes[h])));
    let selected_b = graph::history(b.graph_view(),parent_set(heads.map(|i: int,h: CommitHash| b.read_state().hashes[h])));
    assert(a.causal_from(heads).contains(hash));
    assert(author(a,hash) == author(b,hash));
    match permissions.rule(author(a,hash)) {
        WritePermission::Denied => {},
        WritePermission::Unbounded => {},
        WritePermission::Bounded(frontier) => {
            let f = choose|f: int| crate::permission_refinement::resolved(a.read_state().hashes,frontier@).contains(f)
                && selected_a.contains(f) && graph::reaches(a.graph_view(),index(a,hash),f);
            let i = choose|i: int| 0 <= i < frontier.len() && a.known(frontier[i]) && index(a,frontier[i]) == f;
            assert(a.causal_from(heads).contains(frontier[i]));
            ancestry(a,b,hash,frontier[i]);
            assert(crate::permission_refinement::resolved(b.read_state().hashes,frontier@).contains(index(b,frontier[i])));
            assert(selected_b.contains(index(b,frontier[i])));
        },
    }
}

pub(crate) proof fn authorized(a: &History,b: &History,heads: Seq<CommitHash>,permissions: &WritePermissions)
    requires equivalent(a,b), change_plan::known(&a.topology,heads),
    ensures a.authorized_from(heads,permissions) == b.authorized_from(heads,permissions),
{
    causal(a,b,heads); symmetric(a,b);
    assert forall|h: CommitHash| a.authorized_from(heads,permissions).contains(h) == b.authorized_from(heads,permissions).contains(h) by {
        if a.authorized_from(heads,permissions).contains(h) { authorized_member(a,b,heads,permissions,h); }
        if b.authorized_from(heads,permissions).contains(h) { authorized_member(b,a,heads,permissions,h); }
    }
    assert(a.authorized_from(heads,permissions) =~= b.authorized_from(heads,permissions));
}

pub(crate) proof fn request(a: &History,b: &History,base: &MaterializationCursor,target: &ViewPoint,delivery: Seq<CommitHash>)
    requires equivalent(a,b), materialization_source::valid_request(a,base,target,delivery),
    ensures materialization_source::valid_request(b,base,target,delivery),
        materialization_source::requested(a,base,target,delivery) == materialization_source::requested(b,base,target,delivery),
        a.causal_from(base.delivery_view()) == b.causal_from(base.delivery_view()),
        a.authorized_from(base.target_view().heads_view(),&base.target_view().permissions_view()) == b.authorized_from(base.target_view().heads_view(),&base.target_view().permissions_view()),
        a.authorized_from(target.heads_view(),&target.permissions_view()) == b.authorized_from(target.heads_view(),&target.permissions_view()),
{
    causal(a,b,delivery); causal(a,b,base.delivery_view());
    causal(a,b,base.target_view().heads_view()); causal(a,b,target.heads_view());
    authorized(a,b,base.target_view().heads_view(),&base.target_view().permissions_view());
    authorized(a,b,target.heads_view(),&target.permissions_view());
}

/// Transfer the ready-node rule, including its minimum-hash tie breaker.
pub(crate) proof fn schedule(a: &History,b: &History,initial: Set<CommitHash>,prefix: Seq<CommitHash>)
    requires equivalent(a,b), change_plan::scheduled(a,initial,prefix),
    ensures change_plan::scheduled(b,initial,prefix),
{
    assert forall|x: CommitHash,y: CommitHash| a.strict_ancestor(x,y) == b.strict_ancestor(x,y) by { ancestry(a,b,x,y); }
    assert forall|remaining: Set<CommitHash>,h: CommitHash| change_plan::eligible(a,remaining,h) == change_plan::eligible(b,remaining,h) by {};
    assert forall|i: int| #![trigger prefix[i]] 0 <= i < prefix.len() implies {
        let remaining = initial.difference(prefix.take(i).to_set());
        change_plan::eligible(b,remaining,prefix[i]) && forall|h: CommitHash| change_plan::eligible(b,remaining,h) ==> change_plan::hash_le(prefix[i],h)
    } by {};
}

pub(crate) open spec fn logical(records: Seq<change_plan::Change>) -> Seq<(CommitHash,bool,crate::events::Vis)> {
    records.map(|i: int,c: change_plan::Change| (c.hash,c.newly_delivered,c.visibility))
}

pub(crate) proof fn records_equivalent(a: &History,b: &History,initial: Set<CommitHash>,left: Seq<change_plan::Change>,right: Seq<change_plan::Change>,
    delivered: Set<CommitHash>,before: Set<CommitHash>,after: Set<CommitHash>)
    requires equivalent(a,b), left.len() == right.len(),
        change_plan::scheduled(a,initial,change_plan::hashes(left)), change_plan::scheduled(b,initial,change_plan::hashes(right)),
        forall|i: int| 0 <= i < left.len() ==> change_plan::mapped(a,left[i],delivered,before,after),
        forall|i: int| 0 <= i < right.len() ==> change_plan::mapped(b,right[i],delivered,before,after),
    ensures logical(left) == logical(right),
        forall|i: int| 0 <= i < left.len() ==> left[i].payload == payload(a,left[i].hash) && right[i].payload == payload(b,right[i].hash),
{
    a.representation(); b.representation();
    schedule(a,b,initial,change_plan::hashes(left));
    materialization_source::schedule_prefix(b,initial,change_plan::hashes(left),change_plan::hashes(right),left.len());
    assert(change_plan::hashes(left).take(left.len() as int) =~= change_plan::hashes(left));
    assert(change_plan::hashes(right).take(right.len() as int) =~= change_plan::hashes(right));
    assert forall|i: int| 0 <= i < left.len() implies logical(left)[i] == logical(right)[i] by {
        assert(change_plan::hashes(left)[i] == left[i].hash);
        assert(change_plan::hashes(right)[i] == right[i].hash);
        assert(left[i].hash == right[i].hash);

    }
    assert(logical(left) =~= logical(right));
}

} // verus!
