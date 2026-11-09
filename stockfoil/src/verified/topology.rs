//! Owned production topology: construction, selection and mutation preserve one invariant.
use super::{
    ancestry_cache::{self, Cache},
    author::Author,
    clock::NodeIdx,
    commit_hash::CommitHash,
    indexes::{AuthorIndex, HashIndex},
    storage::{self, Node},
    tree::Clock,
};
#[cfg(verus_keep_ghost)]
use crate::{
    ancestry_walk::summarizes,
    chain_selection_refinement::{self as selection, authors, labels, sound},
    difference::tables_view,
    graph,
    storage_refinement::{graph_of, parent_set},
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Topology<P> {
    nodes: Vec<Node<CommitHash, P, Cache>>,
    parent_ids: Vec<NodeIdx>,
    chain_tips: Vec<NodeIdx>,
    chain_nodes: Vec<Vec<NodeIdx>>,
    chain_authors: Vec<Author>,
    by_hash: HashIndex,
    author_chains: AuthorIndex,
    heads: super::heads::Heads,
    identity: super::identity::Identity,
    #[cfg(verus_keep_ghost)] lineage: Tracked<crate::lineage::Lineage>,
}

#[derive(Debug)]
pub(crate) enum InsertError {
    Duplicate,
    Missing(CommitHash),
    Capacity,
}

/// Keep finite-width guards independent of the potentially enormous graph they protect.
pub(crate) fn can_append(node_count: usize, parent_count: usize, incoming_count: usize) -> (out: bool)
    ensures out == (node_count <= u32::MAX && incoming_count <= u32::MAX
        && parent_count + incoming_count <= usize::MAX),
{
    node_count <= u32::MAX as usize && incoming_count <= u32::MAX as usize
        && incoming_count <= usize::MAX - parent_count
}

pub(crate) fn fresh_chain(chain_count: usize) -> (out: Option<u32>)
    ensures match out {
        Some(c) => chain_count < u32::MAX && c == chain_count,
        None => chain_count >= u32::MAX,
    },
{
    if chain_count >= u32::MAX as usize { None } else { Some(chain_count as u32) }
}

impl<P> Topology<P> {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { self.valid() }

    pub(crate) proof fn established(tracked &self)
        ensures self.valid(),
    { use_type_invariant(self); }

    pub(crate) closed spec fn history_id(&self) -> u64 { self.identity.raw() }
    pub(crate) fn identity(&self) -> (out: &super::identity::Identity)
        ensures out.raw() == self.history_id(), self.valid() ==> out.lineage() == self.lineage_id(),
    { &self.identity }

    pub(crate) closed spec fn lineage_id(&self) -> vstd::resource::Loc { self.lineage@.id() }

    pub(crate) proof fn snapshot(tracked &self) -> (tracked out: crate::lineage::Receipt)
        requires self.valid(),
        ensures out.id() == self.lineage_id(), out.state() == self.read_state(),
    { self.lineage.borrow().snapshot(self.read_state()) }

    pub(crate) proof fn compatible(tracked &self, tracked receipt: &crate::lineage::Receipt)
        requires self.valid(), receipt.id() == self.lineage_id(),
        ensures self.read_state().extends(receipt.state()),
    { self.lineage.borrow().compatible(receipt, self.read_state()); }

    pub(crate) open spec fn read_state(&self) -> crate::capture_refinement::ReadState {
        crate::capture_refinement::ReadState { labels: self.labels(), hashes: self.hashes(),
            graph: self.graph(), tables: tables_view(self.tables_view()) }
    }
    pub(crate) closed spec fn nodes_view(&self) -> Seq<Node<CommitHash, P, Cache>> { self.nodes@ }
    pub(crate) closed spec fn ids_view(&self) -> Seq<NodeIdx> { self.parent_ids@ }
    pub(crate) closed spec fn tips_view(&self) -> Seq<NodeIdx> { self.chain_tips@ }
    pub(crate) closed spec fn owners_view(&self) -> Seq<Author> { self.chain_authors@ }
    pub(crate) closed spec fn tables_view(&self) -> Seq<Vec<NodeIdx>> { self.chain_nodes@ }
    pub(crate) closed spec fn graph(&self) -> graph::Graph { graph_of(self.nodes@, self.parent_ids@) }
    pub(crate) closed spec fn labels(&self) -> Seq<u32> { storage::labels(self.nodes@) }
    pub(crate) closed spec fn hashes(&self) -> Map<CommitHash, NodeIdx> { self.by_hash.contents() }
    pub(crate) closed spec fn head_hashes(&self) -> Set<CommitHash> { self.heads.contents() }
    pub(crate) closed spec fn valid(&self) -> bool {
        &&& self.identity.lineage() == self.lineage_id()
        &&& self.lineage@.valid(self.read_state())
        &&& storage::valid(self.nodes@, self.parent_ids@, self.chain_tips@, tables_view(self.chain_nodes@))
        &&& self.chain_authors.len() == self.chain_tips.len()
        &&& self.by_hash.matches(self.nodes@)
        &&& self.author_chains.matches(self.chain_authors@)
        &&& self.heads.matches(self.nodes@, self.parent_ids@)
        &&& ancestry_cache::valid(self.nodes@, self.parent_ids@)
        &&& graph::chain_invariant(self.graph(), labels(self.nodes@), authors(self.nodes@, self.chain_authors@))
        &&& self.labels().to_set().len() <= self.chain_tips.len() <= u32::MAX
    }

    pub(crate) proof fn permission_facts(&self)
        requires self.valid(),
        ensures self.labels().len() == self.nodes_view().len() <= u32::MAX as nat + 1,
            self.read_state().valid(),
            forall|h: CommitHash| self.hashes().contains_key(h) ==> self.hashes()[h]@ < self.labels().len(),
            forall|n: int| 0 <= n < self.labels().len() ==> self.nodes_view()[n].chain == self.labels()[n]
                && self.labels()[n] < self.owners_view().len(),
    {
        assert forall|n: int| 0 <= n < self.labels().len() implies self.nodes_view()[n].chain == self.labels()[n]
            && self.labels()[n] < self.owners_view().len() by {
            selection::node_chain_bound(self.nodes@, self.parent_ids@, self.chain_tips@,
                tables_view(self.chain_nodes@), n);
        }
    }

    pub(crate) proof fn metadata_facts(tracked &self)
        ensures
            forall|h: CommitHash| self.hashes().contains_key(h) ==>
                self.hashes()[h]@ < self.nodes_view().len(),
            forall|i: int| #![trigger self.nodes_view()[i]] 0 <= i < self.nodes_view().len() ==> {
                let node = self.nodes_view()[i];
                &&& node.chain < self.owners_view().len()
                &&& node.chain == self.read_state().labels[i]
                &&& node.parents_start + node.parents_len <= self.ids_view().len()
                &&& forall|j: int| node.parents_start <= j < node.parents_start + node.parents_len ==>
                    self.ids_view()[j]@ < i
            },
    { self.established(); self.permission_facts(); self.hash_facts(); }

    pub(crate) proof fn hash_facts(&self)
        requires self.valid(),
        ensures forall|h: CommitHash| self.hashes().contains_key(h) ==>
            self.hashes()[h]@ < self.nodes_view().len() && self.nodes_view()[self.hashes()[h]@ as int].hash == h,
    {}

    pub(crate) proof fn head_hash_facts(tracked &self)
        ensures forall|h:CommitHash| self.head_hashes().contains(h) ==> self.hashes().contains_key(h),
    {
        self.established(); self.traversal_facts();
        reveal(super::heads::Heads::matches);
        assert forall|h:CommitHash| self.head_hashes().contains(h) implies self.hashes().contains_key(h) by {
            let i = choose|i:int| 0 <= i < self.nodes_view().len() && self.nodes_view()[i].hash == h;
        }
    }

    pub(crate) proof fn node_count_bound(&self)
        ensures self.nodes_view().len() <= usize::MAX,
    { vstd::std_specs::vec::axiom_spec_len(&self.nodes); }

    /// Parent storage and hash/index correspondence, without chain traversal facts.
    pub(crate) proof fn parent_graph_facts(&self)
        requires self.valid(),
        ensures storage::linked(self.nodes_view(), self.ids_view()),
            self.graph() == graph_of(self.nodes_view(), self.ids_view()),
            forall|i: int| 0 <= i < self.nodes_view().len() ==>
                self.hashes().contains_key(self.nodes_view()[i].hash)
                && self.hashes()[self.nodes_view()[i].hash]@ == i,
    {}

    /// Layout and causal order of the actual stored chain tables.
    pub(crate) proof fn traversal_facts(&self)
        requires self.valid(),
        ensures graph::dag(self.graph()), self.graph().len() == self.nodes_view().len(),
            forall|i: int| 0 <= i < self.nodes_view().len() ==>
                self.hashes().contains_key(self.nodes_view()[i].hash)
                && self.hashes()[self.nodes_view()[i].hash]@ == i,
            forall|c: int| 0 <= c < self.tables_view().len() ==>
                super::clock::ordered(self.tables_view()[c]@)
                && super::chain_tables::row_ordered(self.tables_view()[c]@),
            forall|c: int, p: int| #![trigger self.tables_view()[c][p]] 0 <= c < self.tables_view().len()
                && 0 <= p < self.tables_view()[c].len() ==> {
                let n = self.tables_view()[c][p];
                n@ < self.nodes_view().len() && self.nodes_view()[n@ as int].chain == c
            },
            forall|a: int, d: int| 0 <= a <= d < self.nodes_view().len()
                && self.nodes_view()[a].chain == self.nodes_view()[d].chain ==>
                graph::reaches(self.graph(), a, d),
    {
        assert forall|c: int| 0 <= c < self.tables_view().len() implies
            super::clock::ordered(self.tables_view()[c]@)
                && super::chain_tables::row_ordered(self.tables_view()[c]@) by {
            assert(super::chain_tables::row_valid(tables_view(self.chain_nodes@)[c], self.chain_tips[c]));
        }
        assert forall|c: int, p: int| #![trigger self.tables_view()[c][p]] 0 <= c < self.tables_view().len()
            && 0 <= p < self.tables_view()[c].len() implies {
                let n = self.tables_view()[c][p];
                n@ < self.nodes_view().len() && self.nodes_view()[n@ as int].chain == c
            } by {
            let n = self.tables_view()[c][p];
            assert(tables_view(self.chain_nodes@)[c].contains(n));
            assert(storage::labels(self.nodes@)[n@ as int] == c);
        }
        assert forall|a: int, d: int| 0 <= a <= d < self.nodes_view().len()
            && self.nodes_view()[a].chain == self.nodes_view()[d].chain implies
            graph::reaches(self.graph(), a, d) by {
            assert(labels(self.nodes@)[a] == labels(self.nodes@)[d]);
        }
    }

    pub(crate) proof fn traversal_node(&self, node: NodeIdx)
        requires self.valid(), node@ < self.nodes_view().len(),
        ensures self.nodes_view()[node@ as int].chain < self.tables_view().len(),
            self.tables_view()[self.nodes_view()[node@ as int].chain as int]@.contains(node),
    {
        let chains = storage::labels(self.nodes@);
        assert(chains[node@ as int] == self.nodes_view()[node@ as int].chain);
        assert(tables_view(self.chain_nodes@)[chains[node@ as int] as int].contains(node));
    }

    pub(crate) fn new() -> (out: Self)
        ensures out.valid(), out.nodes_view().len() == 0, out.hashes() == Map::<CommitHash, NodeIdx>::empty(),
            out.head_hashes() == Set::<CommitHash>::empty(),
            out.ids_view().len() == 0, out.tips_view().len() == 0, out.graph() == Seq::<ISet<int>>::empty(),
    {
        let (nodes, parent_ids, chain_tips, chain_nodes) = storage::empty();
        let tracked lineage = crate::lineage::Lineage::new(crate::capture_refinement::ReadState {
            labels: Seq::empty(), hashes: Map::empty(), graph: Seq::empty(), tables: Seq::empty() });
        let identity = super::identity::Identity::new(#[cfg(verus_keep_ghost)] { Ghost(lineage.id()) });
        let chain_authors = Vec::new();
        let by_hash = HashIndex::new();
        let author_chains = AuthorIndex::new();
        let heads = super::heads::Heads::new();
        proof { heads.empty_matches(nodes@, parent_ids@); }
        assert(storage::labels(nodes@) =~= Seq::<u32>::empty());
        assert(tables_view(chain_nodes@) =~= Seq::<Seq<NodeIdx>>::empty());
        assert(graph_of(nodes@, parent_ids@) =~= Seq::<ISet<int>>::empty());
        assert(storage::labels(nodes@).to_set() =~= Set::<u32>::empty());
        Self { nodes, parent_ids, chain_tips, chain_nodes, chain_authors,
            by_hash, author_chains, heads, identity,
            #[cfg(verus_keep_ghost)] lineage: Tracked(lineage) }
    }

    pub(crate) fn nodes(&self) -> (out: &Vec<Node<CommitHash, P, Cache>>)
        ensures out@ == self.nodes_view(),
    { &self.nodes }
    pub(crate) fn parent_ids(&self) -> (out: &Vec<NodeIdx>)
        ensures out@ == self.ids_view(),
    { &self.parent_ids }
    #[cfg(any(test, verus_keep_ghost))]
    pub(crate) fn chain_tips(&self) -> (out: &Vec<NodeIdx>)
        ensures out@ == self.tips_view(),
    { &self.chain_tips }
    pub(crate) fn chain_nodes(&self) -> (out: &Vec<Vec<NodeIdx>>)
        ensures out@ == self.tables_view(),
    { &self.chain_nodes }
    pub(crate) fn chain_authors(&self) -> (out: &Vec<Author>)
        ensures out@ == self.owners_view(),
    { &self.chain_authors }
    pub(crate) fn by_hash(&self) -> (out: &HashIndex)
        ensures out.contents() == self.hashes(),
    { &self.by_hash }
    pub(crate) fn heads(&self) -> (out: &std::collections::BTreeSet<CommitHash>)
        ensures out@ == self.head_hashes(),
    { self.heads.as_set() }
    #[cfg(test)]
    pub(crate) fn author_chains(&self) -> &AuthorIndex { &self.author_chains }

    /// Admission metadata cannot change ancestry, hashes, ownership or cache bindings.
    fn set_flags_data(nodes: &mut Vec<Node<CommitHash, P, Cache>>, idx: NodeIdx,
        admitted: bool, in_fragment: bool,
        #[cfg(verus_keep_ghost)] Ghost(before): Ghost<Self>)
        requires before.valid(), old(nodes)@ == before.nodes@, idx@ < old(nodes).len(),
        ensures final(nodes)@ == before.nodes@.update(idx@ as int,
            Node { admitted, in_fragment, ..before.nodes@[idx@ as int] }),
            (Self { nodes: *final(nodes), ..before }).valid(),
            (Self { nodes: *final(nodes), ..before }).graph() == before.graph(),
            (Self { nodes: *final(nodes), ..before }).labels() == before.labels(),
        no_unwind
    {
        let node = &mut nodes[idx.index()];
        node.admitted = admitted;
        node.in_fragment = in_fragment;
        proof {
            let after = Self { nodes: *nodes, ..before };
            assert(after.graph() =~= before.graph());
            assert(after.labels() =~= before.labels());
            assert(labels(nodes@) =~= labels(before.nodes@));
            assert(authors(nodes@, before.chain_authors@) =~= authors(before.nodes@, before.chain_authors@));
            assert forall|i: int| #![trigger nodes@[i]] 0 <= i < nodes.len() implies {
                let n = nodes@[i];
                &&& n.parents_start + n.parents_len <= before.parent_ids.len()
                &&& n.parents_start == if i == 0 { 0int }
                    else { nodes@[i - 1].parents_start + nodes@[i - 1].parents_len }
                &&& forall|j: int| n.parents_start <= j < n.parents_start + n.parents_len
                    ==> before.parent_ids@[j]@ < i
            } by {
                assert(nodes@[i].parents_start == before.nodes@[i].parents_start);
                assert(nodes@[i].parents_len == before.nodes@[i].parents_len);
                if i > 0 { assert(nodes@[i - 1].parents_start == before.nodes@[i - 1].parents_start); }
            }
            assert(storage::linked(nodes@, before.parent_ids@));
            assert(storage::valid(nodes@, before.parent_ids@, before.chain_tips@, tables_view(before.chain_nodes@)));
            assert forall|hash: CommitHash| after.hashes().contains_key(hash) implies
                after.hashes()[hash]@ < nodes.len() && nodes@[after.hashes()[hash]@ as int].hash == hash by {
                assert(nodes@[after.hashes()[hash]@ as int].hash == before.nodes@[after.hashes()[hash]@ as int].hash);
            }
            assert forall|i: int| #![trigger nodes@[i]] 0 <= i < nodes.len() implies
                after.hashes().contains_key(nodes@[i].hash) && after.hashes()[nodes@[i].hash]@ == i by {
                assert(nodes@[i].hash == before.nodes@[i].hash);
            }
            assert(before.by_hash.matches(nodes@));
            before.heads.unchanged(before.nodes@, before.parent_ids@, nodes@);
            assert forall|i: int| #![trigger nodes@[i]] 0 <= i < nodes.len() implies
                nodes@[i].clock.compatible(after.graph(), after.labels(), i) by {
                assert(nodes@[i].clock == before.nodes@[i].clock);
            }
        }
    }

    fn set_flags(&mut self, idx: NodeIdx, admitted: bool, in_fragment: bool)
        requires old(self).valid(), idx@ < old(self).nodes.len(),
        ensures final(self).valid(), final(self).lineage_id() == old(self).lineage_id(),
            final(self).history_id() == old(self).history_id(),
            final(self).nodes@ == old(self).nodes@.update(idx@ as int,
                Node { admitted, in_fragment, ..old(self).nodes@[idx@ as int] }),
            final(self).graph() == old(self).graph(), final(self).labels() == old(self).labels(),
            final(self).hashes() == old(self).hashes(),
            final(self).head_hashes() == old(self).head_hashes(),
            final(self).ids_view() == old(self).ids_view(), final(self).tips_view() == old(self).tips_view(),
            final(self).owners_view() == old(self).owners_view(), final(self).tables_view() == old(self).tables_view(),
    {
        let ghost before = *self;
        Self::set_flags_data(&mut self.nodes, idx, admitted, in_fragment,
            #[cfg(verus_keep_ghost)] { Ghost(before) });
    }

    fn checked_index(&self, idx: NodeIdx) -> (out: NodeIdx)
        ensures out == idx, out@ < self.nodes.len(),
    {
        super::stdlib::expect_some(if idx.index() < self.nodes.len() { Some(idx) } else { None },
            "topology node index")
    }

    pub(crate) fn mark_admitted(&mut self, idx: NodeIdx)
        ensures final(self).valid(), final(self).lineage_id() == old(self).lineage_id(),
            final(self).history_id() == old(self).history_id(), final(self).graph() == old(self).graph(),
            final(self).head_hashes() == old(self).head_hashes(),
            final(self).labels() == old(self).labels(), final(self).hashes() == old(self).hashes(),
            final(self).ids_view() == old(self).ids_view(), final(self).tips_view() == old(self).tips_view(),
            final(self).owners_view() == old(self).owners_view(), final(self).tables_view() == old(self).tables_view(),
            final(self).nodes_view() == old(self).nodes_view().update(idx@ as int,
                Node { admitted: true, ..old(self).nodes_view()[idx@ as int] }),
    {
        proof { use_type_invariant(&*self); }
        let idx = self.checked_index(idx);
        let fragment = self.nodes[idx.index()].in_fragment;
        self.set_flags(idx, true, fragment);
    }

    pub(crate) fn mark_in_fragment(&mut self, idx: NodeIdx)
        ensures final(self).valid(), final(self).lineage_id() == old(self).lineage_id(),
            final(self).history_id() == old(self).history_id(), final(self).graph() == old(self).graph(),
            final(self).head_hashes() == old(self).head_hashes(),
            final(self).labels() == old(self).labels(), final(self).hashes() == old(self).hashes(),
            final(self).ids_view() == old(self).ids_view(), final(self).tips_view() == old(self).tips_view(),
            final(self).owners_view() == old(self).owners_view(), final(self).tables_view() == old(self).tables_view(),
            final(self).nodes_view() == old(self).nodes_view().update(idx@ as int,
                Node { in_fragment: true, ..old(self).nodes_view()[idx@ as int] }),
    {
        proof { use_type_invariant(&*self); }
        let idx = self.checked_index(idx);
        let admitted = self.nodes[idx.index()].admitted;
        self.set_flags(idx, admitted, true);
    }

    pub(crate) fn clock(&self, head: NodeIdx) -> (out: &Arc<Clock>)
        requires self.valid(), head@ < self.nodes_view().len(),
        ensures out.valid(), summarizes(out.contents(), self.labels(),
            graph::history(self.graph(), ISet::empty().insert(head@ as int))),
            forall|a: usize| a < self.nodes_view().len() ==> super::clock::below(out.lookup(self.nodes_view()[a as int].chain), a)
                == graph::history(self.graph(), ISet::empty().insert(head@ as int)).contains(a as int),
    {
        let out = ancestry_cache::clock(&self.nodes, &self.parent_ids, head);
        proof { crate::ancestry_refinement::exact_membership(out, self.nodes@, self.parent_ids@,
            authors(self.nodes@, self.chain_authors@), ISet::empty().insert(head@ as int)); }
        out
    }

    pub(crate) fn clock_for_heads(&self, heads: &[NodeIdx]) -> (out: Arc<Clock>)
        requires self.valid(), forall|i: int| 0 <= i < heads.len() ==> heads[i]@ < self.nodes_view().len(),
        ensures out.valid(), summarizes(out.contents(), self.labels(), graph::history(self.graph(), parent_set(heads@))),
            forall|a: usize| a < self.nodes_view().len() ==> super::clock::below(out.lookup(self.nodes_view()[a as int].chain), a)
                == graph::history(self.graph(), parent_set(heads@)).contains(a as int),
    {
        let out = ancestry_cache::clock_for_heads(&self.nodes, &self.parent_ids, heads);
        proof { crate::ancestry_refinement::exact_membership(&out, self.nodes@, self.parent_ids@,
            authors(self.nodes@, self.chain_authors@), parent_set(heads@)); }
        out
    }

    /// Every delivered node, using the maintained maximal chain tips.
    pub(crate) fn delivered_clock(&self) -> (out: Arc<Clock>)
        requires self.valid(),
        ensures out.valid(), summarizes(out.contents(), self.labels(), crate::head_refinement::all(self.graph())),
    {
        let out = Arc::new(Clock::from_tips(&self.chain_tips));
        proof {
            assert forall|c: u32| out.contents().contains_key(c) implies
                out.contents()[c]@ < self.labels().len()
                && self.labels()[out.contents()[c]@ as int] == c
                && crate::head_refinement::all(self.graph()).contains(out.contents()[c]@ as int) by {
                selection::tip_is_maximal(self.nodes@, self.parent_ids@, self.chain_tips@,
                    tables_view(self.chain_nodes@), c);
            }
            assert forall|n: int| crate::head_refinement::all(self.graph()).contains(n) implies
                0 <= n < self.labels().len() && out.contents().contains_key(self.labels()[n])
                    && n <= out.contents()[self.labels()[n]]@ by {
                selection::node_chain_bound(self.nodes@, self.parent_ids@, self.chain_tips@,
                    tables_view(self.chain_nodes@), n);
                selection::tip_is_maximal(self.nodes@, self.parent_ids@, self.chain_tips@,
                    tables_view(self.chain_nodes@), self.nodes@[n].chain);
            }
        }
        out
    }

    /// Resolve actual hashes, normalize repeated heads, and justify the delivered fast path.
    pub(crate) fn history_clock(&self, hashes: &[CommitHash]) -> (out: Result<Arc<Clock>, CommitHash>)
        requires self.valid(),
        ensures match out {
            Err(h) => hashes@.contains(h) && !self.hashes().contains_key(h),
            Ok(clock) => clock.valid()
                && (forall|i: int| 0 <= i < hashes.len() ==> self.hashes().contains_key(hashes[i]))
                && summarizes(clock.contents(), self.labels(), graph::history(self.graph(),
                    parent_set(hashes@.map(|i: int, h: CommitHash| self.hashes()[h]))))
                && (forall|a: usize| a < self.nodes_view().len() ==>
                    super::clock::below(clock.lookup(self.nodes_view()[a as int].chain), a)
                    == graph::history(self.graph(), parent_set(hashes@.map(|i: int, h: CommitHash| self.hashes()[h]))).contains(a as int)),
        },
    {
        let mut indices = match self.by_hash.resolve(hashes) {
            Ok(indices) => indices,
            Err(h) => return Err(h),
        };
        let ghost resolved = indices@;
        super::candidates::normalize(&mut indices);
        proof {
            assert forall|i: int| 0 <= i < indices.len() implies indices@[i]@ < self.nodes.len() by {
                assert(resolved.contains(indices@[i]));
                let j = choose|j: int| 0 <= j < resolved.len() && resolved[j] == indices@[i];
                assert(self.hashes().contains_key(hashes@[j]));
            }
            assert forall|n: int| parent_set(indices@).contains(n) == parent_set(resolved).contains(n) by {
                if parent_set(indices@).contains(n) {
                    let i = choose|i: int| 0 <= i < indices.len() && indices@[i]@ == n;
                    assert(resolved.contains(indices@[i]));
                    let j = choose|j: int| 0 <= j < resolved.len() && resolved[j] == indices@[i];
                }
                if parent_set(resolved).contains(n) {
                    let j = choose|j: int| 0 <= j < resolved.len() && resolved[j]@ == n;
                    assert(indices@.contains(resolved[j]));
                    let i = choose|i: int| 0 <= i < indices.len() && indices@[i] == resolved[j];
                }
            }
            assert(parent_set(indices@) =~= parent_set(resolved));
        }
        let out = if self.heads.covers_all(&self.nodes, &self.parent_ids, &self.by_hash, &indices) {
            proof { crate::head_refinement::heads_cover(self.graph()); }
            self.delivered_clock()
        } else {
            self.clock_for_heads(&indices)
        };
        proof { crate::ancestry_refinement::exact_membership(&out, self.nodes@, self.parent_ids@,
            authors(self.nodes@, self.chain_authors@), parent_set(resolved)); }
        Ok(out)
    }

    /// Strict parent-edge ancestry, including unknown hashes and the same-chain fast path.
    pub(crate) fn is_ancestor(&self, ancestor: &CommitHash, descendant: &CommitHash) -> (out: bool)
        requires self.valid(),
        ensures out == (self.hashes().contains_key(*ancestor) && self.hashes().contains_key(*descendant)
            && self.hashes()[*ancestor]@ < self.hashes()[*descendant]@
            && graph::reaches(self.graph(), self.hashes()[*ancestor]@ as int, self.hashes()[*descendant]@ as int)),
    {
        let a = match self.by_hash.get(ancestor) { Some(a) => *a, None => return false };
        let d = match self.by_hash.get(descendant) { Some(d) => *d, None => return false };
        if a.index() >= d.index() { return false; }
        let chain = self.nodes[a.index()].chain;
        if chain == self.nodes[d.index()].chain {
            assert(labels(self.nodes@)[a@ as int] == labels(self.nodes@)[d@ as int]);
            assert(graph::reaches(self.graph(), a@ as int, d@ as int));
            return true;
        }
        let reached = self.clock(d);
        proof {
            crate::ancestry_refinement::singleton_history(self.graph(), a@ as int, d@ as int);
        }
        reached.contains(chain, a)
    }

    /// Direct-parent preference, then the author's first covered tip, then a fresh chain.
    fn assign_chain(&self, author: &Author, parents: &[NodeIdx]) -> (out: Option<u32>)
        requires self.valid(), forall|i: int| 0 <= i < parents.len() ==> parents[i]@ < self.nodes_view().len(),
        ensures match out {
            Some(c) => sound(self.nodes_view(), self.ids_view(), self.tips_view(), self.owners_view(), parents@, c, *author)
                && (c == self.tips_view().len() ==> self.tips_view().len() < u32::MAX),
            None => self.tips_view().len() == u32::MAX,
        },
    {
        proof {
            assert forall|i: int| 0 <= i < parents.len() implies
                self.nodes@[parents[i]@ as int].chain < self.chain_tips.len() by {
                selection::node_chain_bound(self.nodes@, self.parent_ids@, self.chain_tips@,
                    tables_view(self.chain_nodes@), parents[i]@ as int);
            }
        }
        let direct = super::chain_selection::direct(&self.nodes, &self.chain_tips, &self.chain_authors, parents, author);
        if let Some(c) = direct {
            proof {
                let j = choose|j: int| 0 <= j < parents.len() && parents[j]@ < self.nodes.len()
                    && self.nodes@[parents[j]@ as int].chain == c && self.chain_tips@[c as int] == parents[j];
                let tip = self.chain_tips@[c as int]@ as int;
                assert(parent_set(parents@).contains(tip));
                graph::unfold_reaches(self.graph(), tip, tip);
                assert(graph::history(self.graph(), parent_set(parents@)).contains(tip));
            }
            return Some(c);
        }
        if !parents.is_empty() {
            let reached = self.clock_for_heads(parents);
            if let Some(chains) = self.author_chains.get(author) {
                let candidate = super::chain_selection::ancestral(chains, &self.chain_tips, &reached);
                if let Some(c) = candidate {
                    proof {
                        selection::tip_is_maximal(self.nodes@, self.parent_ids@, self.chain_tips@,
                            tables_view(self.chain_nodes@), c);
                        let j = choose|j: int| 0 <= j < chains.len() && chains@[j] == c;
                        assert(self.chain_authors@[c as int] == *author);
                    }
                    return Some(c);
                }
            }
        }
        // Clocks store a u32 coordinate count, so at most u32::MAX chains fit.
        fresh_chain(self.chain_tips.len())
    }

    pub(crate) open spec fn inserted(&self, before: Self, idx: NodeIdx, hash: CommitHash,
        author: Author, parents: Seq<CommitHash>, payload: P, depth: u8) -> bool {
        &&& self.valid()
        &&& before.valid()
        &&& self.lineage_id() == before.lineage_id()
        &&& self.history_id() == before.history_id()
        &&& self.read_state().extends(before.read_state())
        &&& idx@ == before.nodes_view().len()
        &&& self.head_hashes() == before.head_hashes().difference(parents.to_set()).insert(hash)
        &&& !before.hashes().contains_key(hash)
        &&& (forall|i: int| 0 <= i < parents.len() ==> before.hashes().contains_key(parents[i]))
        &&& self.nodes_view().len() == before.nodes_view().len() + 1
        &&& self.tips_view().len() <= before.tips_view().len() + 1
        &&& self.owners_view().subrange(0, before.owners_view().len() as int) == before.owners_view()
        &&& before.owners_view().len() <= self.owners_view().len() <= before.owners_view().len() + 1
        &&& self.ids_view() == before.ids_view() + parents.map(|i: int, h: CommitHash| before.hashes()[h])
        &&& self.labels().subrange(0, before.labels().len() as int) == before.labels()
        &&& self.nodes_view().subrange(0, before.nodes_view().len() as int) == before.nodes_view()
        &&& self.hashes() == before.hashes().insert(hash, idx)
        &&& self.nodes_view()[idx@ as int].hash == hash
        &&& self.nodes_view()[idx@ as int].payload == payload
        &&& self.nodes_view()[idx@ as int].parents_start == before.ids_view().len()
        &&& self.nodes_view()[idx@ as int].parents_len == parents.len()
        &&& self.nodes_view()[idx@ as int].depth == depth
        &&& self.owners_view()[self.nodes_view()[idx@ as int].chain as int] == author
        &&& self.graph() == before.graph().push(parent_set(parents.map(|i: int, h: CommitHash| before.hashes()[h])))
        &&& !self.nodes_view()[idx@ as int].admitted && !self.nodes_view()[idx@ as int].in_fragment
    }

    #[verifier::spinoff_prover]
    #[verifier::rlimit(30)]
    fn commit(nodes: &mut Vec<Node<CommitHash, P, Cache>>,
        parent_ids: &mut Vec<NodeIdx>,
        chain_tips: &mut Vec<NodeIdx>,
        chain_nodes: &mut Vec<Vec<NodeIdx>>,
        chain_authors: &mut Vec<Author>,
        by_hash: &mut HashIndex,
        author_chains: &mut AuthorIndex,
        heads: &mut super::heads::Heads,
        #[cfg(verus_keep_ghost)] lineage: &mut Tracked<crate::lineage::Lineage>,
        incoming: &[NodeIdx], parents: &[CommitHash], hash: CommitHash, author: Author,
        payload: P, depth: u8, chain: u32, uncached_depth: u8, prepared: super::append_plan::Prepared,
        #[cfg(verus_keep_ghost)] Ghost(before): Ghost<Self>) -> (idx: NodeIdx)
        requires before.valid(),
            *old(nodes) == before.nodes,
            *old(parent_ids) == before.parent_ids,
            *old(chain_tips) == before.chain_tips,
            *old(chain_nodes) == before.chain_nodes,
            *old(chain_authors) == before.chain_authors,
            *old(by_hash) == before.by_hash,
            *old(author_chains) == before.author_chains,
            *old(heads) == before.heads,
            *old(lineage) == before.lineage,
            !before.hashes().contains_key(hash), before.nodes.len() <= u32::MAX, incoming.len() <= u32::MAX,
            before.parent_ids.len() + incoming.len() <= usize::MAX,
            incoming@ == parents@.map(|i: int, h: CommitHash| before.hashes()[h]),
            forall|i: int| 0 <= i < parents.len() ==> before.hashes().contains_key(parents[i]),
            forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < before.nodes.len(),
            sound(before.nodes@, before.parent_ids@, before.chain_tips@, before.chain_authors@, incoming@, chain, author),
            chain == before.chain_tips.len() ==> before.chain_tips.len() < u32::MAX,
            super::append_plan::ready(*old(nodes), *old(parent_ids), *old(chain_tips), *old(chain_nodes),
                *old(chain_authors), *old(by_hash), *old(author_chains), prepared, chain, author, incoming.len()),
        ensures (Self {
                nodes: *final(nodes),
                parent_ids: *final(parent_ids),
                chain_tips: *final(chain_tips),
                chain_nodes: *final(chain_nodes),
                chain_authors: *final(chain_authors),
                by_hash: *final(by_hash),
                author_chains: *final(author_chains),
                heads: *final(heads),
                lineage: *final(lineage),
                identity: before.identity,
            }).inserted(before, idx, hash, author, parents@, payload, depth),
        no_unwind
    {
        let idx = ancestry_cache::append_prepared(nodes, parent_ids, chain_tips,
            chain_nodes, chain_authors, by_hash, author_chains,
            &incoming, Node::new(hash, payload, chain, depth, uncached_depth, ()), author, prepared);
        heads.advance(hash, parents);
        assert(heads.contents() == before.head_hashes().difference(parents@.to_set()).insert(hash));
        proof {
            let after = Self {
                nodes: *nodes,
                parent_ids: *parent_ids,
                chain_tips: *chain_tips,
                chain_nodes: *chain_nodes,
                chain_authors: *chain_authors,
                by_hash: *by_hash,
                author_chains: *author_chains,
                heads: *heads,
                lineage: *lineage,
                identity: before.identity,
            };
            let last = after.nodes@.last();
            assert(after.nodes@ =~= before.nodes@.push(last));
            selection::extend_selected(before.nodes@, before.parent_ids@, before.chain_tips@,
                tables_view(before.chain_nodes@), before.chain_authors@, incoming@, last, author);
            let old_labels = before.labels().to_set();
            assert(after.labels().to_set() =~= old_labels.insert(chain));
            if chain < before.chain_tips.len() {
                selection::tip_is_maximal(before.nodes@, before.parent_ids@, before.chain_tips@,
                    tables_view(before.chain_nodes@), chain);
                assert(before.labels()[before.chain_tips@[chain as int]@ as int] == chain);
                assert(old_labels.contains(chain));
                assert(old_labels.insert(chain) =~= old_labels);
            }
            before.heads.appended(before.nodes@, before.parent_ids@, after.nodes@, after.parent_ids@,
                &before.by_hash, parents@, incoming@, &after.heads);
            assert(after.owners_view().subrange(0, before.owners_view().len() as int) =~= before.owners_view());
            assert(after.labels().subrange(0, before.labels().len() as int) =~= before.labels());
            assert(after.hashes() == before.hashes().insert(hash, idx));
            assert(after.graph() == before.graph().push(parent_set(incoming@)));
            assert(after.parent_ids@ == before.parent_ids@ + incoming@);
            assert(after.labels().len() == before.labels().len() + 1);
            assert(idx@ == before.labels().len());
            assert(after.read_state().tables == super::chain_tables::extended(before.read_state().tables, chain, idx));
            after.read_state().appended(before.read_state(), hash, idx, chain);
            let after = after.read_state();
            lineage.borrow_mut().advance(before.read_state(), after);
            assert(lineage@.valid(after));
            let published = Self {
                nodes: *nodes,
                parent_ids: *parent_ids,
                chain_tips: *chain_tips,
                chain_nodes: *chain_nodes,
                chain_authors: *chain_authors,
                by_hash: *by_hash,
                author_chains: *author_chains,
                heads: *heads,
                lineage: *lineage,
                identity: before.identity,
            };
            assert(published.valid());
        }
        idx
    }

    /// Resolve, select and append atomically. Errors preserve the logical state.
    pub(crate) fn insert(&mut self, hash: CommitHash, author: Author, parents: &[CommitHash],
        payload: P, depth: u8) -> (out: Result<NodeIdx, InsertError>)
        ensures final(self).valid(), final(self).lineage_id() == old(self).lineage_id(),
            final(self).history_id() == old(self).history_id(), final(self).read_state().extends(old(self).read_state()), match out {
            Err(error) => *final(self) == *old(self) && match error {
                InsertError::Duplicate => old(self).hashes().contains_key(hash),
                InsertError::Missing(h) => parents@.contains(h) && !old(self).hashes().contains_key(h),
                InsertError::Capacity => old(self).nodes_view().len() > u32::MAX || parents.len() > u32::MAX
                    || old(self).ids_view().len() + parents.len() > usize::MAX || old(self).tips_view().len() == u32::MAX,
            },
            Ok(idx) => final(self).inserted(*old(self), idx, hash, author, parents@, payload, depth),
        },
    {
        proof { use_type_invariant(&*self); self.read_state().reflexive(); }
        if self.by_hash.contains_key(&hash) { return Err(InsertError::Duplicate); }
        if !can_append(self.nodes.len(), self.parent_ids.len(), parents.len()) { return Err(InsertError::Capacity); }
        let incoming = match self.by_hash.resolve(parents) {
            Ok(ps) => ps,
            Err(h) => return Err(InsertError::Missing(h)),
        };
        proof {
            assert(incoming.len() == parents.len());
            assert(incoming@ == parents@.map(|i: int, h: CommitHash| self.hashes()[h]));
            assert forall|i: int| 0 <= i < parents.len() implies self.hashes().contains_key(parents[i]) by {};
            assert forall|i: int| 0 <= i < incoming.len() implies incoming@[i]@ < self.nodes.len() by {
                assert(self.hashes().contains_key(parents@[i]));
            }
        }
        let chain = match self.assign_chain(&author, &incoming) {
            Some(c) => c,
            None => return Err(InsertError::Capacity),
        };
        let mut uncached_depth = 1u8;
        let mut i = 0;
        while i < incoming.len()
            invariant i <= incoming.len(), self.valid(),
                forall|j: int| 0 <= j < incoming.len() ==> incoming@[j]@ < self.nodes.len(),
            decreases incoming.len() - i,
        {
            let d = self.nodes[incoming[i].index()].uncached_depth;
            let next = if d < u8::MAX { d + 1 } else { u8::MAX };
            if next > uncached_depth { uncached_depth = next; }
            i += 1;
        }
        let eager = depth > 0 || incoming.len() > 1 || uncached_depth >= 128;
        let ghost before = *self;
        let preparation = super::append_plan::prepare(&mut self.nodes, &mut self.parent_ids, &mut self.chain_tips,
            &mut self.chain_nodes, &mut self.chain_authors, &mut self.by_hash, &mut self.author_chains,
            chain, author, incoming.len());
        let prepared = super::stdlib::expect_some(preparation, "topology allocation capacity");
        let ghost prepared_before = *self;
        let idx = Self::commit(&mut self.nodes, &mut self.parent_ids, &mut self.chain_tips,
            &mut self.chain_nodes, &mut self.chain_authors, &mut self.by_hash, &mut self.author_chains, &mut self.heads,
            #[cfg(verus_keep_ghost)] &mut self.lineage,
            &incoming, parents, hash, author, payload, depth, chain, if eager { 0 } else { uncached_depth }, prepared,
            #[cfg(verus_keep_ghost)] { Ghost(prepared_before) });
        if eager { self.clock(idx); }
        Ok(idx)
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_metadata_index_panics_before_changing_topology() {
        let mut t = Topology::new();
        let alice = Author([1; 32]);
        let root = CommitHash([2; 32]);
        let idx = t.insert(root, alice, &[], 42u64, 0).unwrap();
        for admitted in [true, false] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if admitted {
                    t.mark_admitted(NodeIdx::new(u32::MAX));
                } else {
                    t.mark_in_fragment(NodeIdx::new(u32::MAX));
                }
            }));
            assert!(result.is_err());
            assert_eq!(t.nodes().len(), 1);
            assert!(!t.nodes()[idx.index()].admitted);
            assert!(!t.nodes()[idx.index()].in_fragment);
            assert_eq!(t.by_hash().get(&root), Some(&idx));
            assert!(t.clock(idx).contains(0, idx));
        }
        let child = CommitHash([3; 32]);
        t.insert(child, alice, &[root], 43, 0).unwrap();
        assert!(t.is_ancestor(&root, &child));
    }

    #[test]
    fn rejected_insertions_preserve_topology_and_cached_answers() {
        let mut t = Topology::new();
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let root = CommitHash([0; 32]);
        let child = CommitHash([1; 32]);
        let next = CommitHash([2; 32]);
        let missing = CommitHash([255; 32]);
        let a = t.insert(root, alice, &[], 10u64, 0).unwrap();
        let b = t.insert(child, bob, &[root, root], 20, 0).unwrap();
        let saved = Arc::clone(t.clock(b));
        let parents = t.parent_ids().clone();
        let tips = t.chain_tips().clone();
        let tables = t.chain_nodes().clone();
        let owners = t.chain_authors().clone();

        assert!(matches!(
            t.insert(root, bob, &[child], 99, 1),
            Err(InsertError::Duplicate)
        ));
        assert!(matches!(t.insert(next, alice, &[child, missing], 99, 1),
            Err(InsertError::Missing(h)) if h == missing));
        assert_eq!(t.nodes().len(), 2);
        assert_eq!(t.nodes()[a.index()].payload, 10);
        assert_eq!(t.parent_ids(), &parents);
        assert_eq!(t.chain_tips(), &tips);
        assert_eq!(t.chain_nodes(), &tables);
        assert_eq!(t.chain_authors(), &owners);
        assert!(!t.by_hash().contains_key(&next));
        assert!(Arc::ptr_eq(&saved, t.clock(b)));

        // A rejected insertion leaves the hash available for a valid retry.
        let c = t.insert(next, alice, &[child], 30, 0).unwrap();
        assert_eq!(t.nodes()[c.index()].chain, t.nodes()[a.index()].chain);
        t.mark_admitted(a);
        t.mark_in_fragment(a);
        assert!(t.nodes()[a.index()].admitted && t.nodes()[a.index()].in_fragment);
        assert!(t.is_ancestor(&root, &next));
        assert!(t.is_ancestor(&child, &next));
        assert!(!t.is_ancestor(&root, &root));
        assert!(!t.is_ancestor(&missing, &next));
        assert!(Arc::ptr_eq(&saved, t.clock(b)));
    }
}
