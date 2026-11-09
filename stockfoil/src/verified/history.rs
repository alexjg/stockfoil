//! The actual public History representation and checked ancestry entry points.
use super::{
    api_error::Error,
    author::Author,
    clock::NodeIdx,
    commit_hash::CommitHash,
    history_data::{Certificate, Key, PayloadId, Signature},
    write_permissions::WritePermissions,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// An authenticated commit history. See the [crate documentation](crate).
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct History {
    pub(crate) topology: super::topology::Topology<PayloadId>,
    pub(crate) admitted_heads: BTreeSet<CommitHash>,
    /// Delivered local commits that are not yet admitted.
    pub(crate) drafts: BTreeSet<NodeIdx>,
    pub(crate) signed_drafts: BTreeSet<NodeIdx>,
    pub(crate) signatures: HashMap<NodeIdx, BTreeMap<Author, Signature>>,
    pub(crate) inventory: BTreeMap<Key, Arc<Certificate>>,
    pub(crate) epoch: u64,
}

impl Default for History {
    fn default() -> (out: Self)
        ensures out.empty(),
    {
        Self::new()
    }
}

impl History {
    pub closed spec fn history_id(&self) -> u64 { self.topology.history_id() }
    pub(crate) fn id(&self) -> (out: u64) ensures out == self.history_id(),
    { self.topology.identity().value() }

    pub closed spec fn read_state(&self) -> crate::capture_refinement::ReadState { self.topology.read_state() }
    pub closed spec fn graph_view(&self) -> crate::graph::Graph { self.topology.graph() }
    pub closed spec fn owners_view(&self) -> Seq<Author> { self.topology.owners_view() }
    pub open spec fn known(&self, hash: CommitHash) -> bool { self.read_state().hashes.contains_key(hash) }
    pub open spec fn causal_from(&self, heads: Seq<CommitHash>) -> Set<CommitHash> {
        let hashes = self.read_state().hashes;
        hashes.dom().filter(|h: CommitHash| crate::graph::history(self.graph_view(),
            crate::storage_refinement::parent_set(heads.map(|i: int, head: CommitHash| hashes[head]))).contains(hashes[h]@ as int))
    }
    pub open spec fn authorized_from(&self, heads: Seq<CommitHash>, permissions: &WritePermissions) -> Set<CommitHash> {
        let state = self.read_state();
        let selection = crate::graph::history(self.graph_view(),
            crate::storage_refinement::parent_set(heads.map(|i: int, head: CommitHash| state.hashes[head])));
        state.hashes.dom().filter(|h: CommitHash| crate::permission_refinement::visible_in(
            self.graph_view(), state.labels, self.owners_view(), state.hashes, selection, permissions, state.hashes[h]@ as int))
    }
    pub open spec fn strict_ancestor(&self, ancestor: CommitHash, descendant: CommitHash) -> bool {
        let hashes = self.read_state().hashes;
        self.known(ancestor) && self.known(descendant) && hashes[ancestor]@ < hashes[descendant]@
            && crate::graph::reaches(self.graph_view(), hashes[ancestor]@ as int, hashes[descendant]@ as int)
    }

    /// Relate the public logical view to private storage; this is a checked lemma.
    pub(crate) proof fn representation(&self)
        ensures self.read_state() == self.topology.read_state(), self.graph_view() == self.topology.graph(),
            self.owners_view() == self.topology.owners_view(), self.history_id() == self.topology.history_id(),
    {}

    pub closed spec fn empty(&self) -> bool {
        &&& self.topology.nodes_view().len() == 0
        &&& self.topology.ids_view().len() == 0 && self.topology.tips_view().len() == 0
        &&& self.topology.head_hashes().is_empty()
        &&& self.topology.hashes() == Map::<CommitHash, NodeIdx>::empty()
        &&& self.admitted_heads@.is_empty() && self.drafts@.is_empty() && self.signed_drafts@.is_empty()
        &&& self.signatures@.is_empty() && self.inventory@.is_empty() && self.epoch == 0
    }

    pub(crate) proof fn empty_facts(&self)
        requires self.empty(),
        ensures self.topology.nodes_view().len() == 0, self.topology.ids_view().len() == 0, self.topology.tips_view().len() == 0,
            self.topology.hashes() == Map::<CommitHash, NodeIdx>::empty(), self.topology.head_hashes().is_empty(), self.epoch == 0,
    {}

    pub fn new() -> (out: Self)
        ensures out.empty(),
    {
        Self {
            topology: super::topology::Topology::new(),
            admitted_heads: BTreeSet::new(),
            drafts: BTreeSet::new(),
            signed_drafts: BTreeSet::new(),
            signatures: HashMap::new(),
            inventory: BTreeMap::new(),
            epoch: 0,
        }
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    pub fn is_ancestor(&self, ancestor: &CommitHash, descendant: &CommitHash) -> (out: bool)
        ensures out == self.strict_ancestor(*ancestor, *descendant),
    {
        proof { self.topology.established(); }
        self.topology.is_ancestor(ancestor, descendant)
    }

    /// Capture the causal past of `heads`, inclusive, and its participation
    /// under `permissions`. Every head must already be delivered.
    ///
    /// The owned handle shares clock structure, not a copy of the graph.
    /// Later deliveries or changes to the supplied permissions do not alter
    /// it. Use its membership queries to filter a CRDT's own indexes for
    /// historical reads, without materializing a second copy of its state.
    pub fn ancestry(
        &self,
        heads: &[CommitHash],
        permissions: &WritePermissions,
    ) -> (out: Result<Ancestry, Error>)
        ensures match out {
            Err(error) => match error {
                Error::UnknownCommit(hash) => heads@.contains(hash) && !self.known(hash),
                _ => false,
            },
            Ok(saved) => saved.history_id() == self.history_id()
                && saved.origin() == self.read_state()
                && (forall|i: int| 0 <= i < heads.len() ==> self.known(heads[i]))
                && saved.heads_view().to_set() == heads@.to_set()
                && saved.causal() == self.causal_from(heads@)
                && saved.authorized() == self.authorized_from(heads@, permissions),
        },
    {
        Ancestry::new(self, heads, permissions)
    }

}


/// The causal past of some heads, inclusive, with participation under the
/// permissions captured when the handle was created.
///
/// Construct with [`History::ancestry`]. Cloning shares the clocks and heads;
/// the handle owns no payloads or copy of the graph. It does not borrow the
/// history, so the graph may keep growing while the handle stays unchanged.
/// Membership needs the originating [`History`] to resolve a hash, followed
/// by a chain-clock lookup. A different history, even if it contains the same
/// commits, is rejected with [`Error::WrongHistory`]. Recreate the handle from
/// its heads and permissions on a rebuilt history.
///
/// [`contains`](Self::contains) is causal membership;
/// [`participates`](Self::participates) additionally applies permissions.
/// Hidden commits remain in the causal past: visible descendants may refer to
/// their contents. Unknown hashes return `false` for both queries.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct Ancestry {
    heads: Arc<[CommitHash]>,
    clocks: super::captured::Captured,
}

impl Clone for Ancestry {
    fn clone(&self) -> (out: Self)
        ensures out.heads_view() == self.heads_view(), out.history_id() == self.history_id(),
            out.origin() == self.origin(), out.causal() == self.causal(), out.authorized() == self.authorized(),
    {
        proof { use_type_invariant(self); }
        Self { heads: Arc::clone(&self.heads), clocks: self.clocks.clone() }
    }
}

impl Ancestry {
    pub closed spec fn normalized(hashes: Seq<CommitHash>) -> bool { super::stdlib::hashes_sorted(hashes) && hashes.no_duplicates() }
    pub closed spec fn heads_view(&self) -> Seq<CommitHash> { (*self.heads)@ }
    pub closed spec fn history_id(&self) -> u64 { self.clocks.history_id() }
    pub(crate) closed spec fn lineage_id(&self) -> vstd::resource::Loc { self.clocks.lineage_id() }
    pub closed spec fn origin(&self) -> crate::capture_refinement::ReadState { self.clocks.origin() }
    pub closed spec fn causal(&self) -> Set<CommitHash> { self.clocks.causal() }
    pub closed spec fn authorized(&self) -> Set<CommitHash> { self.clocks.authorized() }

    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        &&& super::stdlib::hashes_sorted(self.heads_view())
        &&& self.heads_view().no_duplicates()
        &&& forall|h: CommitHash| self.heads_view().to_set().contains(h) ==> self.origin().hashes.contains_key(h)
    }

    pub(crate) fn new(history: &History, heads: &[CommitHash], permissions: &WritePermissions) -> (out: Result<Self, Error>)
        ensures match out {
            Err(error) => match error {
                Error::UnknownCommit(hash) => heads@.contains(hash) && !history.known(hash),
                _ => false,
            },
            Ok(saved) => saved.history_id() == history.history_id()
                && saved.origin() == history.read_state()
                && (forall|i: int| 0 <= i < heads.len() ==> history.known(heads[i]))
                && saved.heads_view().to_set() == heads@.to_set()
                && saved.causal() == history.causal_from(heads@)
                && saved.authorized() == history.authorized_from(heads@, permissions),
        },
    {
        let clocks = match super::captured::Captured::new(&history.topology, heads, permissions) {
            Ok(clocks) => clocks,
            Err(hash) => return Err(Error::UnknownCommit(hash)),
        };
        let mut normalized = Vec::with_capacity(heads.len());
        let mut i = 0;
        while i < heads.len()
            invariant i <= heads.len(), normalized@ == heads@.subrange(0, i as int),
            decreases heads.len() - i,
        {
            normalized.push(heads[i]);
            i += 1;
            assert(normalized@ =~= heads@.subrange(0, i as int));
        }
        assert(normalized@ == heads@);
        super::stdlib::normalize_hashes(&mut normalized);
        let shared = super::stdlib::share_hashes(normalized);
        proof {
            assert forall|h: CommitHash| (*shared)@.to_set().contains(h) implies clocks.origin().hashes.contains_key(h) by {
                assert(heads@.contains(h));
                let j = choose|j: int| 0 <= j < heads.len() && heads@[j] == h;
            }
        }
        Ok(Self { heads: shared, clocks })
    }

    /// The supplied heads, sorted and deduplicated. Redundant ancestors are
    /// kept; all heads were checked as delivered when the handle was created.
    pub fn heads(&self) -> (out: &[CommitHash])
        ensures out@ == self.heads_view(), Self::normalized(out@), out@.no_duplicates(),
            forall|h: CommitHash| out@.contains(h) ==> self.origin().hashes.contains_key(h),
    {
        proof {
            use_type_invariant(self);
            assert forall|h: CommitHash| self.heads_view().contains(h) implies self.origin().hashes.contains_key(h) by {
                assert(self.heads_view().to_set().contains(h));
            }
        }
        &self.heads
    }

    /// Whether `hash` is one of the heads or an ancestor of one, regardless of
    /// permissions. `history` must be the one this handle was created from.
    pub fn contains(&self, history: &History, hash: &CommitHash) -> (out: Result<bool, Error>)
        ensures match out {
            Err(error) => error is WrongHistory && self.history_id() != history.history_id(),
            Ok(found) => self.history_id() == history.history_id() && found == self.causal().contains(*hash),
        },
    {
        match self.clocks.contains(&history.topology, hash) {
            Ok(found) => Ok(found),
            Err(()) => Err(Error::WrongHistory),
        }
    }

    /// Causal membership **and** permission to participate in the captured
    /// view. Later permission changes do not alter this handle. Unlisted authors
    /// use the captured fallback rule; explicit entries override it. A denied
    /// rule grants nothing; a bounded frontier grants only its
    /// ancestors, and only if the frontier itself is in the captured history.
    pub fn participates(&self, history: &History, hash: &CommitHash) -> (out: Result<bool, Error>)
        ensures match out {
            Err(error) => error is WrongHistory && self.history_id() != history.history_id(),
            Ok(found) => self.history_id() == history.history_id() && found == self.authorized().contains(*hash),
        },
    {
        match self.clocks.participates(&history.topology, hash) {
            Ok(found) => Ok(found),
            Err(()) => Err(Error::WrongHistory),
        }
    }

    /// Commits in `self`'s causal past but not `other`'s. Permissions on either
    /// handle are ignored; this is causal set difference, not a visibility diff.
    /// Both handles must originate from `history`.
    ///
    /// The iterator is lazy and exact-sized, with ancestor-first,
    /// smallest-ready-hash order. Its working state scales with touched chains,
    /// not returned commits. It borrows the history, not the handles.
    pub fn difference<'h>(&self, other: &Self, history: &'h History) -> (out: Result<super::commits::Commits<'h>, Error>)
        ensures match out {
            Err(error) => error is WrongHistory && (self.history_id() != history.history_id() || other.history_id() != history.history_id()),
            Ok(iter) => self.history_id() == history.history_id() && other.history_id() == history.history_id()
                && iter.history_view() == history && iter.remaining_hashes() == self.causal().difference(other.causal()),
        },
    {
        match self.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
        match other.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
        proof {
            history.topology.established();
            self.clocks.reached_facts(&history.topology);
            other.clocks.reached_facts(&history.topology);
        }
        let after = self.clocks.reached();
        let before = other.clocks.reached();
        let iter = super::commits::Commits::new(history, after, before);
        proof {
            history.topology.hash_facts();
            assert forall|h: CommitHash| iter.remaining_hashes().contains(h) == self.causal().difference(other.causal()).contains(h) by {
                if history.topology.hashes().contains_key(h) {
                    super::commits::delta_at(&history.topology, after, before, history.topology.hashes()[h]);
                }
            }
            assert(iter.remaining_hashes() =~= self.causal().difference(other.causal()));
        }
        Ok(iter)
    }

    pub(crate) fn check_history(&self, history: &History) -> (out: Result<(), Error>)
        ensures match out {
            Ok(()) => self.history_id() == history.history_id() && self.lineage_id() == history.topology.lineage_id(),
            Err(error) => error is WrongHistory && self.history_id() != history.history_id(),
        },
    {
        if !self.clocks.check_history(&history.topology) {
            return Err(Error::WrongHistory);
        }
        Ok(())
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_shared_hashes_preserve_every_byte_position() {
        let mut hashes = vec![CommitHash([0; 32]), CommitHash([255; 32])];
        for offset in (0..32).rev() {
            for byte in [1, 127, 255] {
                let mut bytes = [0; 32];
                bytes[offset] = byte;
                hashes.push(CommitHash(bytes));
                hashes.push(CommitHash(bytes));
            }
        }
        let expected: BTreeSet<_> = hashes.iter().map(|hash| hash.0).collect();
        super::super::stdlib::normalize_hashes(&mut hashes);
        assert!(hashes.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(hashes.len(), expected.len());
        assert_eq!(
            hashes.iter().map(|hash| hash.0).collect::<BTreeSet<_>>(),
            expected
        );
        let original = hashes.clone();
        let shared = super::super::stdlib::share_hashes(hashes);
        assert_eq!(&*shared, original.as_slice());
        assert_eq!(
            shared.iter().map(|hash| hash.0).collect::<BTreeSet<_>>(),
            expected
        );
        let unsorted = vec![
            CommitHash([2; 32]),
            CommitHash([0; 32]),
            CommitHash([2; 32]),
        ];
        let shared_unsorted = super::super::stdlib::share_hashes(unsorted.clone());
        assert_eq!(&*shared_unsorted, unsorted.as_slice());
        let mut empty = Vec::new();
        super::super::stdlib::normalize_hashes(&mut empty);
        assert!(super::super::stdlib::share_hashes(empty).is_empty());
    }
}
