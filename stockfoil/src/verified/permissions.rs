//! Compile actual permission maps and frontier hashes against the owned topology.
use super::{
    author::Author, clock::NodeIdx, commit_hash::CommitHash, topology::Topology, tree::Clock,
    write_permission::WritePermission, write_permissions::WritePermissions,
};
#[cfg(verus_keep_ghost)]
use crate::{
    authorization, graph,
    permission_refinement::{self as policy, resolved, selected},
    storage_refinement::parent_set,
};
use std::{collections::HashMap, sync::Arc};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Stored prefix endpoints are actual nodes on the coordinate's chain.
pub(crate) open spec fn coordinates<P>(topology: &Topology<P>, clock: &Clock) -> bool {
    forall|c: u32| clock.contents().contains_key(c) ==> clock.contents()[c]@ < topology.labels().len()
        && topology.labels()[clock.contents()[c]@ as int] == c
}


/// Unknown and out-of-selection frontier heads are discarded *before* ancestry.
pub(crate) fn bounded_clock<P>(topology: &Topology<P>, reached: &Clock, frontier: &[CommitHash]) -> (out: Arc<Clock>)
    requires topology.valid(), reached.valid(),
    ensures out.valid(), coordinates(topology, &out),
        forall|n: usize| n < topology.nodes_view().len() ==>
            super::clock::below(out.lookup(topology.labels()[n as int]), n)
            == authorization::allowed(topology.graph(), selected(topology.labels(), reached),
                authorization::Permission::Bounded(resolved(topology.hashes(), frontier@)), n as int),
{
    proof { topology.permission_facts(); }
    let ghost labels = topology.labels();
    let ghost hashes = topology.hashes();
    let ghost chosen = selected(labels, reached);
    let mut available: Vec<NodeIdx> = Vec::new();
    let mut i = 0;
    while i < frontier.len()
        invariant topology.valid(), reached.valid(), i <= frontier.len(),
            topology.labels() == labels, topology.hashes() == hashes, chosen == selected(labels, reached),
            labels.len() == topology.nodes_view().len(),
            forall|h: CommitHash| hashes.contains_key(h) ==> hashes[h]@ < labels.len(),
            forall|n: int| 0 <= n < labels.len() ==> topology.nodes_view()[n].chain == labels[n],
            forall|j: int| 0 <= j < available.len() ==> available[j]@ < labels.len(),
            parent_set(available@) == resolved(hashes, frontier@.subrange(0, i as int)).intersect(chosen),
        decreases frontier.len() - i,
    {
        let ghost previous = available@;
        if let Some(idx) = topology.by_hash().get(&frontier[i]) {
            let chain = topology.nodes()[idx.index()].chain;
            assert(chosen.contains(idx@ as int) == super::clock::below(reached.lookup(chain), idx@ as usize));
            if reached.contains(chain, *idx) {
                available.push(*idx);
            }
        }
        proof {
            assert forall|n: int| parent_set(available@).contains(n)
                == resolved(hashes, frontier@.subrange(0, i + 1)).intersect(chosen).contains(n) by {
                if resolved(hashes, frontier@.subrange(0, i + 1)).contains(n) && chosen.contains(n) {
                    let j = choose|j: int| 0 <= j < i + 1 && hashes.contains_key(frontier@[j]) && hashes[frontier@[j]]@ == n;
                    if j < i {
                        assert(frontier@.subrange(0, i as int)[j] == frontier@[j]);
                        assert(resolved(hashes, frontier@.subrange(0, i as int)).contains(n));
                        assert(parent_set(previous).contains(n));
                        let k = choose|k: int| 0 <= k < previous.len() && previous[k]@ == n;
                        assert(available@[k] == previous[k]);
                        assert(parent_set(available@).contains(n));
                    } else if chosen.contains(n) {
                        assert(available@.last()@ == n);
                        assert(parent_set(available@).contains(n));
                    }
                }
                if parent_set(available@).contains(n) {
                    let j = choose|j: int| 0 <= j < available.len() && available@[j]@ == n;
                    if j < previous.len() {
                        assert(parent_set(previous).contains(n));
                        let k = choose|k: int| 0 <= k < i && hashes.contains_key(frontier@[k]) && hashes[frontier@[k]]@ == n;
                        assert(frontier@.subrange(0, i + 1)[k] == frontier@[k]);
                    } else { assert(frontier@.subrange(0, i + 1)[i as int] == frontier@[i as int]); }
                    assert(chosen.contains(n));
                    assert(resolved(hashes, frontier@.subrange(0, i + 1)).contains(n));
                }
            }
            assert(parent_set(available@) =~= resolved(hashes, frontier@.subrange(0, i + 1)).intersect(chosen));
        }
        i += 1;
    }
    assert(frontier@.subrange(0, i as int) =~= frontier@);
    let ghost before = available@;
    super::candidates::normalize(&mut available);
    proof {
        assert forall|n: int| parent_set(available@).contains(n) == parent_set(before).contains(n) by {
            if parent_set(available@).contains(n) {
                let j = choose|j: int| 0 <= j < available.len() && available@[j]@ == n;
                assert(before.contains(available@[j]));
            }
            if parent_set(before).contains(n) {
                let j = choose|j: int| 0 <= j < before.len() && before[j]@ == n;
                assert(available@.contains(before[j]));
            }
        }
        assert(parent_set(available@) =~= parent_set(before));
        assert forall|j: int| 0 <= j < available.len() implies available@[j]@ < topology.nodes_view().len() by {
            assert(before.contains(available@[j]));
        }
    }
    let out = topology.clock_for_heads(&available);
    proof {
        assert forall|n: usize| n < topology.nodes_view().len() implies
            super::clock::below(out.lookup(labels[n as int]), n)
            == authorization::allowed(topology.graph(), chosen,
                authorization::Permission::Bounded(resolved(hashes, frontier@)), n as int) by {
            authorization::bounded_policy(topology.graph(), chosen, resolved(hashes, frontier@), n as int);
        }
    }
    out
}

