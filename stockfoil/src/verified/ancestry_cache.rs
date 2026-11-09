//! Cache predicates are established from actual topology, never assumed by OnceLock.
#[cfg(verus_keep_ghost)]
use super::storage;
use super::{ancestry_walk, clock::NodeIdx, stdlib::OnceValue, storage::Node, tree::Clock};
#[cfg(verus_keep_ghost)]
use crate::{
    ancestry_walk::summarizes,
    graph,
    storage_refinement::{graph_of, parent_set},
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Only the OnceLock occupies runtime storage. The labels and ancestry set are
/// immutable proof snapshots captured when this node is appended.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Cache {
    cell: OnceValue<Arc<Clock>>,
    #[cfg(verus_keep_ghost)] labels: Ghost<Seq<u32>>,
    #[cfg(verus_keep_ghost)] ancestors: Ghost<ISet<int>>,
}

pub(crate) open spec fn allowed(labels: Seq<u32>, ancestors: ISet<int>) -> spec_fn(Arc<Clock>) -> bool {
    |clock: Arc<Clock>| clock.valid() && summarizes(clock.contents(), labels, ancestors)
}

pub(crate) proof fn relabel(map: Map<u32, NodeIdx>, before: Seq<u32>, after: Seq<u32>, ancestors: ISet<int>)
    requires summarizes(map, before, ancestors),
        forall|n: int| ancestors.contains(n) ==> 0 <= n < after.len() && before[n] == after[n],
    ensures summarizes(map, after, ancestors),
{}

impl Cache {
    pub(crate) closed spec fn accepts(&self, value: Arc<Clock>) -> bool { allowed(self.labels@, self.ancestors@)(value) }

    pub(crate) closed spec fn wf(&self) -> bool { self.cell.predicate() == allowed(self.labels@, self.ancestors@) }

    pub(crate) closed spec fn compatible(&self, g: graph::Graph, labels: Seq<u32>, head: int) -> bool {
        &&& self.wf()
        &&& self.labels@.len() <= labels.len()
        &&& self.labels@ == labels.subrange(0, self.labels@.len() as int)
        &&& self.ancestors@ == graph::history(g, ISet::empty().insert(head))
        &&& forall|a: int| self.ancestors@.contains(a) ==> 0 <= a < self.labels@.len()
    }

