//! Production materialization planning and emission, including captured permissions and batching.
use super::view::{MaterializationCursor, ViewPoint};
use super::{
    api_error::Error, commit_hash::CommitHash, history::History, owned_traversal::OwnedTraversal,
    topology::Topology, tree::Clock, write_permissions::WritePermissions,
};
#[cfg(verus_keep_ghost)]
use super::{
    captured,
    clock::{NodeIdx, below},
    commits, traversal_init,
};
use super::{
    events::{self, Vis},
    history_data::PayloadId,
    identity::Identity,
};
#[cfg(verus_keep_ghost)]
use crate::{capture_refinement::frozen, graph, storage_refinement::parent_set};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg(verus_keep_ghost)]
pub(crate) struct EmissionMeaning {
    pub state: crate::capture_refinement::ReadState,
    pub delivered: Set<CommitHash>,
    pub before: Set<CommitHash>,
    pub after: Set<CommitHash>,
}

/// One metadata change for a materializer. Contains no payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub hash: CommitHash,
    /// The retained logical payload ID.
    pub payload: PayloadId,
    /// Not delivered to this materializer before. Store it even if it does not
    /// participate now: a later view may enable it.
    pub newly_delivered: bool,
    /// Participation before (the cursor's view) and after (the target view).
    pub visibility: Vis,
}

/// Whether a change traversal has finished.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub enum Batch {
    More,
    /// Every record has been emitted. Persist this cursor together with the
    /// application effects of the whole traversal.
    Complete(MaterializationCursor),
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct ChangePlan {
    pub(crate) traversal: Option<OwnedTraversal>,
    identity: Identity,
    #[cfg(verus_keep_ghost)] meaning: Ghost<EmissionMeaning>,
    #[cfg(verus_keep_ghost)] receipt: Tracked<crate::lineage::Receipt>,
    pub(crate) delivered_before: Arc<Clock>,
    pub(crate) before: Clock,
    pub(crate) after: Clock,
}

pub(crate) open spec fn eligible(history: &History, selected: Set<CommitHash>, hash: CommitHash) -> bool {
    selected.contains(hash) && forall|a: CommitHash| selected.contains(a) && a != hash ==> !history.strict_ancestor(a, hash)
}

pub(crate) open spec fn hash_le(a: CommitHash, b: CommitHash) -> bool { a == b || super::stdlib::hash_less(a, b) }

/// Each record is the least ready hash after removing the preceding records.
pub(crate) open spec fn scheduled(history: &History, initial: Set<CommitHash>, hashes: Seq<CommitHash>) -> bool {
    forall|i: int| #![trigger hashes[i]] 0 <= i < hashes.len() ==> {
        let remaining = initial.difference(hashes.take(i).to_set());
        eligible(history, remaining, hashes[i]) && forall|h: CommitHash| eligible(history, remaining, h) ==> hash_le(hashes[i], h)
    }
}

proof fn schedule_push(history: &History, initial: Set<CommitHash>, hashes: Seq<CommitHash>, hash: CommitHash)
    requires scheduled(history, initial, hashes), eligible(history, initial.difference(hashes.to_set()), hash),
        forall|h: CommitHash| eligible(history, initial.difference(hashes.to_set()), h) ==> hash_le(hash, h),
    ensures scheduled(history, initial, hashes.push(hash)),
{
    assert forall|i: int| #![trigger hashes.push(hash)[i]] 0 <= i < hashes.push(hash).len() implies {
        let remaining = initial.difference(hashes.push(hash).take(i).to_set());
        eligible(history, remaining, hashes.push(hash)[i]) && forall|h: CommitHash| eligible(history, remaining, h) ==> hash_le(hashes.push(hash)[i], h)
    } by {
        if i < hashes.len() { assert(hashes.push(hash).take(i) =~= hashes.take(i)); }
        else { assert(hashes.push(hash).take(i) =~= hashes); }
    }
}

