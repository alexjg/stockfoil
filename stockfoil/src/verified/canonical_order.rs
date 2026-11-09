//! Pure canonical-order model for the public iterator's standard Iterator contract.
#[cfg(verus_keep_ghost)]
use super::{
    clock::NodeIdx,
    commit_hash::CommitHash,
    stdlib::{hash_less, ready_le},
    topology::Topology,
};
#[cfg(verus_keep_ghost)]
use crate::{graph, scheduler_refinement::eligible};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

proof fn hash_total_at(a: CommitHash, b: CommitHash, k: int)
    requires 0 <= k <= 32, forall|i: int| 0 <= i < k ==> a.0@[i] == b.0@[i],
    ensures a == b || hash_less(a, b) || hash_less(b, a),
    decreases 32 - k,
{
    if k == 32 { assert(a.0@ =~= b.0@); assert(a.0 == b.0); }
    else if a.0@[k] < b.0@[k] { assert(hash_less(a, b)); }
    else if b.0@[k] < a.0@[k] { assert(hash_less(b, a)); }
    else { hash_total_at(a, b, k + 1); }
}

pub(crate) proof fn hash_asymmetric(a: CommitHash, b: CommitHash)
    requires hash_less(a, b),
    ensures !hash_less(b, a), a != b,
{
    let i = choose|i: int| 0 <= i < 32 && a.0@[i] < b.0@[i] && forall|j: int| 0 <= j < i ==> a.0@[j] == b.0@[j];
    if hash_less(b, a) {
        let k = choose|k: int| 0 <= k < 32 && b.0@[k] < a.0@[k] && forall|j: int| 0 <= j < k ==> b.0@[j] == a.0@[j];
        if i < k { assert(a.0@[i] == b.0@[i]); }
        else if k < i { assert(a.0@[k] == b.0@[k]); }
    }
}

proof fn hash_transitive(a: CommitHash, b: CommitHash, c: CommitHash)
    requires hash_less(a, b), hash_less(b, c),
    ensures hash_less(a, c),
{
    let i = choose|i: int| 0 <= i < 32 && a.0@[i] < b.0@[i] && forall|j: int| 0 <= j < i ==> a.0@[j] == b.0@[j];
    let k = choose|k: int| 0 <= k < 32 && b.0@[k] < c.0@[k] && forall|j: int| 0 <= j < k ==> b.0@[j] == c.0@[j];
    let m = if i < k { i } else { k };
    assert(a.0@[m] < c.0@[m]);
    assert forall|j: int| 0 <= j < m implies a.0@[j] == c.0@[j] by {};
    assert(hash_less(a, c));
}

proof fn key_total(a: (CommitHash, NodeIdx), b: (CommitHash, NodeIdx))
    ensures ready_le(a, b) || ready_le(b, a),
{ hash_total_at(a.0, b.0, 0); }

proof fn key_transitive(a: (CommitHash, NodeIdx), b: (CommitHash, NodeIdx), c: (CommitHash, NodeIdx))
    requires ready_le(a, b), ready_le(b, c),
    ensures ready_le(a, c),
{ if hash_less(a.0, b.0) && hash_less(b.0, c.0) { hash_transitive(a.0, b.0, c.0); } }

proof fn key_antisymmetric(a: (CommitHash, NodeIdx), b: (CommitHash, NodeIdx))
    requires ready_le(a, b), ready_le(b, a),
    ensures a == b,
{
    if hash_less(a.0, b.0) { hash_asymmetric(a.0, b.0); }
    if hash_less(b.0, a.0) { hash_asymmetric(b.0, a.0); }
    NodeIdx::same_index(a.1, b.1);
}

pub(crate) open spec fn bounded<P>(t: &Topology<P>, s: Set<NodeIdx>) -> bool {
    forall|n: NodeIdx| s.contains(n) ==> n@ < t.nodes_view().len()
}

pub(crate) open spec fn key<P>(t: &Topology<P>, n: NodeIdx) -> (CommitHash, NodeIdx) { (t.nodes_view()[n@ as int].hash, n) }

proof fn minimum_key<P>(t: &Topology<P>, s: Set<NodeIdx>) -> (out: NodeIdx)
    requires !s.is_empty(), bounded(t, s),
    ensures s.contains(out), forall|n: NodeIdx| s.contains(n) ==> ready_le(key(t, out), key(t, n)),
    decreases s.len(),
{
    let n = s.choose();
    if s.remove(n).is_empty() { n }
    else {
        let other = minimum_key(t, s.remove(n));
        key_total(key(t, n), key(t, other));
        if ready_le(key(t, n), key(t, other)) {
            assert forall|a: NodeIdx| s.contains(a) implies ready_le(key(t, n), key(t, a)) by {
                if a != n { key_transitive(key(t, n), key(t, other), key(t, a)); }
            }
            n
        } else { other }
    }
}