    fn new(
        #[cfg(verus_keep_ghost)] Ghost(g): Ghost<graph::Graph>,
        #[cfg(verus_keep_ghost)] Ghost(labels): Ghost<Seq<u32>>,
        #[cfg(verus_keep_ghost)] Ghost(head): Ghost<int>,
    ) -> (out: Self)
        requires 0 <= head < g.len(), labels.len() == g.len(),
        ensures out.compatible(g, labels, head),
        no_unwind
    {
        let ghost ancestors = graph::history(g, ISet::empty().insert(head));
        let cell = OnceValue::new(#[cfg(verus_keep_ghost)] { Ghost(allowed(labels, ancestors)) });
        proof {
            assert forall|a: int| ancestors.contains(a) implies 0 <= a < labels.len() by {
                let h = choose|h: int| ISet::empty().insert(head).contains(h) && graph::reaches(g, a, h);
                assert(h == head);
                graph::reaches_order(g, a, head);
            }
            assert(labels.subrange(0, labels.len() as int) =~= labels);
        }
        Self {
            cell,
            #[cfg(verus_keep_ghost)] labels: Ghost(labels),
            #[cfg(verus_keep_ghost)] ancestors: Ghost(ancestors),
        }
    }

    /// Used for diagnostics as well as nonblocking reads during a walk.
    pub(crate) fn get(&self) -> (out: Option<&Arc<Clock>>)
        requires self.wf(),
        ensures match out { None => true, Some(clock) => self.accepts(*clock) },
    {
        self.cell.get()
    }

    pub(crate) proof fn survives_append(&self, g: graph::Graph, labels: Seq<u32>, parents: ISet<int>, chain: u32, head: int)
        requires self.compatible(g, labels, head), 0 <= head < g.len(),
        ensures self.compatible(g.push(parents), labels.push(chain), head),
    {
        let heads = ISet::empty().insert(head);
        assert forall|a: int| graph::history(g, heads).contains(a) == graph::history(g.push(parents), heads).contains(a) by {
            graph::append_preserves_reachability(g, parents, a, head);
            if graph::history(g, heads).contains(a) {
                let h = choose|h: int| heads.contains(h) && graph::reaches(g, a, h);
                assert(h == head); assert(graph::reaches(g.push(parents), a, h));
            }
            if graph::history(g.push(parents), heads).contains(a) {
                let h = choose|h: int| heads.contains(h) && graph::reaches(g.push(parents), a, h);
                assert(h == head); assert(graph::reaches(g, a, h));
            }
        }
        assert(graph::history(g, heads) =~= graph::history(g.push(parents), heads));
        assert(labels.push(chain).subrange(0, self.labels@.len() as int) =~= self.labels@);
    }
}

pub(crate) open spec fn valid<H, P>(nodes: Seq<Node<H, P, Cache>>, ids: Seq<NodeIdx>) -> bool {
    forall|i: int| #![trigger nodes[i]] 0 <= i < nodes.len() ==>
        nodes[i].clock.compatible(graph_of(nodes, ids), storage::labels(nodes), i)
}

fn read<'a, H, P>(nodes: &'a [Node<H, P, Cache>], _ids: &[NodeIdx], idx: NodeIdx) -> (out: Option<&'a Clock>)
    requires valid(nodes@, _ids@), idx@ < nodes.len(),
    ensures match out {
        None => true,
        Some(clock) => clock.valid() && summarizes(clock.contents(), storage::labels(nodes@),
            graph::history(graph_of(nodes@, _ids@), ISet::empty().insert(idx@ as int))),
    },
{
    let cache = &nodes[idx.index()].clock;
    match cache.get() {
        None => None,
        Some(clock) => {
            proof { relabel(clock.contents(), cache.labels@, storage::labels(nodes@), cache.ancestors@); }
            Some(&**clock)
        },
    }
}

/// The actual get-or-initialize path. The checked initializer supplies the
/// library predicate; callers no longer provide correct cached clock values.
pub(crate) fn clock<'a, H, P>(nodes: &'a [Node<H, P, Cache>], ids: &[NodeIdx], head: NodeIdx) -> (out: &'a Arc<Clock>)
    requires storage::linked(nodes@, ids@), valid(nodes@, ids@), head@ < nodes.len(),
        nodes.len() <= u32::MAX as nat + 1, storage::labels(nodes@).to_set().len() <= u32::MAX,
    ensures out.valid(), summarizes(out.contents(), storage::labels(nodes@),
        graph::history(graph_of(nodes@, ids@), ISet::empty().insert(head@ as int))),
{
    let cache = &nodes[head.index()].clock;
    let init = || -> (out: Arc<Clock>)
        ensures allowed(cache.labels@, cache.ancestors@)(out),
    {
        let lookup = |idx: NodeIdx| -> (out: Option<&Clock>)
            requires idx@ < nodes.len(),
            ensures match out {
                None => true,
                Some(clock) => clock.valid() && summarizes(clock.contents(), storage::labels(nodes@),
                    graph::history(graph_of(nodes@, ids@), ISet::empty().insert(idx@ as int))),
            },
        { read(nodes, ids, idx) };
        let out = Arc::new(ancestry_walk::compute(nodes, ids, head, lookup));
        proof { relabel(out.contents(), storage::labels(nodes@), cache.labels@, cache.ancestors@); }
        out
    };
    let out = cache.cell.get_or_init(init);
    proof { relabel(out.contents(), cache.labels@, storage::labels(nodes@), cache.ancestors@); }
    out
}

pub(crate) fn clock_for_heads<H, P>(nodes: &[Node<H, P, Cache>], ids: &[NodeIdx], heads: &[NodeIdx]) -> (out: Arc<Clock>)
    requires storage::linked(nodes@, ids@), valid(nodes@, ids@),
        nodes.len() <= u32::MAX as nat + 1, storage::labels(nodes@).to_set().len() <= u32::MAX,
        forall|i: int| 0 <= i < heads.len() ==> heads[i]@ < nodes.len(),
    ensures out.valid(), summarizes(out.contents(), storage::labels(nodes@), graph::history(graph_of(nodes@, ids@), parent_set(heads@))),
{
    let lookup = |idx: NodeIdx| -> (out: &Arc<Clock>)
        requires idx@ < nodes.len(),
        ensures out.valid(), summarizes(out.contents(), storage::labels(nodes@),
            graph::history(graph_of(nodes@, ids@), ISet::empty().insert(idx@ as int))),
    { clock(nodes, ids, idx) };
    ancestry_walk::combine(nodes, ids, heads, lookup)
}

/// Append constructs the cache predicate from the actual next graph, then
/// preserves every older cache's interpretation as storage and indexes grow.
#[cfg(any(test, verus_keep_ghost))]
pub(crate) fn append<P>(
    nodes: &mut Vec<Node<super::commit_hash::CommitHash, P, Cache>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<super::author::Author>,
    hashes: &mut super::indexes::HashIndex, authors: &mut super::indexes::AuthorIndex, incoming: &[NodeIdx],
    node: Node<super::commit_hash::CommitHash, P, ()>, author: super::author::Author,
) -> (position: NodeIdx)
    requires storage::valid(old(nodes)@, old(ids)@, old(tips)@, super::difference::tables_view(old(tables)@)),
        valid(old(nodes)@, old(ids)@),
        old(hashes).matches(old(nodes)@), old(authors).matches(old(owners)@),
        !old(hashes).contents().contains_key(node.hash),
        old(owners).len() == old(tips).len(), node.chain <= old(tips).len(),
        node.chain < old(owners).len() ==> old(owners)@[node.chain as int] == author,
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, super::difference::tables_view(final(tables)@)),
        valid(final(nodes)@, final(ids)@),
        final(hashes).matches(final(nodes)@), final(authors).matches(final(owners)@),
        final(owners).len() == final(tips).len(), position@ == old(nodes).len(),
        final(hashes).contents() == old(hashes).contents().insert(node.hash, position),
        final(owners)@ == if node.chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, position) } else { old(tips)@.push(position) },
        super::difference::tables_view(final(tables)@) == super::chain_tables::extended(super::difference::tables_view(old(tables)@), node.chain, position),
        final(nodes).len() == old(nodes).len() + 1,
        final(nodes)@.subrange(0, old(nodes).len() as int) == old(nodes)@,
        final(nodes)@.last().hash == node.hash, final(nodes)@.last().payload == node.payload,
        final(nodes)@.last().chain == node.chain, final(nodes)@.last().depth == node.depth,
        final(nodes)@.last().admitted == node.admitted, final(nodes)@.last().in_fragment == node.in_fragment,
        final(nodes)@.last().uncached_depth == node.uncached_depth,
        final(nodes)@.last().parents_start == old(ids).len(), final(nodes)@.last().parents_len == incoming.len(),
        final(ids)@ == old(ids)@ + incoming@,
        graph_of(final(nodes)@, final(ids)@) == graph_of(old(nodes)@, old(ids)@).push(parent_set(incoming@)),
        storage::labels(final(nodes)@) == storage::labels(old(nodes)@).push(node.chain),
{
    let preparation = super::append_plan::prepare(nodes, ids, tips, tables, owners, hashes, authors, node.chain, author, incoming.len());
    let prepared = super::stdlib::expect_some(preparation, "topology allocation capacity");
    append_prepared(nodes, ids, tips, tables, owners, hashes, authors, incoming, node, author, prepared)
}

