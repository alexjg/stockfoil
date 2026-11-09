//! Persistent captured clocks and exact hash membership after compatible appends.
use super::{
    commit_hash::CommitHash, topology::Topology, tree::Clock, write_permissions::WritePermissions,
};
#[cfg(verus_keep_ghost)]
use crate::{
    capture_refinement::{self, ReadState, frozen},
    graph, permission_refinement,
    storage_refinement::parent_set,
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg(verus_keep_ghost)]
pub(crate) struct Meaning {
    pub state: ReadState,
    pub causal: Set<CommitHash>,
    pub authorized: Set<CommitHash>,
}

/// Runtime storage is the original history ID and two shared clocks. Receipts are erased.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Captured {
    identity: super::identity::Identity,
    reached: Arc<Clock>,
    participating: Arc<Clock>,
    #[cfg(verus_keep_ghost)] meaning: Ghost<Meaning>,
    #[cfg(verus_keep_ghost)] receipt: Tracked<crate::lineage::Receipt>,
}

pub(crate) open spec fn causal<P>(topology: &Topology<P>, heads: Seq<CommitHash>) -> Set<CommitHash> {
    topology.hashes().dom().filter(|h: CommitHash| graph::history(topology.graph(),
        parent_set(heads.map(|i: int, head: CommitHash| topology.hashes()[head]))).contains(topology.hashes()[h]@ as int))
}

pub(crate) open spec fn authorized<P>(topology: &Topology<P>, heads: Seq<CommitHash>, permissions: &WritePermissions) -> Set<CommitHash> {
    topology.hashes().dom().filter(|h: CommitHash| permission_refinement::visible_in(
        topology.graph(), topology.labels(), topology.owners_view(), topology.hashes(),
        graph::history(topology.graph(), parent_set(heads.map(|i: int, head: CommitHash| topology.hashes()[head]))),
        permissions, topology.hashes()[h]@ as int))
}

impl Clone for Captured {
    fn clone(&self) -> (out: Self)
        ensures out.valid(), out.history_id() == self.history_id(), out.origin() == self.origin(),
            out.lineage_id() == self.lineage_id(), out.causal() == self.causal(),
            out.authorized() == self.authorized(), out.reached_view() == self.reached_view(),
    {
        proof { use_type_invariant(self); }
        Self { identity: self.identity.clone(), reached: Arc::clone(&self.reached), participating: Arc::clone(&self.participating),
            #[cfg(verus_keep_ghost)] meaning: Ghost(self.meaning@),
            #[cfg(verus_keep_ghost)] receipt: Tracked(self.receipt.borrow().duplicate()) }
    }
}

impl Captured {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { self.valid() }

    pub closed spec fn history_id(&self) -> u64 { self.identity.raw() }
    pub closed spec fn lineage_id(&self) -> vstd::resource::Loc { self.receipt@.id() }
    pub closed spec fn reached_view(&self) -> Clock { *self.reached }
    pub closed spec fn origin(&self) -> ReadState { self.meaning@.state }
    pub closed spec fn causal(&self) -> Set<CommitHash> { self.meaning@.causal }
    pub closed spec fn authorized(&self) -> Set<CommitHash> { self.meaning@.authorized }
    pub closed spec fn valid(&self) -> bool {
        self.identity.lineage() == self.lineage_id()
            && self.receipt@.state() == self.origin()
            && frozen(&self.reached, self.origin(), self.causal())
            && (forall|c: u32| self.reached_view().contents().contains_key(c) ==>
                self.origin().labels[self.reached_view().contents()[c]@ as int] == c)
            && frozen(&self.participating, self.origin(), self.authorized())
    }
    pub(crate) open spec fn compatible(&self, state: ReadState) -> bool { state.extends(self.origin()) }

    pub(crate) proof fn prove_compatible<P>(tracked &self, tracked topology: &Topology<P>)
        requires topology.valid(), self.lineage_id() == topology.lineage_id(),
        ensures self.compatible(topology.read_state()),
    { use_type_invariant(self); topology.compatible(self.receipt.borrow()); }

