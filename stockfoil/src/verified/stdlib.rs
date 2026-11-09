//! Audited standard-library contracts. See verification/TRUSTED_STDLIB.md.
#[cfg(verus_keep_ghost)]
use super::clock;
use super::node_key::NodeIdx;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

// The same body is checked inside a ghost atomic invariant and erased by Cargo.
#[cfg(not(verus_keep_ghost))]
macro_rules! open_atomic_invariant {
    ($invariant:expr => $state:ident => $body:tt) => {
        verus_builtin_macros::verus_exec_expr!($body)
    };
}

#[cfg(not(verus_keep_ghost))]
pub(crate) use open_atomic_invariant;

verus! {

// TRUST EARLIER-TYPE-1: opaque, immutable std BTreeMap range iterator.
#[verifier::external_body]
#[verifier::reject_recursive_types(V)]
pub(crate) struct Earlier<'a, V> {
    entries: std::iter::Peekable<std::collections::btree_map::Range<'a, NodeIdx, V>>,
}

impl<'a, V> Earlier<'a, V> {
    pub uninterp spec fn remaining(&self) -> Seq<(NodeIdx, V)>;

    // TRUST EARLIER-NEW-1: range(..limit), in the pinned key's derived u32 order.
    #[verifier::external_body]
    pub(crate) fn new(map: &'a std::collections::BTreeMap<NodeIdx, V>, limit: NodeIdx) -> (out: (Self, bool))
        ensures out.1 == (out.0.remaining().len() > 0),
            forall|i: int| #![trigger out.0.remaining()[i]] 0 <= i < out.0.remaining().len() ==> {
                let (k, v) = out.0.remaining()[i];
                k@ < limit@ && map@.contains_key(k) && map@[k] == v
            },
            forall|k: NodeIdx| map@.contains_key(k) && k@ < limit@ ==>
                out.0.remaining().contains((k, map@[k])),
            forall|i: int, j: int| 0 <= i < j < out.0.remaining().len() ==>
                out.0.remaining()[i].0@ < out.0.remaining()[j].0@,
    {
        let mut entries = map.range(..limit).peekable();
        let nonempty = entries.peek().is_some();
        (Self { entries }, nonempty)
    }

    // TRUST EARLIER-NEXT-1: Iterator::next removes exactly the next borrowed entry.
    #[verifier::external_body]
    pub(crate) fn next(&mut self) -> (out: Option<(&'a NodeIdx, &'a V)>)
        ensures match out {
            None => old(self).remaining().len() == 0 && final(self).remaining() == old(self).remaining(),
            Some((k, v)) => old(self).remaining().len() > 0
                && (*k, *v) == old(self).remaining()[0]
                && final(self).remaining() == old(self).remaining().skip(1),
        },
    { self.entries.next() }
}

/// Lexicographic byte order used by CommitHash's pinned derived Ord implementation.
pub(crate) open spec fn hash_less(a: super::commit_hash::CommitHash, b: super::commit_hash::CommitHash) -> bool {
    exists|k: int| 0 <= k < 32 && a.0@[k] < b.0@[k]
        && forall|j: int| 0 <= j < k ==> a.0@[j] == b.0@[j]
}

pub(crate) open spec fn hashes_sorted(hashes: Seq<super::commit_hash::CommitHash>) -> bool {
    forall|i: int, j: int| 0 <= i < j < hashes.len() ==> hash_less(hashes[i], hashes[j])
}

// TRUST HASH-NORMALIZE-1: std slice sort and Vec dedup on the pinned byte key.
#[verifier::external_body]
pub(crate) fn normalize_hashes(hashes: &mut Vec<super::commit_hash::CommitHash>)
    ensures hashes_sorted(final(hashes)@), final(hashes)@.no_duplicates(),
        final(hashes)@.to_set() == old(hashes)@.to_set(), final(hashes).len() <= old(hashes).len(),
{
    hashes.sort_unstable();
    hashes.dedup();
}