pub(crate) proof fn schedule_facts(history: &History, initial: Set<CommitHash>, hashes: Seq<CommitHash>)
    requires scheduled(history, initial, hashes),
    ensures hashes.no_duplicates(), hashes.to_set().subset_of(initial),
        forall|i: int, j: int| 0 <= i < j < hashes.len() ==> !history.strict_ancestor(hashes[j], hashes[i]),
{
    assert forall|i: int, j: int| 0 <= i < j < hashes.len() implies hashes[i] != hashes[j] by {
        assert(hashes.take(j)[i] == hashes[i]);
        assert(hashes.take(j).contains(hashes[i]));
    }
    assert(hashes.no_duplicates());
    assert forall|h: CommitHash| hashes.to_set().contains(h) implies initial.contains(h) by {
        let i = choose|i: int| 0 <= i < hashes.len() && hashes[i] == h;
        assert(eligible(history, initial.difference(hashes.take(i).to_set()), h));
    }
    assert forall|i: int, j: int| 0 <= i < j < hashes.len() implies !history.strict_ancestor(hashes[j], hashes[i]) by {
        assert(!hashes.take(i).contains(hashes[j])) by {
            if hashes.take(i).contains(hashes[j]) {
                let k = choose|k: int| 0 <= k < i && hashes[k] == hashes[j];
            }
        }
        assert(initial.contains(hashes[j]));
        assert(eligible(history, initial.difference(hashes.take(i).to_set()), hashes[i]));
    }
}

pub(crate) open spec fn hashes(records: Seq<Change>) -> Seq<CommitHash> { records.map(|i: int, c: Change| c.hash) }

pub(crate) open spec fn mapped(history: &History, change: Change, delivered: Set<CommitHash>, before: Set<CommitHash>, after: Set<CommitHash>) -> bool {
    &&& history.known(change.hash)
    &&& change.payload == history.topology.nodes_view()[history.topology.hashes()[change.hash]@ as int].payload
    &&& change.newly_delivered == !delivered.contains(change.hash)
    &&& events::before(change.visibility) == before.contains(change.hash)
    &&& events::after(change.visibility) == after.contains(change.hash)
}

pub(crate) open spec fn known<P>(t: &Topology<P>, heads: Seq<CommitHash>) -> bool {
    forall|i: int| 0 <= i < heads.len() ==> t.hashes().contains_key(heads[i])
}

pub(crate) open spec fn bounded<P>(t: &Topology<P>, clock: &Clock) -> bool {
    clock.ordered() && forall|c: u32| clock.contents().contains_key(c) ==> c < t.tables_view().len()
}

pub(crate) open spec fn records(delivered: Set<CommitHash>, old: Set<CommitHash>, before: Set<CommitHash>, after: Set<CommitHash>) -> Set<CommitHash> {
    delivered.difference(old).union(before.difference(after)).union(after.difference(before))
}

pub(crate) proof fn causal_coordinates<P>(t: &Topology<P>, clock: &Clock, covered: ISet<int>)
    requires t.valid(), clock.valid(), super::ancestry_walk::summarizes(clock.contents(), t.labels(), covered),
    ensures bounded(t, clock), forall|c: u32| clock.contents().contains_key(c) ==> clock.contents()[c]@ < t.labels().len(),
{
    t.permission_facts();
    assert forall|c: u32| clock.contents().contains_key(c) implies c < t.tables_view().len() by {
        let n = clock.contents()[c];
        t.traversal_node(n);
        assert(t.nodes_view()[n@ as int].chain == c);
    }
}

pub(crate) fn delivery<P>(t: &Topology<P>, heads: &[CommitHash]) -> (out: Result<Arc<Clock>, Error>)
    ensures match out {
        Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !t.hashes().contains_key(h),
        Ok(clock) => known(t, heads@) && bounded(t, &clock) && super::permissions::coordinates(t, &clock)
            && frozen(&clock, t.read_state(), captured::causal(t, heads@)),
    },
{
    proof { t.established(); }
    let reached = match t.history_clock(heads) { Ok(c) => c, Err(h) => return Err(Error::UnknownCommit(h)) };
    proof {
        t.permission_facts();
        causal_coordinates(t, &reached, graph::history(t.graph(), parent_set(heads@.map(|i: int, h: CommitHash| t.hashes()[h]))));
        assert forall|h: CommitHash| t.hashes().contains_key(h) implies captured::causal(t, heads@).contains(h)
            == below(reached.lookup(t.labels()[t.hashes()[h]@ as int]), t.hashes()[h]@ as usize) by {};
    }
    Ok(reached)
}