pub(crate) fn append_prepared<P>(
    nodes: &mut Vec<Node<super::commit_hash::CommitHash, P, Cache>>, ids: &mut Vec<NodeIdx>,
    tips: &mut Vec<NodeIdx>, tables: &mut Vec<Vec<NodeIdx>>, owners: &mut Vec<super::author::Author>,
    hashes: &mut super::indexes::HashIndex, authors: &mut super::indexes::AuthorIndex, incoming: &[NodeIdx],
    node: Node<super::commit_hash::CommitHash, P, ()>, author: super::author::Author, prepared: super::append_plan::Prepared,
) -> (position: NodeIdx)
    requires super::append_plan::ready(*old(nodes), *old(ids), *old(tips), *old(tables), *old(owners),
            *old(hashes), *old(authors), prepared, node.chain, author, incoming.len()),
        storage::valid(old(nodes)@, old(ids)@, old(tips)@, super::difference::tables_view(old(tables)@)),
        valid(old(nodes)@, old(ids)@),
        old(hashes).matches(old(nodes)@), old(authors).matches(old(owners)@),
        !old(hashes).contents().contains_key(node.hash),
        old(owners).len() == old(tips).len(), node.chain <= old(tips).len(),
        node.chain < old(owners).len() ==> old(owners)@[node.chain as int] == author,
        old(nodes).len() <= u32::MAX, incoming.len() <= u32::MAX,
        old(ids).len() + incoming.len() <= usize::MAX,
        forall|i: int| 0 <= i < incoming.len() ==> incoming[i]@ < old(nodes).len(),
    ensures storage::valid(final(nodes)@, final(ids)@, final(tips)@, super::difference::tables_view(final(tables)@)),
        valid(final(nodes)@, final(ids)@),
        final(hashes).matches(final(nodes)@), final(authors).matches(final(owners)@),
        final(owners).len() == final(tips).len(), position@ == old(nodes).len(),
        final(hashes).contents() == old(hashes).contents().insert(node.hash, position),
        final(owners)@ == if node.chain < old(owners).len() { old(owners)@ } else { old(owners)@.push(author) },
        final(tips)@ == if node.chain < old(tips).len() { old(tips)@.update(node.chain as int, position) } else { old(tips)@.push(position) },
        super::difference::tables_view(final(tables)@) == super::chain_tables::extended(super::difference::tables_view(old(tables)@), node.chain, position),
        final(nodes).len() == old(nodes).len() + 1,
        final(nodes)@.subrange(0, old(nodes).len() as int) == old(nodes)@,
        final(nodes)@.last().hash == node.hash, final(nodes)@.last().payload == node.payload,
        final(nodes)@.last().chain == node.chain, final(nodes)@.last().depth == node.depth,
        final(nodes)@.last().admitted == node.admitted, final(nodes)@.last().in_fragment == node.in_fragment,
        final(nodes)@.last().uncached_depth == node.uncached_depth,
        final(nodes)@.last().parents_start == old(ids).len(), final(nodes)@.last().parents_len == incoming.len(),
        final(ids)@ == old(ids)@ + incoming@,
        graph_of(final(nodes)@, final(ids)@) == graph_of(old(nodes)@, old(ids)@).push(parent_set(incoming@)),
        storage::labels(final(nodes)@) == storage::labels(old(nodes)@).push(node.chain),
    no_unwind
{
    let ghost before_nodes = nodes@;
    let ghost before_ids = ids@;
    let ghost g = graph_of(nodes@, ids@);
    let ghost labels = storage::labels(nodes@);
    let ghost parents = parent_set(incoming@);
    let chain = node.chain;
    let cache = Cache::new(
        #[cfg(verus_keep_ghost)] { Ghost(g.push(parents)) },
        #[cfg(verus_keep_ghost)] { Ghost(labels.push(chain)) },
        #[cfg(verus_keep_ghost)] { Ghost(nodes.len() as int) },
    );
    let linked = Node {
        hash: node.hash, payload: node.payload, parents_start: node.parents_start, parents_len: node.parents_len,
        chain, depth: node.depth, admitted: node.admitted, in_fragment: node.in_fragment,
        uncached_depth: node.uncached_depth, clock: cache,
    };
    let position = super::append_plan::append(nodes, ids, tips, tables, owners, hashes, authors, incoming, linked, author, prepared);
    proof {
        crate::storage_refinement::append_preserves_graph(before_nodes, before_ids, incoming@, nodes@.last());
        assert(storage::labels(nodes@) =~= labels.push(chain));
        assert forall|i: int| #![trigger nodes@[i]] 0 <= i < nodes.len() implies
            nodes@[i].clock.compatible(graph_of(nodes@, ids@), storage::labels(nodes@), i) by {
            if i < before_nodes.len() {
                assert(nodes@[i] == before_nodes[i]);
                before_nodes[i].clock.survives_append(g, labels, parents, chain, i);
            }
        }
        assert(nodes@.subrange(0, before_nodes.len() as int) =~= before_nodes);
    }
    position
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{
        author::Author,
        commit_hash::CommitHash,
        indexes::{AuthorIndex, HashIndex},
        storage,
    };
    use std::sync::Barrier;

    #[test]
    fn real_cache_initialization_is_shared_and_survives_append() {
        let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<CommitHash, u64, Cache>();
        let mut owners = Vec::new();
        let mut hashes = HashIndex::new();
        let mut authors = AuthorIndex::new();
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let mut add = |id: u8, chain, author, parents: &[NodeIdx]| {
            append(
                &mut nodes,
                &mut ids,
                &mut tips,
                &mut tables,
                &mut owners,
                &mut hashes,
                &mut authors,
                parents,
                Node::new(CommitHash([id; 32]), id as u64, chain, 0, 1, ()),
                author,
            )
        };
        let root = add(0, 0, alice, &[]);
        let left = add(1, 0, alice, &[root, root]);
        let right = add(2, 1, bob, &[root]);
        let merge = add(3, 0, alice, &[left, right, left]);
        let unrelated = add(4, 2, bob, &[]);
        assert!(nodes[merge.index()].clock.get().is_none());
        let start = Barrier::new(6);
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..6)
                .map(|_| {
                    let nodes = &nodes;
                    let ids = &ids;
                    let start = &start;
                    scope.spawn(move || {
                        start.wait();
                        Arc::clone(clock(nodes, ids, merge))
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        let saved = &results[0];
        assert!(results.iter().all(|clock| Arc::ptr_eq(saved, clock)));
        assert!(saved.contains(0, root) && saved.contains(1, right));
        assert!(!saved.contains(2, unrelated));
        assert_eq!(
            **saved,
            ancestry_walk::compute(&nodes, &ids, merge, |_| None)
        );
        let next = append(
            &mut nodes,
            &mut ids,
            &mut tips,
            &mut tables,
            &mut owners,
            &mut hashes,
            &mut authors,
            &[merge, unrelated, merge],
            Node::new(CommitHash([5; 32]), 5, 0, 0, 1, ()),
            alice,
        );
        assert!(Arc::ptr_eq(saved, clock(&nodes, &ids, merge)));
        let latest = clock(&nodes, &ids, next);
        assert_eq!(
            **latest,
            ancestry_walk::compute(&nodes, &ids, next, |_| None)
        );
        assert!(latest.contains(0, next) && latest.contains(2, unrelated));
        assert!(!saved.contains(0, next) && !saved.contains(2, unrelated));
        let multi = clock_for_heads(&nodes, &ids, &[merge, unrelated, merge]);
        assert!(multi.contains(0, merge) && multi.contains(2, unrelated));
        assert!(!multi.contains(0, next));
        assert_eq!(
            std::mem::size_of::<Cache>(),
            std::mem::size_of::<std::sync::OnceLock<Arc<Clock>>>()
        );
    }
}