// TRUST ARC-SLICE-1: conversion to the existing shared slice preserves every element.
#[verifier::external_body]
pub(crate) fn share_hashes(hashes: Vec<super::commit_hash::CommitHash>) -> (out: std::sync::Arc<[super::commit_hash::CommitHash]>)
    ensures (*out)@ == hashes@,
{ hashes.into() }

// TRUST ARC-BYTES-1: Vec<u8> to Arc<[u8]> preserves the canonical byte sequence.
#[verifier::external_body]
pub(crate) fn share_bytes(bytes: Vec<u8>) -> (out: std::sync::Arc<[u8]>)
    ensures (*out)@ == bytes@,
{ bytes.into() }

// TRUST RANGE-SORT-1: std sort by the concrete usize start field.
#[verifier::external_body]
pub(crate) fn sort_ranges(ranges: &mut Vec<std::ops::Range<usize>>)
    ensures final(ranges).len() == old(ranges).len(),
        final(ranges)@.to_set() == old(ranges)@.to_set(),
        forall|i: int, j: int| 0 <= i < j < final(ranges).len() ==> final(ranges)[i].start <= final(ranges)[j].start,
{ ranges.sort_unstable_by_key(|range| range.start); }

pub(crate) open spec fn ready_le(a: (super::commit_hash::CommitHash, NodeIdx), b: (super::commit_hash::CommitHash, NodeIdx)) -> bool {
    hash_less(a.0, b.0) || (a.0 == b.0 && a.1@ <= b.1@)
}

// TRUST READY-ORDER-1: derived byte-hash/u32 ordering and Rust tuple comparison laws.
#[verifier::external_body]
pub(crate) proof fn ready_key_model()
    ensures vstd::laws_cmp::obeys_cmp::<(super::commit_hash::CommitHash, NodeIdx)>(),
{}

// TRUST NODE-KEY-1: derived Hash/Eq/Clone on the pinned u32 index wrapper.
#[verifier::external_body]
pub(crate) proof fn node_key_model()
    ensures vstd::std_specs::hash::obeys_key_model::<NodeIdx>(),
{}

// TRUST NODE-ORDER-1: derived Ord/Eq on the pinned u32 index wrapper.
#[verifier::external_body]
pub(crate) proof fn node_order_model()
    ensures vstd::laws_cmp::obeys_cmp::<NodeIdx>(),
{}

// TRUST READY-POP-1: std BTreeSet pop_first with the fixed derived tuple order.
#[verifier::external_body]
pub(crate) fn pop_ready(ready: &mut std::collections::BTreeSet<(super::commit_hash::CommitHash, NodeIdx)>) -> (out: Option<(super::commit_hash::CommitHash, NodeIdx)>)
    ensures match out {
        None => old(ready)@.is_empty() && final(ready)@ == old(ready)@,
        Some(key) => old(ready)@.contains(key) && final(ready)@ == old(ready)@.remove(key)
            && (forall|other: (super::commit_hash::CommitHash, NodeIdx)| old(ready)@.contains(other) ==> ready_le(key, other)),
    },
{ ready.pop_first() }

// Spare capacity is allocation metadata, separate from a vector's sequence.
pub(crate) uninterp spec fn vec_spare<T>(v: Vec<T>) -> nat;

// TRUST VEC-EMPTY-1: Vec::new allocates nothing and cannot unwind.
#[verifier::external_body]
pub(crate) fn empty_vec<T>() -> (out: Vec<T>)
    ensures out@ == Seq::<T>::empty(),
    no_unwind
{ Vec::new() }

// TRUST VEC-RESERVE-1: fallible reservation preserves elements, including on failure.
#[verifier::external_body]
pub(crate) fn reserve_vec<T>(v: &mut Vec<T>, additional: usize) -> (out: bool)
    ensures final(v)@ == old(v)@, out ==> vec_spare(*final(v)) >= additional,
    no_unwind
{ v.try_reserve(additional).is_ok() }