    pub(crate) fn new<P>(topology: &Topology<P>, heads: &[CommitHash], permissions: &WritePermissions) -> (out: Result<Self, CommitHash>)
        ensures match out {
            Err(h) => heads@.contains(h) && !topology.hashes().contains_key(h),
            Ok(saved) => saved.valid()
                && (forall|i: int| 0 <= i < heads.len() ==> topology.hashes().contains_key(heads[i]))
                && saved.history_id() == topology.history_id()
                && saved.lineage_id() == topology.lineage_id()
                && saved.origin() == topology.read_state()
                && saved.compatible(topology.read_state())
                && saved.causal() == causal(topology, heads@)
                && saved.authorized() == authorized(topology, heads@, permissions),
        },
    {
        proof { topology.established(); }
        let (reached, participating) = match super::permissions::capture_clocks(topology, heads, permissions) {
            Ok(clocks) => clocks,
            Err(h) => return Err(h),
        };
        let participating = Arc::new(participating);
        proof {
            topology.permission_facts();
            topology.read_state().reflexive();
            assert forall|h: CommitHash| topology.hashes().contains_key(h) implies
                causal(topology, heads@).contains(h) == super::clock::below(reached.lookup(topology.labels()[topology.hashes()[h]@ as int]), topology.hashes()[h]@ as usize) by {};
            assert forall|h: CommitHash| topology.hashes().contains_key(h) implies
                authorized(topology, heads@, permissions).contains(h) == super::clock::below(participating.lookup(topology.labels()[topology.hashes()[h]@ as int]), topology.hashes()[h]@ as usize) by {};
        }
        Ok(Self { identity: topology.identity().clone(), reached, participating,
            #[cfg(verus_keep_ghost)] meaning: Ghost(Meaning { state: topology.read_state(),
                causal: causal(topology, heads@), authorized: authorized(topology, heads@, permissions) }),
            #[cfg(verus_keep_ghost)] receipt: Tracked(topology.snapshot()) })
    }

    /// Frozen causal coordinates remain meaningful in the originating history.
    pub(crate) proof fn reached_facts<P>(tracked &self, tracked t: &Topology<P>)
        requires t.valid(), self.lineage_id() == t.lineage_id(),
        ensures self.reached_view().ordered(),
            forall|c: u32| self.reached_view().contents().contains_key(c) ==> c < t.tables_view().len(),
            self.causal().subset_of(t.hashes().dom()),
            forall|h: CommitHash| self.causal().contains(h) == (t.hashes().contains_key(h)
                && super::clock::below(self.reached_view().lookup(t.nodes_view()[t.hashes()[h]@ as int].chain), t.hashes()[h]@ as usize)),
    {
        use_type_invariant(self);
        self.prove_compatible(t);
        t.permission_facts();
        reveal(ReadState::extends);
        assert forall|c: u32| self.reached_view().contents().contains_key(c) implies c < t.tables_view().len() by {
            let tip = self.reached_view().contents()[c];
            assert(t.read_state().labels[tip@ as int] == self.origin().labels[tip@ as int]);
            t.traversal_node(tip);
            assert(t.nodes_view()[tip@ as int].chain == c);
        }
        assert forall|h: CommitHash| self.causal().contains(h) == (t.hashes().contains_key(h)
            && super::clock::below(self.reached_view().lookup(t.nodes_view()[t.hashes()[h]@ as int].chain), t.hashes()[h]@ as usize)) by {
            capture_refinement::frozen_membership(&self.reached_view(), self.origin(), t.read_state(), self.causal(), h);
        }
        assert(self.causal().subset_of(t.hashes().dom()));
    }

    pub(crate) fn reached(&self) -> (out: &Clock)
        ensures *out == self.reached_view(), self.valid() ==> out.valid(),
    { &self.reached }

    pub(crate) fn check_history<P>(&self, topology: &Topology<P>) -> (out: bool)
        ensures out == (self.history_id() == topology.history_id()),
            out ==> self.lineage_id() == topology.lineage_id(),
    {
        proof { use_type_invariant(self); topology.established(); }
        self.identity.same(topology.identity())
    }