pub(crate) open spec fn bound_valid<P>(topology: &Topology<P>, reached: &Clock,
    permission: WritePermission, clock: &Clock) -> bool
{
    permission is Bounded && clock.valid() && coordinates(topology, clock)
        && (forall|n: usize| n < topology.labels().len() ==>
            super::clock::below(clock.lookup(topology.labels()[n as int]), n)
            == authorization::allowed(topology.graph(), selected(topology.labels(), reached),
                policy::model(topology.hashes(), permission), n as int))
}

/// Explicit authors have separate bounds; all unlisted authors share one bound.
struct Bounds {
    listed: HashMap<Author, Arc<Clock>>,
    unlisted: Option<Arc<Clock>>,
}

impl Bounds {
    spec fn value(&self, permissions: &WritePermissions, author: Author) -> Option<Arc<Clock>> {
        if permissions.authors().contains_key(author) {
            if self.listed@.contains_key(author) { Some(self.listed@[author]) } else { None }
        } else { self.unlisted }
    }

    spec fn ready(&self, permissions: &WritePermissions, author: Author) -> bool {
        permissions.rule(author) is Bounded ==> self.value(permissions, author) is Some
    }

    spec fn valid<P>(&self, topology: &Topology<P>, reached: &Clock, permissions: &WritePermissions) -> bool {
        &&& forall|a: Author| self.listed@.contains_key(a) ==> permissions.authors().contains_key(a)
            && bound_valid(topology, reached, permissions.authors()[a], &*self.listed@[a])
        &&& match self.unlisted {
            Some(clock) => bound_valid(topology, reached, permissions.fallback(), &*clock),
            None => true,
        }
    }

    fn new() -> (out: Self)
        ensures out.listed@ == Map::<Author, Arc<Clock>>::empty(), out.unlisted is None,
    { Self { listed: HashMap::new(), unlisted: None } }

    fn fill<P>(&mut self, topology: &Topology<P>, reached: &Clock, permissions: &WritePermissions, author: Author)
        requires topology.valid(), reached.valid(), old(self).valid(topology, reached, permissions),
        ensures final(self).valid(topology, reached, permissions), final(self).ready(permissions, author),
            forall|a: Author| old(self).ready(permissions, a) ==> final(self).ready(permissions, a),
    {
        proof { super::stdlib::author_key_model(); topology.permission_facts(); }
        match permissions.listed(&author) {
            Some(WritePermission::Bounded(frontier)) => {
                if !self.listed.contains_key(&author) {
                    let clock = bounded_clock(topology, reached, frontier);
                    assert(bound_valid(topology, reached, permissions.rule(author), &clock));
                    self.listed.insert(author, clock);
                }
            },
            None => {
                if let WritePermission::Bounded(frontier) = permissions.unlisted() {
                    if self.unlisted.is_none() {
                        let clock = bounded_clock(topology, reached, frontier);
                        assert(bound_valid(topology, reached, permissions.fallback(), &clock));
                        self.unlisted = Some(clock);
                    }
                }
            },
            _ => {},
        }
    }
}