// TRUST VEC-ROW-RESERVE-1: reserve one nested vector without changing any elements.
#[verifier::external_body]
pub(crate) fn reserve_row<T>(v: &mut Vec<Vec<T>>, index: usize, additional: usize) -> (out: bool)
    requires index < old(v).len(),
    ensures final(v).len() == old(v).len(), vec_spare(*final(v)) == vec_spare(*old(v)),
        forall|i: int| 0 <= i < old(v).len() ==> final(v)@[i]@ == old(v)@[i]@,
        out ==> vec_spare(final(v)@[index as int]) >= additional,
    no_unwind
{ v[index].try_reserve(additional).is_ok() }

// TRUST VEC-PUSH-1: a push with spare capacity neither allocates nor drops T.
#[verifier::external_body]
pub(crate) fn push_reserved<T>(v: &mut Vec<T>, value: T)
    requires vec_spare(*old(v)) > 0,
    ensures final(v)@ == old(v)@.push(value), vec_spare(*final(v)) + 1 == vec_spare(*old(v)),
    no_unwind
{ v.push(value); }

// TRUST VEC-SET-1: replacing Copy data at an in-bounds position preserves capacity.
#[verifier::external_body]
pub(crate) fn set_reserved<T: Copy>(v: &mut Vec<T>, index: usize, value: T)
    requires index < old(v).len(),
    ensures final(v)@ == old(v)@.update(index as int, value),
        vec_spare(*final(v)) == vec_spare(*old(v)),
    no_unwind
{ v[index] = value; }

// TRUST VEC-ROW-PUSH-1: an in-bounds nested reserved push preserves other rows.
#[verifier::external_body]
pub(crate) fn push_row_reserved<T>(v: &mut Vec<Vec<T>>, index: usize, value: T)
    requires index < old(v).len(), vec_spare(old(v)@[index as int]) > 0,
    ensures final(v).len() == old(v).len(), vec_spare(*final(v)) == vec_spare(*old(v)),
        final(v)@[index as int]@ == old(v)@[index as int]@.push(value),
        forall|i: int| 0 <= i < old(v).len() && i != index ==> final(v)@[i] == old(v)@[i],
    no_unwind
{ v[index].push(value); }

pub(crate) uninterp spec fn hash_spare<K, V>(m: std::collections::HashMap<K, V>) -> nat;

// TRUST HEAD-REMOVE-1: fixed Copy byte keys have non-panicking comparison/destruction.
#[verifier::external_body]
pub(crate) fn remove_head(m: &mut std::collections::BTreeSet<super::commit_hash::CommitHash>,
    key: &super::commit_hash::CommitHash)
    ensures final(m)@ == old(m)@.remove(*key),
    no_unwind
{ m.remove(key); }

// TRUST HEAD-INSERT-1: BTreeSet allocates fixed-size nodes; allocation failure aborts.
// The fixed byte key has no user comparison/drop callback or dynamic allocation.
#[verifier::external_body]
pub(crate) fn insert_head(m: &mut std::collections::BTreeSet<super::commit_hash::CommitHash>,
    key: super::commit_hash::CommitHash)
    ensures final(m)@ == old(m)@.insert(key),
    no_unwind
{ m.insert(key); }

// TRUST HASH-RESERVE-1: fixed derived byte keys cannot panic during rehashing.
#[verifier::external_body]
pub(crate) fn reserve_hashes(m: &mut std::collections::HashMap<super::commit_hash::CommitHash, NodeIdx>) -> (out: bool)
    ensures final(m)@ == old(m)@, out ==> hash_spare(*final(m)) > 0,
    no_unwind
{ m.try_reserve(1).is_ok() }

// TRUST HASH-INSERT-1: a reserved insert with these Copy keys/values cannot unwind.
#[verifier::external_body]
pub(crate) fn insert_hash_reserved(m: &mut std::collections::HashMap<super::commit_hash::CommitHash, NodeIdx>,
    key: super::commit_hash::CommitHash, value: NodeIdx)
    requires hash_spare(*old(m)) > 0,
    ensures final(m)@ == old(m)@.insert(key, value),
    no_unwind
{ m.insert(key, value); }