pub(crate) open spec fn first_ok<P>(t: &Topology<P>, s: Set<NodeIdx>, n: NodeIdx) -> bool {
    eligible(t.graph(), s, n) && forall|a: NodeIdx| eligible(t.graph(), s, a) ==> ready_le(key(t, n), key(t, a))
}

pub(crate) open spec fn first<P>(t: &Topology<P>, s: Set<NodeIdx>) -> NodeIdx { choose|n: NodeIdx| first_ok(t, s, n) }

proof fn first_exists<P>(t: &Topology<P>, s: Set<NodeIdx>)
    requires bounded(t, s), !s.is_empty(),
    ensures first_ok(t, s, first(t, s)),
{
    let lowest = crate::scheduler_refinement::minimum(s);
    assert forall|a: NodeIdx| s.contains(a) && a != lowest implies !graph::reaches(t.graph(), a@ as int, lowest@ as int) by {
        if graph::reaches(t.graph(), a@ as int, lowest@ as int) {
            graph::reaches_order(t.graph(), a@ as int, lowest@ as int);
            NodeIdx::same_index(a, lowest);
        }
    }
    assert(eligible(t.graph(), s, lowest));
    let ready = s.filter(|n: NodeIdx| eligible(t.graph(), s, n));
    assert(ready.contains(lowest));
    let best = minimum_key(t, ready);
    assert forall|a: NodeIdx| eligible(t.graph(), s, a) implies ready_le(key(t, best), key(t, a)) by {
        assert(ready.contains(a));
    }
    assert(first_ok(t, s, best));
}

pub(crate) proof fn first_unique<P>(t: &Topology<P>, s: Set<NodeIdx>, n: NodeIdx)
    requires bounded(t, s), first_ok(t, s, n),
    ensures first(t, s) == n,
{
    first_exists(t, s);
    key_antisymmetric(key(t, first(t, s)), key(t, n));
}

pub(crate) open spec fn order<P>(t: &Topology<P>, s: Set<NodeIdx>, fuel: nat) -> Seq<NodeIdx>
    decreases fuel,
{
    if fuel == 0 || s.is_empty() { Seq::empty() }
    else { seq![first(t, s)] + order(t, s.remove(first(t, s)), (fuel - 1) as nat) }
}

pub(crate) proof fn order_facts<P>(t: &Topology<P>, s: Set<NodeIdx>)
    requires bounded(t, s),
    ensures order(t, s, s.len()).len() == s.len(), order(t, s, s.len()).to_set() == s,
        order(t, s, s.len()).no_duplicates(),
        forall|i: int, j: int| 0 <= i < j < order(t, s, s.len()).len() ==>
            !graph::reaches(t.graph(), order(t, s, s.len())[j]@ as int, order(t, s, s.len())[i]@ as int),
    decreases s.len(),
{
    if !s.is_empty() {
        first_exists(t, s);
        let n = first(t, s);
        order_facts(t, s.remove(n));
        let tail = order(t, s.remove(n), s.remove(n).len());
        assert(order(t, s, s.len()) == seq![n] + tail);
        assert forall|a: NodeIdx| (seq![n] + tail).contains(a) == s.contains(a) by {
            if (seq![n] + tail).contains(a) {
                let i = choose|i: int| 0 <= i < 1 + tail.len() && (seq![n] + tail)[i] == a;
                if i > 0 { assert(tail[i - 1] == a); assert(tail.contains(a)); }
            }
            if s.contains(a) {
                if a == n { assert((seq![n] + tail)[0] == a); }
                else {
                    assert(tail.contains(a));
                    let i = choose|i: int| 0 <= i < tail.len() && tail[i] == a;
                    assert((seq![n] + tail)[i + 1] == a);
                }
            }
        }
        assert((seq![n] + tail).to_set() =~= s);
        assert forall|i: int, j: int| 0 <= i < j < 1 + tail.len() implies
            !graph::reaches(t.graph(), (seq![n] + tail)[j]@ as int, (seq![n] + tail)[i]@ as int) by {
            if i == 0 { assert(tail.contains(tail[j - 1])); assert(s.contains(tail[j - 1])); }
        }
        assert((seq![n] + tail).no_duplicates()) by {
            assert forall|i: int, j: int| 0 <= i < j < 1 + tail.len() implies (seq![n] + tail)[i] != (seq![n] + tail)[j] by {
                if i == 0 { assert(tail.contains(tail[j - 1])); }
            }
        }
    }
}

pub(crate) proof fn order_step<P>(t: &Topology<P>, s: Set<NodeIdx>, n: NodeIdx)
    requires bounded(t, s), first_ok(t, s, n),
    ensures order(t, s, s.len()) == seq![n] + order(t, s.remove(n), s.remove(n).len()),
{
    first_unique(t, s, n);
}

}