fn compile_bounds<P>(topology: &Topology<P>, reached: &Clock, permissions: &WritePermissions) -> (bounded: Bounds)
    requires topology.valid(), reached.valid(),
        forall|c: u32| reached.contents().contains_key(c) ==> c < topology.owners_view().len(),
    ensures bounded.valid(topology, reached, permissions),
        forall|c: u32| reached.contents().contains_key(c) ==> bounded.ready(permissions, topology.owners_view()[c as int]),
{
    proof { reached.sequence_correct(); }
    let mut bounded = Bounds::new();
    let mut iter = reached.entries();
    let ghost mut done: nat = 0;
    loop
        invariant topology.valid(), reached.valid(), done <= reached.sequence().len(),
            iter.remaining() == reached.sequence().skip(done as int),
            forall|c: u32| reached.contents().contains_key(c) ==> c < topology.owners_view().len(),
            bounded.valid(topology, reached, permissions),
            forall|i: int| 0 <= i < done ==> bounded.ready(permissions, topology.owners_view()[reached.sequence()[i].0 as int]),
        ensures bounded.valid(topology, reached, permissions),
            forall|i: int| 0 <= i < reached.sequence().len() ==> bounded.ready(permissions, topology.owners_view()[reached.sequence()[i].0 as int]),
        decreases iter.remaining().len(),
    {
        match iter.next() {
            None => break,
            Some((chain, _tip)) => {
                proof { reached.sequence_correct(); assert(reached.sequence()[done as int].0 == chain); }
                let author = topology.chain_authors()[chain as usize];
                bounded.fill(topology, reached, permissions, author);
                proof { done = done + 1; }
            },
        }
    }
    proof {
        reached.sequence_correct();
        assert forall|c: u32| reached.contents().contains_key(c) implies bounded.ready(permissions, topology.owners_view()[c as int]) by {
            let i = choose|i: int| 0 <= i < reached.sequence().len() && reached.sequence()[i].0 == c;
        }
    }
    bounded
}

spec fn coordinate_bound(owners: Seq<Author>, permissions: &WritePermissions,
    bounded: &Bounds, chain: u32, tip: NodeIdx) -> Option<NodeIdx>
{
    match permissions.rule(owners[chain as int]) {
        WritePermission::Unbounded => Some(tip),
        WritePermission::Bounded(_) => bounded.value(permissions, owners[chain as int]).unwrap().lookup(chain),
        WritePermission::Denied => None,
    }
}

/// Apply explicit rules or the unlisted fallback to an already selected history.
pub(crate) fn participation_in<P>(topology: &Topology<P>, reached: &Clock,
    permissions: &WritePermissions) -> (out: Clock)
    requires topology.valid(), reached.valid(),
        forall|c: u32| reached.contents().contains_key(c) ==> c < topology.owners_view().len(),
    ensures out.valid(), out.size() <= reached.size(),
        coordinates(topology, reached) ==> coordinates(topology, &out),
        out.contents().dom().subset_of(reached.contents().dom()),
        forall|c: u32| out.contents().contains_key(c) ==> out.contents()[c]@ <= reached.contents()[c]@,
        forall|n: usize| n < topology.nodes_view().len() ==>
            super::clock::below(out.lookup(topology.labels()[n as int]), n)
            == policy::visible(topology.graph(), topology.labels(), topology.owners_view(),
                topology.hashes(), reached, permissions, n as int),
{
    proof { super::stdlib::author_key_model(); topology.permission_facts(); }
    let bounded = compile_bounds(topology, reached, permissions);
    let owners = topology.chain_authors();
    let bound = |chain: u32, tip: NodeIdx| -> (limit: Option<NodeIdx>)
        requires reached.contents().contains_key(chain),
        ensures limit == coordinate_bound(owners@, permissions, &bounded, chain, tip),
    {
        let author = owners[chain as usize];
        match permissions.permission(&author) {
            WritePermission::Unbounded => Some(tip),
            WritePermission::Bounded(_) => {
                let clock = if permissions.listed(&author).is_some() {
                    bounded.listed.get(&author).unwrap()
                } else {
                    bounded.unlisted.as_ref().unwrap()
                };
                clock.position(chain)
            },
            WritePermission::Denied => None,
        }
    };
    let out = reached.mask(bound);
    proof {
        if coordinates(topology, reached) {
            assert forall|c: u32| out.contents().contains_key(c) implies
                out.contents()[c]@ < topology.labels().len() && topology.labels()[out.contents()[c]@ as int] == c by {
                let tip = reached.contents()[c];
                assert(Clock::callback_bound(bound, c, tip) == coordinate_bound(owners@, permissions, &bounded, c, tip));
                if permissions.rule(owners@[c as int]) is Bounded {
                    assert(bound_valid(topology, reached, permissions.rule(owners@[c as int]), &*bounded.value(permissions, owners@[c as int]).unwrap()));
                }
            }
        }

        assert forall|n: usize| n < topology.nodes_view().len() implies
            super::clock::below(out.lookup(topology.labels()[n as int]), n)
            == policy::visible(topology.graph(), topology.labels(), topology.owners_view(),
                topology.hashes(), reached, permissions, n as int) by {
            let c = topology.labels()[n as int];
            let a = owners@[c as int];
            if reached.contents().contains_key(c) {
                let tip = reached.contents()[c];
                assert(Clock::callback_bound(bound, c, tip) == coordinate_bound(owners@, permissions, &bounded, c, tip));
                if permissions.rule(a) is Bounded {
                    assert(bounded.value(permissions, a) is Some);
                    assert(bound_valid(topology, reached, permissions.rule(a), &*bounded.value(permissions, a).unwrap()));
                }
            }
        }
    }
    out
}