    pub(crate) fn contains<P>(&self, topology: &Topology<P>, hash: &CommitHash) -> (out: Result<bool, ()>)
        ensures match out {
            Err(()) => self.history_id() != topology.history_id(),
            Ok(found) => self.history_id() == topology.history_id() && found == self.causal().contains(*hash),
        },
    {
        proof { topology.established(); }
        if !self.check_history(topology) { return Err(()); }
        proof { use_type_invariant(self); self.prove_compatible(topology); }
        Ok(Self::lookup(&self.reached, topology, hash,
            #[cfg(verus_keep_ghost)] { Ghost((self.origin(), self.causal())) }))
    }

    pub(crate) fn participates<P>(&self, topology: &Topology<P>, hash: &CommitHash) -> (out: Result<bool, ()>)
        ensures match out {
            Err(()) => self.history_id() != topology.history_id(),
            Ok(found) => self.history_id() == topology.history_id() && found == self.authorized().contains(*hash),
        },
    {
        proof { topology.established(); }
        if !self.check_history(topology) { return Err(()); }
        proof { use_type_invariant(self); self.prove_compatible(topology); }
        Ok(Self::lookup(&self.participating, topology, hash,
            #[cfg(verus_keep_ghost)] { Ghost((self.origin(), self.authorized())) }))
    }

    fn lookup<P>(clock: &Clock, topology: &Topology<P>, hash: &CommitHash,
        #[cfg(verus_keep_ghost)] Ghost(saved): Ghost<(ReadState, Set<CommitHash>)>) -> (out: bool)
        requires topology.valid(), frozen(clock, saved.0, saved.1), topology.read_state().extends(saved.0),
        ensures out == saved.1.contains(*hash),
    {
        proof {
            topology.permission_facts();
            capture_refinement::frozen_membership(clock, saved.0, topology.read_state(), saved.1, *hash);
        }
        match topology.by_hash().get(hash) {
            None => false,
            Some(idx) => clock.contains(topology.nodes()[idx.index()].chain, *idx),
        }
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Author;

    #[test]
    fn captured_clocks_share_storage_and_ignore_later_growth() {
        use super::super::write_permission::WritePermission;
        assert_eq!(
            std::mem::size_of::<Captured>(),
            2 * std::mem::size_of::<Arc<Clock>>() + std::mem::size_of::<u64>()
        );
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let hashes = [0, 1, 2, 3].map(|n| CommitHash([n; 32]));
        let mut t = Topology::new();
        let empty = Captured::new(&t, &[], &WritePermissions::new()).unwrap();
        let root = t.insert(hashes[0], alice, &[], (), 0).unwrap();
        let mut permissions = WritePermissions::new().with_unlisted(WritePermission::Unbounded);
        let saved = Captured::new(&t, &[hashes[0]], &permissions).unwrap();
        let cloned = saved.clone();
        assert!(Arc::ptr_eq(&saved.reached, &cloned.reached));
        assert!(Arc::ptr_eq(&saved.participating, &cloned.participating));
        drop(saved);
        permissions.set_unlisted(WritePermission::Denied);
        let bounds =
            WritePermissions::new().with_unlisted(WritePermission::Bounded(vec![hashes[1]]));
        let withheld = Captured::new(&t, &[hashes[0]], &bounds).unwrap();
        assert!(!withheld.participates(&t, &hashes[0]).unwrap());
        t.insert(hashes[1], bob, &[hashes[0]], (), 0).unwrap();
        t.insert(hashes[2], alice, &[hashes[0]], (), 0).unwrap();
        t.insert(hashes[3], alice, &[hashes[1], hashes[2]], (), 0)
            .unwrap();
        t.mark_admitted(root);
        t.mark_in_fragment(root);
        assert!(cloned.contains(&t, &hashes[0]).unwrap());
        assert!(cloned.participates(&t, &hashes[0]).unwrap());
        assert!(!withheld.participates(&t, &hashes[0]).unwrap());
        for h in hashes {
            assert!(!empty.contains(&t, &h).unwrap());
            assert!(!empty.participates(&t, &h).unwrap());
        }
        for h in &hashes[1..] {
            assert!(!cloned.contains(&t, h).unwrap());
            assert!(!cloned.participates(&t, h).unwrap());
        }
        let fresh = Captured::new(&t, &[hashes[3]], &bounds).unwrap();
        assert!(fresh.participates(&t, &hashes[0]).unwrap());
        assert!(fresh.participates(&t, &hashes[1]).unwrap());
        let unknown = CommitHash([255; 32]);
        assert!(!cloned.contains(&t, &unknown).unwrap());
        assert!(!cloned.participates(&t, &unknown).unwrap());
        assert_eq!(Captured::new(&t, &[unknown], &bounds).unwrap_err(), unknown);
    }
}