pub(crate) fn participation<P>(t: &Topology<P>, heads: &[CommitHash], permissions: &WritePermissions) -> (out: Result<Clock, Error>)
    ensures match out {
        Err(e) => e matches Error::UnknownCommit(h) && heads@.contains(h) && !t.hashes().contains_key(h),
        Ok(clock) => known(t, heads@) && bounded(t, &clock) && super::permissions::coordinates(t, &clock)
            && frozen(&clock, t.read_state(), captured::authorized(t, heads@, permissions)),
    },
{
    proof { t.established(); }
    let (_reached, participating) = match super::permissions::capture_clocks(t, heads, permissions) {
        Ok(pair) => pair, Err(h) => return Err(Error::UnknownCommit(h)),
    };
    proof {
        t.permission_facts();
        causal_coordinates(t, &_reached, graph::history(t.graph(), parent_set(heads@.map(|i: int, h: CommitHash| t.hashes()[h]))));
        assert forall|c: u32| participating.contents().contains_key(c) implies
            c < t.tables_view().len() && participating.contents()[c]@ < t.labels().len() by {
            assert(_reached.contents().contains_key(c));
        }
        assert forall|h: CommitHash| t.hashes().contains_key(h) implies captured::authorized(t, heads@, permissions).contains(h)
            == below(participating.lookup(t.labels()[t.hashes()[h]@ as int]), t.hashes()[h]@ as usize) by {};
    }
    Ok(participating)
}

