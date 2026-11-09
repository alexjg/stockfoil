//! The production cached parent walk. ancestry_cache establishes its cache premise.
use super::{
    clock::NodeIdx,
    storage::{self, Node},
    tree::{Clock, ClockBuilder},
};
#[cfg(verus_keep_ghost)]
use crate::{
    graph,
    storage_refinement::{graph_of, parent_set},
    walk,
};
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Every coordinate names an actually covered node; every covered node is
/// bounded by its chain's coordinate. No ancestry membership is assumed here.
pub(crate) open spec fn summarizes(map: Map<u32, NodeIdx>, labels: Seq<u32>, covered: ISet<int>) -> bool {
    &&& forall|c: u32| map.contains_key(c) ==> map[c]@ < labels.len()
        && labels[map[c]@ as int] == c && covered.contains(map[c]@ as int)
    &&& forall|n: int| covered.contains(n) ==> 0 <= n < labels.len()
        && map.contains_key(labels[n]) && n <= map[labels[n]]@
}

pub(crate) proof fn summary_keys(map: Map<u32, NodeIdx>, labels: Seq<u32>, covered: ISet<int>)
    requires summarizes(map, labels, covered),
    ensures map.dom().subset_of(labels.to_set()), map.dom().len() <= labels.to_set().len(),
{
    assert forall|c: u32| map.contains_key(c) implies labels.to_set().contains(c) by {
        assert(labels[map[c]@ as int] == c);
    }
    vstd::set_lib::lemma_len_subset(map.dom(), labels.to_set());
}

proof fn include_summary(before: Map<u32, NodeIdx>, after: Map<u32, NodeIdx>, labels: Seq<u32>,
    covered: ISet<int>, n: NodeIdx)
    requires summarizes(before, labels, covered), n@ < labels.len(),
        after == if before.contains_key(labels[n@ as int]) && n@ <= before[labels[n@ as int]]@ {
            before
        } else { before.insert(labels[n@ as int], n) },
    ensures summarizes(after, labels, covered.insert(n@ as int)),
{}

pub(crate) proof fn merge_summary(a: Map<u32, NodeIdx>, b: Map<u32, NodeIdx>, out: Map<u32, NodeIdx>,
    labels: Seq<u32>, covered: ISet<int>, added: ISet<int>)
    requires summarizes(a, labels, covered), summarizes(b, labels, added), Clock::joins(out, a, b),
    ensures summarizes(out, labels, covered.union(added)),
{}