// TRUST AUTHOR-RESERVE-1: reserve the map and, if present, its existing u32 row.
#[verifier::external_body]
pub(crate) fn reserve_author(m: &mut std::collections::HashMap<super::author::Author, Vec<u32>>,
    key: super::author::Author) -> (out: bool)
    ensures final(m)@.dom() == old(m)@.dom(),
        forall|a: super::author::Author| old(m)@.contains_key(a) ==> final(m)@[a]@ == old(m)@[a]@,
        out ==> hash_spare(*final(m)) > 0,
        out && final(m)@.contains_key(key) ==> vec_spare(final(m)@[key]) > 0,
    no_unwind
{
    if m.try_reserve(1).is_err() { return false; }
    match m.get_mut(&key) { Some(row) => row.try_reserve(1).is_ok(), None => true }
}

// TRUST AUTHOR-PUSH-1: entry/or_insert and a reserved u32 push; no user callbacks.
#[verifier::external_body]
pub(crate) fn push_author_reserved(m: &mut std::collections::HashMap<super::author::Author, Vec<u32>>,
    key: super::author::Author, value: u32, empty: Vec<u32>)
    requires empty@.len() == 0, vec_spare(empty) > 0, hash_spare(*old(m)) > 0,
        old(m)@.contains_key(key) ==> vec_spare(old(m)@[key]) > 0,
    ensures final(m)@.dom() == old(m)@.dom().insert(key),
        final(m)@[key]@ == (if old(m)@.contains_key(key) { old(m)@[key]@ } else { Seq::empty() }).push(value),
        forall|a: super::author::Author| old(m)@.contains_key(a) && a != key ==> final(m)@[a] == old(m)@[a],
    no_unwind
{ m.entry(key).or_insert(empty).push(value); }

// TRUST ATOMIC-TYPE-1: an opaque std atomic, governed by vstd's linear permission.
#[verifier::external_body]
#[derive(Debug)]
pub(crate) struct RelaxedAtomic {
    value: AtomicU64,
}

impl RelaxedAtomic {
    pub uninterp spec fn id(&self) -> int;

    // TRUST ATOMIC-NEW-1: allocate the cell and its unique initial permission.
    #[verifier::external_body]
    pub(crate) const fn new(value: u64,
        #[cfg(verus_keep_ghost)] permission: &mut Tracked<Option<vstd::atomic::PermissionU64>>,
    ) -> (out: Self)
        ensures final(permission)@ is Some,
            final(permission)@->Some_0.id() == out.id(),
            final(permission)@->Some_0.value() == value,
    { Self { value: AtomicU64::new(value) } }

    // TRUST ATOMIC-LOAD-1: Relaxed observation; deliberately no value/order guarantee.
    #[verifier::external_body]
    #[verifier::atomic]
    pub(crate) fn load(&self,
        #[cfg(verus_keep_ghost)] Tracked(permission): Tracked<&vstd::atomic::PermissionU64>,
    ) -> (out: u64)
        requires permission.id() == self.id(),
        opens_invariants none
        no_unwind
    { self.value.load(Ordering::Relaxed) }

    // TRUST ATOMIC-CAS-1: successful Relaxed CAS updates exactly the matching cell.
    // Failure preserves the cell; its returned observation is left unconstrained.
    #[verifier::external_body]
    #[verifier::atomic]
    pub(crate) fn compare_exchange(&self, current: u64, next: u64,
        #[cfg(verus_keep_ghost)] Tracked(permission): Tracked<&mut vstd::atomic::PermissionU64>,
    ) -> (out: Result<u64, u64>)
        requires old(permission).id() == self.id(),
        ensures final(permission).id() == self.id(),
            match out {
                Ok(previous) => previous == current && previous == old(permission).value()
                    && final(permission).value() == next,
                Err(_) => final(permission).value() == old(permission).value(),
            },
        opens_invariants none
        no_unwind
    { self.value.compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed) }
}

pub(crate) uninterp spec fn global_allocator_ref() -> &'static super::identity::Allocator;