/// Check the supplied captured frontier, not the History's current delivery tips.
fn covers<P>(t: &Topology<P>, delivered: &Clock, heads: &[CommitHash],
    #[cfg(verus_keep_ghost)] Ghost(members): Ghost<Set<CommitHash>>) -> (out: bool)
    requires t.valid(), known(t, heads@), frozen(delivered, t.read_state(), members),
    ensures out == heads@.to_set().subset_of(members),
{
    proof { t.permission_facts(); }
    let mut i = 0;
    while i < heads.len()
        invariant i <= heads.len(), t.valid(), known(t, heads@), frozen(delivered, t.read_state(), members),
            forall|j: int| 0 <= j < i ==> members.contains(heads[j]),
        decreases heads.len() - i,
    {
        proof { t.permission_facts(); }
        let hash = heads[i];
        let idx = t.by_hash().delivered(&hash);
        if !delivered.contains(t.nodes()[idx.index()].chain, idx) {
            assert(!members.contains(hash));
            assert(heads@.to_set().contains(hash));
            return false;
        }
        i += 1;
    }
    assert(heads@.to_set().subset_of(members)) by {
        assert forall|h: CommitHash| heads@.to_set().contains(h) implies members.contains(h) by {
            let j = choose|j: int| 0 <= j < heads.len() && heads[j] == h;
        }
    }
    true
}

/// Causal closure lifts checked head coverage to all records below those heads.
pub(crate) proof fn covered_history<P>(t: &Topology<P>, heads: Seq<CommitHash>, delivered: Seq<CommitHash>)
    requires t.valid(), known(t, heads), known(t, delivered), heads.to_set().subset_of(captured::causal(t, delivered)),
    ensures captured::causal(t, heads).subset_of(captured::causal(t, delivered)),
{
    assert forall|h: CommitHash| captured::causal(t, heads).contains(h) implies captured::causal(t, delivered).contains(h) by {
        let n = t.hashes()[h]@ as int;
        let resolved = heads.map(|i: int, head: CommitHash| t.hashes()[head]);
        let head = choose|head: int| parent_set(resolved).contains(head) && graph::reaches(t.graph(), n, head);
        let i = choose|i: int| 0 <= i < resolved.len() && resolved[i]@ == head;
        assert(heads.to_set().contains(heads[i]));
        graph::history_closed(t.graph(), parent_set(delivered.map(|i: int, h: CommitHash| t.hashes()[h])), n, head);
    }
}

impl ChangePlan {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        &&& self.identity.lineage() == self.receipt@.id()
        &&& self.receipt@.state() == self.meaning@.state
        &&& frozen(&self.delivered_before, self.meaning@.state, self.delivered_members())
        &&& frozen(&self.before, self.meaning@.state, self.before_members())
        &&& frozen(&self.after, self.meaning@.state, self.after_members())
        &&& self.traversal is Some ==> self.traversal.unwrap().history_id() == self.history_id()
    }
    pub(crate) closed spec fn selected(&self) -> Set<CommitHash> { match self.traversal { None => Set::empty(), Some(t) => t.remaining_hashes() } }
    pub(crate) closed spec fn history_id(&self) -> u64 { self.identity.raw() }
    pub(crate) closed spec fn delivery_clock(&self) -> Clock { *self.delivered_before }
    pub(crate) closed spec fn before_clock(&self) -> Clock { self.before }
    pub(crate) closed spec fn after_clock(&self) -> Clock { self.after }
    pub(crate) closed spec fn delivered_members(&self) -> Set<CommitHash> { self.meaning@.delivered }
    pub(crate) closed spec fn before_members(&self) -> Set<CommitHash> { self.meaning@.before }
    pub(crate) closed spec fn after_members(&self) -> Set<CommitHash> { self.meaning@.after }

    pub(crate) fn remaining(&self) -> (out: usize) ensures out == self.selected().len(),
    { match &self.traversal { None => 0, Some(t) => t.remaining() } }

    pub(crate) fn check_history(&self, history: &History) -> (out: Result<(), Error>)
        ensures match out {
            Ok(()) => self.history_id() == history.history_id() && self.identity_lineage() == history.topology.lineage_id(),
            Err(e) => e is WrongHistory && self.history_id() != history.history_id(),
        },
    {
        proof { history.topology.established(); history.representation(); }
        if self.identity.same(history.topology.identity()) { Ok(()) } else { Err(Error::WrongHistory) }
    }
    pub(crate) closed spec fn identity_lineage(&self) -> vstd::resource::Loc { self.identity.lineage() }

    /// Emit metadata from the captured clocks, even when History has grown.
    pub(crate) fn next(&mut self, history: &History) -> (out: Result<Option<Change>, Error>)
        ensures final(self).history_id() == old(self).history_id(),
            final(self).delivered_members() == old(self).delivered_members(),
            final(self).before_members() == old(self).before_members(), final(self).after_members() == old(self).after_members(),
            match out {
                Err(e) => e is WrongHistory && old(self).history_id() != history.history_id() && *final(self) == *old(self),
                Ok(None) => old(self).history_id() == history.history_id() && old(self).selected().is_empty() && final(self).selected().is_empty(),
                Ok(Some(change)) => old(self).history_id() == history.history_id() && old(self).selected().contains(change.hash)
                    && final(self).selected() == old(self).selected().remove(change.hash)
                    && mapped(history, change, old(self).delivered_members(), old(self).before_members(), old(self).after_members())
                    && eligible(history, old(self).selected(), change.hash)
                    && (forall|h: CommitHash| eligible(history, old(self).selected(), h) ==> hash_le(change.hash, h)),
            },
    {
        match self.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
        proof {
            use_type_invariant(&*self); history.topology.established(); history.representation();
            history.topology.compatible(self.receipt.borrow()); history.topology.permission_facts();
        }
        let ghost previous = *self;
        let mut traversal = match self.traversal.take() { None => return Ok(None), Some(t) => t };
        let result = traversal.next(history);
        self.traversal = Some(traversal);
        let Some(idx) = result.unwrap() else { return Ok(None); };
        let node = &history.topology.nodes()[idx.index()];
        proof {
            history.topology.traversal_facts();
            assert forall|h: CommitHash| eligible(history, previous.selected(), h) implies hash_le(node.hash, h) by {
                assert(previous.traversal.unwrap().eligible(history, h));
            }
            NodeIdx::same_index(history.topology.hashes()[node.hash], idx);
            crate::capture_refinement::frozen_membership(&self.delivered_before, self.meaning@.state, history.topology.read_state(), self.delivered_members(), node.hash);
            crate::capture_refinement::frozen_membership(&self.before, self.meaning@.state, history.topology.read_state(), self.before_members(), node.hash);
            crate::capture_refinement::frozen_membership(&self.after, self.meaning@.state, history.topology.read_state(), self.after_members(), node.hash);
        }
        let newly_delivered = !self.delivered_before.contains(node.chain, idx);
        let was_visible = self.before.contains(node.chain, idx);
        let is_visible = self.after.contains(node.chain, idx);
        Ok(Some(Change { hash: node.hash, payload: node.payload, newly_delivered,
            visibility: events::classify(was_visible, is_visible) }))
    }

    /// Inputs may be arbitrary persisted heads and arbitrary permissions.
    /// Unknown heads and an insufficient captured delivery frontier are rejected.
    pub(crate) fn new(history: &History, old_delivery: &[CommitHash], before_heads: &[CommitHash], before_permissions: &WritePermissions,
        after_heads: &[CommitHash], after_permissions: &WritePermissions, delivery_heads: &[CommitHash]) -> (out: Result<Self, Error>)
        ensures match out {
            Err(Error::UnknownCommit(h)) => !history.known(h) && (old_delivery@.contains(h) || before_heads@.contains(h)
                || after_heads@.contains(h) || delivery_heads@.contains(h)),
            Err(Error::InvalidItem(_)) => known(&history.topology, old_delivery@) && known(&history.topology, before_heads@)
                && known(&history.topology, after_heads@) && known(&history.topology, delivery_heads@)
                && !old_delivery@.to_set().union(before_heads@.to_set()).union(after_heads@.to_set()).subset_of(history.causal_from(delivery_heads@)),
            Err(_) => false,
            Ok(plan) => plan.history_id() == history.history_id()
                && plan.delivered_members() == history.causal_from(old_delivery@)
                && plan.before_members() == history.authorized_from(before_heads@, before_permissions)
                && plan.after_members() == history.authorized_from(after_heads@, after_permissions)
                && known(&history.topology, old_delivery@) && known(&history.topology, before_heads@)
                && known(&history.topology, after_heads@) && known(&history.topology, delivery_heads@)
                && old_delivery@.to_set().union(before_heads@.to_set()).union(after_heads@.to_set()).subset_of(history.causal_from(delivery_heads@))
                && plan.selected() == records(history.causal_from(delivery_heads@), history.causal_from(old_delivery@),
                    history.authorized_from(before_heads@, before_permissions), history.authorized_from(after_heads@, after_permissions))
                && plan.selected().subset_of(history.causal_from(delivery_heads@))
                && frozen(&plan.delivery_clock(), history.read_state(), history.causal_from(old_delivery@))
                && frozen(&plan.before_clock(), history.read_state(), history.authorized_from(before_heads@, before_permissions))
                && frozen(&plan.after_clock(), history.read_state(), history.authorized_from(after_heads@, after_permissions)),
        },
    {
        proof { history.topology.established(); history.representation(); }
        let t = &history.topology;
        let delivered_before = match delivery(t, old_delivery) { Ok(c) => c, Err(e) => return Err(e) };
        let before = match participation(t, before_heads, before_permissions) { Ok(c) => c, Err(e) => return Err(e) };
        let after = match participation(t, after_heads, after_permissions) { Ok(c) => c, Err(e) => return Err(e) };
        let delivered_now = match delivery(t, delivery_heads) { Ok(c) => c, Err(e) => return Err(e) };
        let ghost delivered = captured::causal(t, delivery_heads@);
        if !covers(t, &delivered_now, old_delivery, #[cfg(verus_keep_ghost)] { Ghost(delivered) })
            || !covers(t, &delivered_now, before_heads, #[cfg(verus_keep_ghost)] { Ghost(delivered) })
            || !covers(t, &delivered_now, after_heads, #[cfg(verus_keep_ghost)] { Ghost(delivered) }) {
            return Err(Error::InvalidItem("captured delivery history excludes a view or cursor head"));
        }
        let differences = [(&*delivered_now, &*delivered_before), (&before, &after), (&after, &before)];
        proof { t.traversal_facts(); }
        let traversal = OwnedTraversal::new(history, &differences);
        proof {
            t.permission_facts();
            assert forall|h: CommitHash| traversal.remaining_hashes().contains(h) == records(delivered, captured::causal(t, old_delivery@),
                captured::authorized(t, before_heads@, before_permissions), captured::authorized(t, after_heads@, after_permissions)).contains(h) by {
                if t.hashes().contains_key(h) {
                    let n = t.hashes()[h];
                    traversal_init::requested_difference(t, differences@, n);
                    commits::delta_at(t, &delivered_now, &delivered_before, n);
                    commits::delta_at(t, &before, &after, n);
                    commits::delta_at(t, &after, &before, n);
                    if traversal_init::requested(t, differences@, n) {
                        let i = choose|i: int| 0 <= i < differences.len() && super::difference::delta_member(
                            super::difference::tables_view(t.tables_view()), differences[i].0, differences[i].1, n);
                        assert(i == 0 || i == 1 || i == 2);
                    }
                }
            }
            assert(traversal.remaining_hashes() =~= records(delivered, captured::causal(t, old_delivery@),
                captured::authorized(t, before_heads@, before_permissions), captured::authorized(t, after_heads@, after_permissions)));
            covered_history(t, before_heads@, delivery_heads@);
            covered_history(t, after_heads@, delivery_heads@);
            assert(captured::authorized(t, before_heads@, before_permissions).subset_of(captured::causal(t, before_heads@)));
            assert(captured::authorized(t, after_heads@, after_permissions).subset_of(captured::causal(t, after_heads@)));
        }
        Ok(Self { traversal: Some(traversal), identity: t.identity().clone(), delivered_before, before, after,
            #[cfg(verus_keep_ghost)] meaning: Ghost(EmissionMeaning { state: t.read_state(), delivered: captured::causal(t, old_delivery@),
                before: captured::authorized(t, before_heads@, before_permissions), after: captured::authorized(t, after_heads@, after_permissions) }),
            #[cfg(verus_keep_ghost)] receipt: Tracked(t.snapshot()) })
    }
}

/// A captured change traversal. Later deliveries belong to a later traversal.
///
/// Records are ancestors-first, with the smallest hash first among records
/// whose selected ancestors have all been emitted. This order depends only on
/// the selected commits and their ancestry, not on local delivery order or
/// batch size. Unchanged commits between two selected commits still impose
/// ordering. The traversal retains intervals and a frontier per touched chain,
/// rather than a list of every selected commit.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct Changes {
    pub(crate) plan: ChangePlan,
    pub(crate) delivery_heads: Vec<CommitHash>,
    pub(crate) target: ViewPoint,
}

impl Changes {
    pub(crate) proof fn metadata(&self)
        ensures self.delivery_view() == self.delivery_heads@, self.target_view() == self.target, self.history_id() == self.plan.history_id(),
    {}
    pub closed spec fn selected(&self) -> Set<CommitHash> { self.plan.selected() }
    pub closed spec fn history_id(&self) -> u64 { self.plan.history_id() }
    pub closed spec fn delivered_members(&self) -> Set<CommitHash> { self.plan.delivered_members() }
    pub closed spec fn before_members(&self) -> Set<CommitHash> { self.plan.before_members() }
    pub closed spec fn after_members(&self) -> Set<CommitHash> { self.plan.after_members() }
    pub closed spec fn delivery_view(&self) -> Seq<CommitHash> { self.delivery_heads@ }
    pub closed spec fn target_view(&self) -> ViewPoint { self.target }

    /// Records remaining without expanding the retained intervals.
    pub fn remaining(&self) -> (out: usize) ensures out == self.selected().len(),
    { self.plan.remaining() }
}

/// Re-plan the actual captured traversal after a restart, without absorbing later deliveries.
pub(crate) fn plan_changes_at(history: &History, cursor: &MaterializationCursor, target: &ViewPoint,
    delivery_heads: &[CommitHash]) -> (out: Result<Changes, Error>)
        ensures match out {
            Err(Error::UnknownCommit(h)) => !history.known(h) && (cursor.delivery_view().contains(h) || cursor.target_view().heads_view().contains(h)
                || target.heads_view().contains(h) || delivery_heads@.contains(h)),
            Err(Error::InvalidItem(_)) => known(&history.topology, cursor.delivery_view()) && known(&history.topology, cursor.target_view().heads_view())
                && known(&history.topology, target.heads_view()) && known(&history.topology, delivery_heads@)
                && !cursor.delivery_view().to_set().union(cursor.target_view().heads_view().to_set()).union(target.heads_view().to_set()).subset_of(history.causal_from(delivery_heads@)),
            Err(_) => false,
            Ok(changes) => changes.delivered_members() == history.causal_from(cursor.delivery_view())
                && changes.before_members() == history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view())
                && changes.after_members() == history.authorized_from(target.heads_view(), &target.permissions_view())
                && changes.delivery_view() == delivery_heads@ && changes.target_view().same(target) && changes.history_id() == history.history_id()
                && known(&history.topology, cursor.delivery_view()) && known(&history.topology, cursor.target_view().heads_view())
                && known(&history.topology, target.heads_view()) && known(&history.topology, delivery_heads@)
                && cursor.delivery_view().to_set().union(cursor.target_view().heads_view().to_set()).union(target.heads_view().to_set()).subset_of(history.causal_from(delivery_heads@))
                && changes.selected() == records(history.causal_from(delivery_heads@), history.causal_from(cursor.delivery_view()),
                    history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()), history.authorized_from(target.heads_view(), &target.permissions_view()))
                && changes.selected().subset_of(history.causal_from(delivery_heads@))
                && frozen(&changes.plan.delivery_clock(), history.read_state(), history.causal_from(cursor.delivery_view()))
                && frozen(&changes.plan.before_clock(), history.read_state(), history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()))
                && frozen(&changes.plan.after_clock(), history.read_state(), history.authorized_from(target.heads_view(), &target.permissions_view())),
        },
{
    proof { cursor.representation(); cursor.view_point.representation(); target.representation(); }
    let plan = match ChangePlan::new(history, &cursor.delivery_heads, &cursor.view_point.heads,
        &cursor.view_point.write_permissions, &target.heads, &target.write_permissions, delivery_heads) {
        Ok(plan) => plan, Err(e) => return Err(e),
    };
    let mut saved_delivery = Vec::new();
    saved_delivery.extend_from_slice(delivery_heads);
    assert(saved_delivery@ =~= delivery_heads@);
    Ok(Changes { plan, delivery_heads: saved_delivery, target: target.clone() })
}