// Proof-only arguments are omitted from the standalone Cargo build.
fn include_node(builder: &mut ClockBuilder, chain: u32, idx: NodeIdx,
    #[cfg(verus_keep_ghost)] Ghost(labels): Ghost<Seq<u32>>, #[cfg(verus_keep_ghost)] Ghost(covered): Ghost<ISet<int>>)
    requires old(builder).valid(), old(builder).size() == old(builder).contents().dom().len(),
        summarizes(old(builder).contents(), labels, covered), idx@ < labels.len(), labels[idx@ as int] == chain,
        labels.to_set().len() <= u32::MAX,
    ensures final(builder).valid(), final(builder).size() == final(builder).contents().dom().len(),
        summarizes(final(builder).contents(), labels, covered.insert(idx@ as int)),
{
    let ghost before = builder.contents();
    proof {
        summary_keys(before, labels, covered);
        assert(labels.to_set().contains(chain));
        assert(before.dom().insert(chain).subset_of(labels.to_set()));
        vstd::set_lib::lemma_len_subset(before.dom().insert(chain), labels.to_set());
    }
    builder.include(chain, idx);
    proof { include_summary(before, builder.contents(), labels, covered, idx); }
}

fn merge_cached(builder: &mut ClockBuilder, clock: &Clock,
    #[cfg(verus_keep_ghost)] Ghost(labels): Ghost<Seq<u32>>, #[cfg(verus_keep_ghost)] Ghost(covered): Ghost<ISet<int>>, #[cfg(verus_keep_ghost)] Ghost(added): Ghost<ISet<int>>)
    requires old(builder).valid(), clock.valid(),
        summarizes(old(builder).contents(), labels, covered), summarizes(clock.contents(), labels, added),
        labels.to_set().len() <= u32::MAX,
    ensures final(builder).valid(), final(builder).size() == final(builder).contents().dom().len(),
        summarizes(final(builder).contents(), labels, covered.union(added)),
{
    let ghost before = builder.contents();
    proof {
        summary_keys(before, labels, covered);
        summary_keys(clock.contents(), labels, added);
        assert(before.dom().union(clock.contents().dom()).subset_of(labels.to_set()));
        vstd::set_lib::lemma_len_subset(before.dom().union(clock.contents().dom()), labels.to_set());
    }
    builder.merge(clock);
    proof { merge_summary(before, clock.contents(), builder.contents(), labels, covered, added); }
}

pub(crate) proof fn summary_unique(a: Map<u32, NodeIdx>, b: Map<u32, NodeIdx>, labels: Seq<u32>, covered: ISet<int>)
    requires summarizes(a, labels, covered), summarizes(b, labels, covered),
    ensures a == b,
{
    assert forall|c: u32| a.contains_key(c) == b.contains_key(c) by {
        if a.contains_key(c) { assert(covered.contains(a[c]@ as int)); }
        if b.contains_key(c) { assert(covered.contains(b[c]@ as int)); }
    }
    assert forall|c: u32| a.contains_key(c) implies a[c] == b[c] by {
        assert(covered.contains(a[c]@ as int));
        assert(covered.contains(b[c]@ as int));
        NodeIdx::same_index(a[c], b[c]);
    }
    assert(a =~= b);
}

/// A pop removes one occurrence, whereas the semantic walk removes a set member.
/// Extra duplicate occurrences are harmless once their node is covered.
proof fn retarget(g: graph::Graph, heads: ISet<int>, covered: ISet<int>, pending: ISet<int>, next: ISet<int>)
    requires walk::walk_invariant(g, heads, covered, pending),
        forall|n: int| next.contains(n) ==> covered.contains(n) || pending.contains(n),
        forall|n: int| pending.contains(n) ==> covered.contains(n) || next.contains(n),
    ensures walk::walk_invariant(g, heads, covered, next),
{}

proof fn pop_set(stack: Seq<NodeIdx>)
    requires stack.len() > 0,
    ensures parent_set(stack).contains(stack.last()@ as int),
        forall|n: int| parent_set(stack).contains(n) <==>
            parent_set(stack.drop_last()).contains(n) || n == stack.last()@,
{
    assert forall|n: int| parent_set(stack).contains(n) <==>
        parent_set(stack.drop_last()).contains(n) || n == stack.last()@ by {
        if parent_set(stack).contains(n) {
            let j = choose|j: int| 0 <= j < stack.len() && stack[j]@ == n;
            if j < stack.len() - 1 { assert(stack.drop_last()[j]@ == n); }
        }
        if parent_set(stack.drop_last()).contains(n) {
            let j = choose|j: int| 0 <= j < stack.drop_last().len() && stack.drop_last()[j]@ == n;
            assert(stack[j]@ == n);
        }
    }
}

fn push_parents(stack: &mut Vec<NodeIdx>, parents: &[NodeIdx])
    ensures final(stack)@ == old(stack)@ + parents@,
{
    let ghost before = stack@;
    let mut i = 0;
    while i < parents.len()
        invariant i <= parents.len(), stack@ == before + parents@.subrange(0, i as int),
        decreases parents.len() - i,
    {
        stack.push(parents[i]);
        i += 1;
        assert(stack@ =~= before + parents@.subrange(0, i as int));
    }
}

proof fn concatenated_sets(a: Seq<NodeIdx>, b: Seq<NodeIdx>)
    ensures parent_set(a + b) == parent_set(a).union(parent_set(b)),
{
    assert forall|n: int| parent_set(a + b).contains(n) == parent_set(a).union(parent_set(b)).contains(n) by {
        if parent_set(a + b).contains(n) {
            let j = choose|j: int| 0 <= j < (a + b).len() && (a + b)[j]@ == n;
            if j < a.len() { assert(a[j]@ == n); } else { assert(b[j - a.len()]@ == n); }
        }
        if parent_set(a).contains(n) {
            let j = choose|j: int| 0 <= j < a.len() && a[j]@ == n;
            assert((a + b)[j]@ == n);
        }
        if parent_set(b).contains(n) {
            let j = choose|j: int| 0 <= j < b.len() && b[j]@ == n;
            assert((a + b)[a.len() + j]@ == n);
        }
    }
    assert(parent_set(a + b) =~= parent_set(a).union(parent_set(b)));
}

#[verifier::rlimit(30)]
pub(crate) fn compute<'a, H, P, C, F: Fn(NodeIdx) -> Option<&'a Clock>>(
    nodes: &[Node<H, P, C>], ids: &[NodeIdx], head: NodeIdx, cached: F,
) -> (out: Clock)
    requires storage::linked(nodes@, ids@), head@ < nodes.len(),
        nodes.len() <= u32::MAX as nat + 1,
        storage::labels(nodes@).to_set().len() <= u32::MAX,
        forall|n: NodeIdx| n@ < nodes.len() && n@ != head@ ==> cached.requires((n,)),
        forall|n: NodeIdx, result: Option<&'a Clock>| n@ < nodes.len() && n@ != head@
            && cached.ensures((n,), result) ==> match result {
                None => true,
                Some(clock) => clock.valid() && summarizes(clock.contents(), storage::labels(nodes@),
                    graph::history(graph_of(nodes@, ids@), ISet::empty().insert(n@ as int))),
            },
    ensures out.valid(), summarizes(out.contents(), storage::labels(nodes@),
        graph::history(graph_of(nodes@, ids@), ISet::empty().insert(head@ as int))),
{
    let ghost g = graph_of(nodes@, ids@);
    let ghost labels = storage::labels(nodes@);
    let ghost heads = ISet::empty().insert(head@ as int);
    let ghost universe = Seq::new(nodes.len() as nat, |i: int| i as u32).to_set();
    let ghost mut covered = ISet::<int>::empty();
    let mut builder = ClockBuilder::default();
    let mut stack = vec![head];
    let mut visited = HashSet::<u32>::new();
    proof {
        crate::storage_refinement::stored_graph_is_dag(nodes@, ids@);
        walk::start(g, heads);
        assert(parent_set(stack@) =~= heads);
    }
    while stack.len() > 0
        invariant storage::linked(nodes@, ids@), head@ < nodes.len(),
            nodes.len() <= u32::MAX as nat + 1,
            labels == storage::labels(nodes@), g == graph_of(nodes@, ids@),
            heads == ISet::empty().insert(head@ as int),
            universe == Seq::new(nodes.len() as nat, |i: int| i as u32).to_set(),
            labels.to_set().len() <= u32::MAX,
            builder.valid(), builder.size() == builder.contents().dom().len(),
            summarizes(builder.contents(), labels, covered),
            walk::walk_invariant(g, heads, covered, parent_set(stack@)),
            forall|n: u32| visited@.contains(n) ==> covered.contains(n as int),
            forall|n: NodeIdx| n@ < nodes.len() && n@ != head@ ==> cached.requires((n,)),
            forall|n: NodeIdx, result: Option<&'a Clock>| n@ < nodes.len() && n@ != head@
                && cached.ensures((n,), result) ==> match result {
                    None => true,
                    Some(clock) => clock.valid() && summarizes(clock.contents(), labels,
                        graph::history(g, ISet::empty().insert(n@ as int))),
                },
        decreases universe.difference(visited@).len(), stack.len(),
    {
        let ghost before_stack = stack@;
        let ghost pending = parent_set(stack@);
        proof { pop_set(stack@); }
        let idx = stack.pop().unwrap();
        let n = idx.index();
        assert(pending.contains(n as int));
        let ghost prior_visited = visited@;
        if !visited.insert(n as u32) {
            proof {
                assert(visited@ =~= prior_visited);
                walk::revisit(g, heads, covered, pending, n as int);
                retarget(g, heads, covered, pending.remove(n as int), parent_set(stack@));
            }
            continue;
        }
        proof {
            assert(Seq::new(nodes.len() as nat, |i: int| i as u32)[n as int] == n as u32);
            assert(universe.contains(n as u32));
            assert(universe.difference(visited@) =~= universe.difference(prior_visited).remove(n as u32));
        }
        proof { graph::unfold_reaches(g, n as int, n as int); }
        let node = &nodes[n];
        let clock = if n != head.index() { cached(idx) } else { None };
        if let Some(clock) = clock {
            let ghost added = graph::history(g, ISet::empty().insert(n as int));
            proof {
                assert(ISet::empty().insert(n as int).contains(n as int));
                assert(added.contains(n as int));
            }
            merge_cached(&mut builder, clock, #[cfg(verus_keep_ghost)] { Ghost(labels) }, #[cfg(verus_keep_ghost)] { Ghost(covered) }, #[cfg(verus_keep_ghost)] { Ghost(added) });
            proof {
                walk::use_cache(g, heads, covered, pending, n as int, added);
                covered = covered.union(added);
                retarget(g, heads, covered, pending.remove(n as int), parent_set(stack@));
            }
        } else {
            include_node(&mut builder, node.chain, idx, #[cfg(verus_keep_ghost)] { Ghost(labels) }, #[cfg(verus_keep_ghost)] { Ghost(covered) });
            let parents = storage::parent_slice(node, ids);
            let ghost popped = stack@;
            push_parents(&mut stack, parents);
            proof {
                assert(stack@ == popped + parents@);
                crate::storage_refinement::stored_parent_set(nodes@, ids@, n as int);
                assert(parent_set(parents@) == g[n as int]);
                concatenated_sets(popped, parents@);
                walk::expand(g, heads, covered, pending, n as int);
                covered = covered.insert(n as int);
                retarget(g, heads, covered, pending.remove(n as int).union(g[n as int]), parent_set(stack@));
            }
        }
    }
    proof {
        assert(parent_set(stack@) =~= ISet::<int>::empty());
        walk::finish(g, heads, covered);
    }
    builder.finish()
}

pub(crate) proof fn histories_union(g: graph::Graph, a: ISet<int>, b: ISet<int>)
    ensures graph::history(g, a.union(b)) == graph::history(g, a).union(graph::history(g, b)),
{
    assert forall|n: int| graph::history(g, a.union(b)).contains(n)
        == graph::history(g, a).union(graph::history(g, b)).contains(n) by {
        if graph::history(g, a.union(b)).contains(n) {
            let h = choose|h: int| a.union(b).contains(h) && graph::reaches(g, n, h);
            if a.contains(h) { assert(graph::history(g, a).contains(n)); }
            else { assert(graph::history(g, b).contains(n)); }
        }
        if graph::history(g, a).contains(n) {
            let h = choose|h: int| a.contains(h) && graph::reaches(g, n, h);
            assert(a.union(b).contains(h));
        }
        if graph::history(g, b).contains(n) {
            let h = choose|h: int| b.contains(h) && graph::reaches(g, n, h);
            assert(a.union(b).contains(h));
        }
    }
    assert(graph::history(g, a.union(b)) =~= graph::history(g, a).union(graph::history(g, b)));
}

/// Combine exact head clocks, preserving the single-head Arc-sharing fast path.
pub(crate) fn combine<'a, H, P, C, F: Fn(NodeIdx) -> &'a Arc<Clock>>(
    _nodes: &[Node<H, P, C>], _ids: &[NodeIdx], heads: &[NodeIdx], clock: F,
) -> (out: Arc<Clock>)
    requires storage::linked(_nodes@, _ids@),
        storage::labels(_nodes@).to_set().len() <= u32::MAX,
        forall|i: int| 0 <= i < heads.len() ==> heads[i]@ < _nodes.len(),
        forall|n: NodeIdx| heads@.contains(n) ==> clock.requires((n,)),
        forall|n: NodeIdx, result: &'a Arc<Clock>| #![trigger clock.ensures((n,), result)] heads@.contains(n) && clock.ensures((n,), result)
            ==> result.valid() && summarizes(result.contents(), storage::labels(_nodes@),
                graph::history(graph_of(_nodes@, _ids@), ISet::empty().insert(n@ as int))),
    ensures out.valid(), summarizes(out.contents(), storage::labels(_nodes@),
        graph::history(graph_of(_nodes@, _ids@), parent_set(heads@))),
{
    let ghost g = graph_of(_nodes@, _ids@);
    let ghost labels = storage::labels(_nodes@);
    if heads.len() == 1 {
        assert(parent_set(heads@) =~= ISet::empty().insert(heads@[0]@ as int));
        let head = heads[0];
        assert(heads@.contains(head));
        let result = clock(head);
        assert(clock.ensures((head,), result));
        return Arc::clone(result);
    }
    let mut builder = ClockBuilder::default();
    let mut i = 0;
    while i < heads.len()
        invariant i <= heads.len(),
            labels == storage::labels(_nodes@), g == graph_of(_nodes@, _ids@),
            labels.to_set().len() <= u32::MAX,
            builder.valid(), summarizes(builder.contents(), labels,
                graph::history(g, parent_set(heads@.subrange(0, i as int)))),
            forall|j: int| 0 <= j < heads.len() ==> heads[j]@ < _nodes.len(),
            forall|n: NodeIdx| heads@.contains(n) ==> clock.requires((n,)),
            forall|n: NodeIdx, result: &'a Arc<Clock>| #![trigger clock.ensures((n,), result)] heads@.contains(n) && clock.ensures((n,), result)
                ==> result.valid() && summarizes(result.contents(), storage::labels(_nodes@),
                    graph::history(graph_of(_nodes@, _ids@), ISet::empty().insert(n@ as int))),
        decreases heads.len() - i,
    {
        let head = heads[i];
        assert(heads@.contains(head));
        let next = clock(head);
        assert(clock.ensures((head,), next));
        assert(next.valid());
        let ghost covered = graph::history(g, parent_set(heads@.subrange(0, i as int)));
        let ghost added = graph::history(g, ISet::empty().insert(head@ as int));
        merge_cached(&mut builder, next, #[cfg(verus_keep_ghost)] { Ghost(labels) }, #[cfg(verus_keep_ghost)] { Ghost(covered) }, #[cfg(verus_keep_ghost)] { Ghost(added) });
        proof {
            let prefix = heads@.subrange(0, i as int);
            let after = heads@.subrange(0, i + 1);
            assert(after =~= prefix.push(head));
            pop_set(after);
            assert(parent_set(after) =~= parent_set(prefix).insert(head@ as int));
            assert(parent_set(after) =~= parent_set(prefix).union(ISet::empty().insert(head@ as int)));
            histories_union(g, parent_set(prefix), ISet::empty().insert(head@ as int));
            assert(graph::history(g, parent_set(after)) =~= covered.union(added));
        }
        i += 1;
    }
    assert(heads@.subrange(0, i as int) =~= heads@);
    Arc::new(builder.finish())
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn cached_walk_and_overlapping_heads_match_cold_walk_after_append() {
        let (mut nodes, mut ids, mut tips, mut tables) = storage::empty::<u64, u64, ()>();
        let mut add = |hash, chain, parents: &[NodeIdx]| {
            storage::append(
                &mut nodes,
                &mut ids,
                &mut tips,
                &mut tables,
                parents,
                Node::new(hash, hash, chain, 0, 1, ()),
            )
        };
        let root = add(10, 0, &[]);
        let left = add(20, 0, &[root, root]);
        let right = add(30, 1, &[root]);
        let merge = add(40, 0, &[left, right, left]);
        let unrelated = add(50, 2, &[]);
        let right_clock = compute(&nodes, &ids, right, |_| None);
        let hits = Cell::new(0);
        let cached = Arc::new(compute(&nodes, &ids, merge, |n| {
            assert_ne!(n, merge, "the initializer must not read its own cache");
            if n == right {
                hits.set(hits.get() + 1);
                Some(&right_clock)
            } else {
                None
            }
        }));
        let cold = compute(&nodes, &ids, merge, |_| None);
        assert_eq!(*cached, cold);
        assert_eq!(hits.get(), 1);
        assert!(cached.contains(0, root));
        assert!(cached.contains(1, right));
        assert!(!cached.contains(2, unrelated));
        let other = Arc::new(compute(&nodes, &ids, unrelated, |_| None));
        let get = |n| if n == merge { &cached } else { &other };
        let singleton = combine(&nodes, &ids, &[merge], get);
        assert!(Arc::ptr_eq(&singleton, &cached));
        let empty = combine(&nodes, &ids, &[], |_| {
            panic!("empty heads must not query a cache")
        });
        assert_eq!(empty.len(), 0);
        let combined = combine(&nodes, &ids, &[merge, unrelated, merge], get);
        let last = storage::append(
            &mut nodes,
            &mut ids,
            &mut tips,
            &mut tables,
            &[merge, unrelated, merge],
            Node::new(60, 60, 0, 0, 1, ()),
        );
        assert_eq!(*cached, compute(&nodes, &ids, merge, |_| None));
        let after = compute(&nodes, &ids, last, |n| {
            if n == merge {
                Some(cached.as_ref())
            } else {
                None
            }
        });
        assert_eq!(after, compute(&nodes, &ids, last, |_| None));
        assert!(after.contains(0, last));
        assert!(after.contains(2, unrelated));
        assert!(combined.contains(0, merge));
        assert!(combined.contains(2, unrelated));
        assert!(!combined.contains(0, last));
    }
}