/// Resolve selected heads and compile permissions with no caller-supplied clock premises.
pub(crate) fn capture_clocks<P>(topology: &Topology<P>, heads: &[CommitHash],
    permissions: &WritePermissions) -> (out: Result<(Arc<Clock>, Clock), CommitHash>)
    requires topology.valid(),
    ensures match out {
        Err(h) => heads@.contains(h) && !topology.hashes().contains_key(h),
        Ok((reached, participating)) => reached.valid() && participating.valid()
            && coordinates(topology, &reached) && coordinates(topology, &participating)
            && (forall|i: int| 0 <= i < heads.len() ==> topology.hashes().contains_key(heads[i]))
            && crate::ancestry_walk::summarizes(reached.contents(), topology.labels(), graph::history(topology.graph(),
                parent_set(heads@.map(|i: int, h: CommitHash| topology.hashes()[h]))))
            && participating.contents().dom().subset_of(reached.contents().dom())
            && (forall|c: u32| participating.contents().contains_key(c) ==>
                participating.contents()[c]@ <= reached.contents()[c]@)
            && (forall|n: usize| n < topology.nodes_view().len() ==>
                super::clock::below(reached.lookup(topology.labels()[n as int]), n)
                == graph::history(topology.graph(), parent_set(heads@.map(|i: int, h: CommitHash| topology.hashes()[h]))).contains(n as int))
            && (forall|n: usize| n < topology.nodes_view().len() ==>
                super::clock::below(participating.lookup(topology.labels()[n as int]), n)
                == policy::visible_in(topology.graph(), topology.labels(), topology.owners_view(), topology.hashes(),
                    graph::history(topology.graph(), parent_set(heads@.map(|i: int, h: CommitHash| topology.hashes()[h]))),
                    permissions, n as int)),
    },
{
    let reached = match topology.history_clock(heads) {
        Ok(clock) => clock,
        Err(h) => return Err(h),
    };
    proof {
        topology.permission_facts();
        assert forall|c: u32| reached.contents().contains_key(c) implies c < topology.owners_view().len() by {
            let n = reached.contents()[c]@ as int;
            assert(topology.labels()[n] == c);
        }
    }
    let participating = participation_in(topology, &reached, permissions);
    proof {
        let h = graph::history(topology.graph(), parent_set(heads@.map(|i: int, h: CommitHash| topology.hashes()[h])));
        assert forall|n: int| selected(topology.labels(), &reached).contains(n) == h.contains(n) by {
            if 0 <= n < topology.labels().len() {
                assert(super::clock::below(reached.lookup(topology.nodes_view()[n].chain), n as usize) == h.contains(n));
            }
        }
        assert(selected(topology.labels(), &reached) =~= h);
        assert forall|n: usize| n < topology.nodes_view().len() implies
            super::clock::below(participating.lookup(topology.labels()[n as int]), n)
            == policy::visible_in(topology.graph(), topology.labels(), topology.owners_view(), topology.hashes(), h,
                permissions, n as int) by {
            assert(policy::visible(topology.graph(), topology.labels(), topology.owners_view(), topology.hashes(),
                &reached, permissions, n as int)
                == policy::visible_in(topology.graph(), topology.labels(), topology.owners_view(), topology.hashes(), h,
                    permissions, n as int));
        }
    }
    Ok((reached, participating))
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlisted_bounds_are_shared_and_explicit_rules_take_precedence() {
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let h0 = CommitHash([0; 32]);
        let h1 = CommitHash([1; 32]);
        let unknown = CommitHash([255; 32]);
        let mut t = Topology::new();
        let root = t.insert(h0, alice, &[], (), 0).unwrap();
        let child = t.insert(h1, bob, &[h0], (), 0).unwrap();
        let reached = t.history_clock(&[h1]).unwrap();
        let mut permissions =
            WritePermissions::new().with_unlisted(WritePermission::Bounded(vec![h1, unknown, h1]));
        let cache = compile_bounds(&t, &reached, &permissions);
        assert!(cache.listed.is_empty());
        assert!(cache.unlisted.is_some());
        let visible = |heads: &[CommitHash], p: &WritePermissions| {
            let (_, clock) = capture_clocks(&t, heads, p).unwrap();
            [root, child].map(|idx| clock.contains(t.nodes()[idx.index()].chain, idx))
        };
        assert_eq!(visible(&[h1], &permissions), [true, true]);
        assert_eq!(visible(&[h0], &permissions), [false, false]);
        permissions.insert(alice, WritePermission::Denied);
        assert_eq!(visible(&[h1], &permissions), [false, true]);
        permissions.remove(&alice);
        assert_eq!(visible(&[h1], &permissions), [true, true]);
        permissions.set_unlisted(WritePermission::Unbounded);
        permissions.insert(alice, WritePermission::Bounded(vec![]));
        assert_eq!(visible(&[h1], &permissions), [false, true]);
        permissions.set_unlisted(WritePermission::Denied);
        permissions.insert(alice, WritePermission::Unbounded);
        assert_eq!(visible(&[h1], &permissions), [true, false]);
    }

    #[test]
    fn actual_permission_maps_filter_heads_before_ancestry() {
        let alice = Author([1; 32]);
        let bob = Author([2; 32]);
        let hashes = [0, 1, 2, 3].map(|n| CommitHash([n; 32]));
        let unknown = CommitHash([255; 32]);
        let mut t = Topology::new();
        let root = t.insert(hashes[0], alice, &[], (), 0).unwrap();
        let separate = t.insert(hashes[1], alice, &[], (), 0).unwrap();
        let bound = t.insert(hashes[2], bob, &[hashes[0]], (), 0).unwrap();
        let later = t.insert(hashes[3], alice, &[hashes[2]], (), 0).unwrap();
        assert_ne!(
            t.nodes()[root.index()].chain,
            t.nodes()[separate.index()].chain
        );
        let heads = [hashes[2], hashes[1], hashes[2]];
        let cases = [
            (None, None, [false, false, false, false]),
            (
                Some(WritePermission::Denied),
                Some(WritePermission::Unbounded),
                [false, false, true, false],
            ),
            (
                Some(WritePermission::Unbounded),
                None,
                [true, true, false, false],
            ),
            (
                Some(WritePermission::Bounded(vec![])),
                None,
                [false, false, false, false],
            ),
            (
                Some(WritePermission::Bounded(vec![hashes[3], unknown])),
                None,
                [false, false, false, false],
            ),
            (
                Some(WritePermission::Bounded(vec![
                    hashes[2], unknown, hashes[2],
                ])),
                None,
                [true, false, false, false],
            ),
            (
                Some(WritePermission::Bounded(vec![hashes[1], hashes[2]])),
                Some(WritePermission::Bounded(vec![hashes[2]])),
                [true, true, true, false],
            ),
        ];
        for (a, b, expected) in cases {
            let mut permissions = WritePermissions::new();
            if let Some(a) = a {
                permissions.insert(alice, a);
            }
            if let Some(b) = b {
                permissions.insert(bob, b);
            }
            let (reached, visible) = capture_clocks(&t, &heads, &permissions).unwrap();
            for (i, idx) in [root, separate, bound, later].into_iter().enumerate() {
                let chain = t.nodes()[idx.index()].chain;
                assert_eq!(reached.contains(chain, idx), i < 3);
                assert_eq!(
                    visible.contains(chain, idx),
                    expected[i],
                    "node {i}, permissions {permissions:?}"
                );
            }
            let (empty, hidden) = capture_clocks(&t, &[], &permissions).unwrap();
            for idx in [root, separate, bound, later] {
                let chain = t.nodes()[idx.index()].chain;
                assert!(!empty.contains(chain, idx));
                assert!(!hidden.contains(chain, idx));
            }
            assert_eq!(
                capture_clocks(&t, &[hashes[0], unknown], &permissions).unwrap_err(),
                unknown
            );
        }
    }
}