/// Append exactly one bounded batch, preserving any pre-existing output prefix.
pub(crate) fn emit_next_changes(history: &History, changes: &mut Changes, out: &mut Vec<Change>, max: usize) -> (result: Result<Batch, Error>)
    ensures final(changes).history_id() == old(changes).history_id(),
        final(changes).delivery_view() == old(changes).delivery_view(), final(changes).target_view() == old(changes).target_view(),
        final(changes).delivered_members() == old(changes).delivered_members(),
        final(changes).before_members() == old(changes).before_members(), final(changes).after_members() == old(changes).after_members(),
        (max == 0 || old(changes).selected().is_empty()) ==> *final(changes) == *old(changes),
        match result {
            Err(e) => e is WrongHistory && old(changes).history_id() != history.history_id()
                && *final(changes) == *old(changes) && final(out)@ == old(out)@,
            Ok(batch) => old(changes).history_id() == history.history_id() && {
                let added = final(out)@.skip(old(out)@.len() as int);
                &&& final(out)@.take(old(out)@.len() as int) == old(out)@
                &&& final(out)@.len() == old(out)@.len() + added.len()
                &&& added.len() == if max <= old(changes).selected().len() { max as nat } else { old(changes).selected().len() }
                &&& final(changes).selected() == old(changes).selected().difference(hashes(added).to_set())
                &&& scheduled(history, old(changes).selected(), hashes(added))
                &&& hashes(added).no_duplicates()
                &&& final(changes).selected().len() + added.len() == old(changes).selected().len()
                &&& forall|i: int, j: int| 0 <= i < j < added.len() ==> !history.strict_ancestor(added[j].hash, added[i].hash)
                &&& forall|i: int| 0 <= i < added.len() ==> mapped(history, added[i], old(changes).delivered_members(), old(changes).before_members(), old(changes).after_members())
                &&& match batch {
                    Batch::More => !final(changes).selected().is_empty(),
                    Batch::Complete(cursor) => final(changes).selected().is_empty()
                        && cursor.delivery_view() == old(changes).delivery_view() && cursor.target_view().same(&old(changes).target_view()),
                }
            },
        },
{
    match changes.plan.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
    let ghost initial = *changes;
    let ghost prefix = out@;
    let ghost mut added: Seq<Change> = Seq::empty();
    let count = changes.remaining();
    let limit = events::batch_end(0, count, max);
    let mut i = 0;
    while i < limit
        invariant i <= limit <= count, added.len() == i,
            count == initial.selected().len(), limit == if max <= count { max } else { count },
            changes.history_id() == initial.history_id() == history.history_id(),
            i == 0 ==> *changes == initial,
            changes.delivery_view() == initial.delivery_view(), changes.target_view() == initial.target_view(),
            changes.delivered_members() == initial.delivered_members(), changes.before_members() == initial.before_members(), changes.after_members() == initial.after_members(),
            out@ == prefix + added,
            changes.selected() == initial.selected().difference(hashes(added).to_set()),
            changes.selected().len() + added.len() == count,
            scheduled(history, initial.selected(), hashes(added)),
            forall|j: int| 0 <= j < added.len() ==> mapped(history, added[j], initial.delivered_members(), initial.before_members(), initial.after_members()),
        decreases limit - i,
    {
        let ghost previous = added;
        let ghost remaining = changes.selected();
        let change = changes.plan.next(history).unwrap().expect("an acyclic frontier has a ready record");
        out.push(change);
        proof {
            schedule_push(history, initial.selected(), hashes(previous), change.hash);
            added = previous.push(change);
            assert(hashes(added) =~= hashes(previous).push(change.hash));
            hashes(previous).lemma_push_to_set_commute(change.hash);
            assert(changes.selected() =~= initial.selected().difference(hashes(added).to_set()));
            assert(out@ =~= prefix + added);
        }
        i += 1;
    }
    proof {
        assert(out@.take(prefix.len() as int) =~= prefix);
        assert(out@.skip(prefix.len() as int) =~= added);
        schedule_facts(history, initial.selected(), hashes(added));
        assert forall|j: int, k: int| 0 <= j < k < added.len() implies !history.strict_ancestor(added[k].hash, added[j].hash) by {
            assert(hashes(added)[j] == added[j].hash);
            assert(hashes(added)[k] == added[k].hash);
        }
    }
    if changes.remaining() > 0 { return Ok(Batch::More); }
    Ok(Batch::Complete(MaterializationCursor::from_parts(changes.delivery_heads.clone(), changes.target.clone())))
}

} // verus!