// TRUST STATIC-REF-1: every read borrows the same immutable Rust static.
// This assumes only reference stability, not an allocator or history invariant.
#[verifier::external_body]
pub(crate) fn global_allocator() -> (out: &'static super::identity::Allocator)
    ensures out == global_allocator_ref(),
{ &super::identity::ALLOCATOR }

// TRUST EXPECT-1: Option::expect returns its value or unwinds/aborts on None.
// This is a normal-return contract, not a panic-freedom guarantee.
#[verifier::external_body]
pub(crate) fn expect_some<T>(value: Option<T>, message: &str) -> (out: T)
    ensures value is Some, out == value->Some_0,
{ value.expect(message) }

// TRUST ONCE-TYPE-1: opaque representation of a private, write-once cell.
// No mutable access, take, or unchecked publication is exposed.
#[verifier::external_body]
#[derive(Debug)]
#[verifier::reject_recursive_types(T)]
pub(crate) struct OnceValue<T> {
    cell: OnceLock<T>,
}

impl<T> OnceValue<T> {
    pub uninterp spec fn predicate(&self) -> spec_fn(T) -> bool;

    // TRUST ONCE-NEW-1: an empty cell establishes any publication predicate.
    #[verifier::external_body]
    pub(crate) fn new(#[cfg(verus_keep_ghost)] Ghost(predicate): Ghost<spec_fn(T) -> bool>) -> (out: Self)
        ensures out.predicate() == predicate,
        no_unwind
    {
        Self { cell: OnceLock::new() }
    }

    // TRUST ONCE-GET-1: a successful read observes a fully published value.
    #[verifier::external_body]
    pub(crate) fn get(&self) -> (out: Option<&T>)
        ensures match out { Some(value) => self.predicate()(*value), None => true },
    {
        self.cell.get()
    }

    // TRUST ONCE-INIT-1: initialization or a competing successful initializer
    // publishes only a value satisfying the cell's fixed predicate.
    // These are normal-return guarantees, not a scheduler/deadlock theorem.
    #[verifier::external_body]
    pub(crate) fn get_or_init<F: FnOnce() -> T>(&self, init: F) -> (out: &T)
        requires init.requires(()),
            forall|value: T| #[trigger] init.ensures((), value) ==> self.predicate()(value),
        ensures self.predicate()(*out),
    {
        self.cell.get_or_init(init)
    }
}

// TRUST AUTHOR-KEY-1: Rust-derived Hash/Eq/Clone on the pinned [u8; 32] wrapper.
// This only enables vstd's collection specifications, not an application fact.
#[verifier::external_body]
pub(crate) proof fn author_key_model()
    ensures vstd::std_specs::hash::obeys_key_model::<super::author::Author>(),
{}

// TRUST COMMIT-KEY-1: the same key laws for the pinned commit-hash wrapper.
// No assumption of cryptographic collision resistance is made here.
#[verifier::external_body]
pub(crate) proof fn commit_key_model()
    ensures vstd::std_specs::hash::obeys_key_model::<super::commit_hash::CommitHash>(),
{}

// TRUST COMMIT-ORDER-1: Rust-derived Eq/PartialOrd/Ord on the pinned byte wrapper.
// This enables vstd's BTreeSet specifications; it assumes no graph/head property.
#[verifier::external_body]
pub(crate) proof fn commit_order_model()
    ensures vstd::laws_cmp::obeys_cmp::<super::commit_hash::CommitHash>(),
{}

// TRUST SORT-1: std slice sorting and NodeIdx's derived u32 ordering.
#[verifier::external_body]
pub(crate) fn sort_nodes(nodes: &mut Vec<NodeIdx>)
    ensures
        clock::ordered(final(nodes)@),
        final(nodes)@.to_multiset() == old(nodes)@.to_multiset(),
        forall|node: NodeIdx| final(nodes)@.contains(node) == old(nodes)@.contains(node),
{
    nodes.sort_unstable();
}

// TRUST DEDUP-1: std Vec deduplication and NodeIdx's derived u32 equality.
#[verifier::external_body]
pub(crate) fn dedup_nodes(nodes: &mut Vec<NodeIdx>)
    requires clock::ordered(old(nodes)@),
    ensures
        final(nodes).len() <= old(nodes).len(),
        forall|i: int, j: int| 0 <= i < j < final(nodes).len()
            ==> final(nodes)@[i]@ < final(nodes)@[j]@,
        forall|node: NodeIdx| final(nodes)@.contains(node) == old(nodes)@.contains(node),
{
    nodes.dedup();
}

} // verus!

#[cfg(test)]
mod once_tests {
    use super::OnceValue;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn once_value_publishes_one_value_under_contention() {
        let cell = OnceValue::<usize>::new();
        assert!(cell.get().is_none());
        let start = Barrier::new(8);
        let calls = AtomicUsize::new(0);
        let values = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|worker| {
                    let cell = &cell;
                    let start = &start;
                    let calls = &calls;
                    scope.spawn(move || {
                        start.wait();
                        *cell.get_or_init(|| {
                            calls.fetch_add(1, Ordering::Relaxed);
                            worker
                        })
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(values.iter().all(|value| *value == values[0]));
        assert_eq!(cell.get(), Some(&values[0]));
        let retained = cell.get().unwrap();
        let reused = cell.get_or_init(|| panic!("an initialized cell must not call this"));
        assert!(std::ptr::eq(retained, reused));
        assert_eq!(
            std::mem::size_of::<OnceValue<Arc<u32>>>(),
            std::mem::size_of::<std::sync::OnceLock<Arc<u32>>>()
        );
    }

    #[test]
    fn panicking_initializer_leaves_the_cell_available_for_retry() {
        let cell = OnceValue::<u32>::new();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cell.get_or_init(|| panic!("incomplete initialization"));
        }));
        assert!(failed.is_err());
        assert!(cell.get().is_none());
        assert_eq!(*cell.get_or_init(|| 7), 7);
        assert_eq!(cell.get(), Some(&7));
    }
}

#[cfg(test)]
mod atomic_tests {
    use super::RelaxedAtomic;

    #[test]
    fn relaxed_atomic_models_success_failure_and_arbitrary_replacement() {
        let cell = RelaxedAtomic::new(9);
        assert_eq!(cell.load(), 9);
        assert_eq!(cell.compare_exchange(8, 1), Err(9));
        assert_eq!(cell.load(), 9);
        assert_eq!(cell.compare_exchange(9, 1), Ok(9));
        assert_eq!(cell.load(), 1);
        assert_eq!(cell.compare_exchange(1, u64::MAX), Ok(1));
        assert_eq!(cell.load(), u64::MAX);
    }
}

#[cfg(test)]
mod earlier_tests {
    use super::{Earlier, NodeIdx};
    use std::collections::BTreeMap;

    #[test]
    fn range_adapter_matches_numeric_filter_and_preserves_values() {
        let keys = [u32::MAX, 0, 1 << 31, 7, 1 << 16, 1];
        for count in 0..=keys.len() {
            let map: BTreeMap<_, _> = keys[..count]
                .iter()
                .map(|&k| (NodeIdx::new(k), u64::from(k) + 1))
                .collect();
            for limit in [0, 1, 2, 7, 8, 1 << 16, 1 << 31, u32::MAX] {
                let mut expected: Vec<_> = keys[..count]
                    .iter()
                    .copied()
                    .filter(|&k| k < limit)
                    .map(|k| (k as usize, u64::from(k) + 1))
                    .collect();
                expected.sort_unstable();
                let (mut range, nonempty) = Earlier::new(&map, NodeIdx::new(limit));
                assert_eq!(nonempty, !expected.is_empty());
                let mut actual = Vec::new();
                while let Some((&key, &value)) = range.next() {
                    actual.push((key.index(), value));
                }
                assert_eq!(actual, expected);
                assert_eq!(range.next(), None);
                assert_eq!(range.next(), None);
            }
        }
    }
}
