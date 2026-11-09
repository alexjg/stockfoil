//! Non-reusing numeric IDs and their persistent binding to a logical lineage.
use super::stdlib::{self, RelaxedAtomic};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    atomic::PermissionU64,
    invariant::{AtomicInvariant, InvariantPredicate, open_atomic_invariant},
    prelude::*,
    resource::{
        Loc,
        map::{GhostMapAuth, GhostPersistentPointsTo},
    },
};

#[cfg(not(verus_keep_ghost))]
use super::stdlib::open_atomic_invariant;

verus! {

#[cfg(verus_keep_ghost)]
pub(crate) struct AllocationInvariant;
#[cfg(verus_keep_ghost)]
impl InvariantPredicate<(int, Loc, u64), (PermissionU64, GhostMapAuth<u64, Loc>)> for AllocationInvariant {
    open spec fn inv(key: (int, Loc, u64), state: (PermissionU64, GhostMapAuth<u64, Loc>)) -> bool {
        &&& state.0.id() == key.0
        &&& state.1.id() == key.1
        &&& state.0.value() >= key.2
        &&& forall|id: u64| state.1@.contains_key(id) ==> id < state.0.value()
    }
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Allocator {
    counter: RelaxedAtomic,
    #[cfg(verus_keep_ghost)] invariant: Tracked<AtomicInvariant<(int, Loc, u64),
        (PermissionU64, GhostMapAuth<u64, Loc>), AllocationInvariant>>,
}

#[allow(unused_braces)]
pub(crate) exec static ALLOCATOR: Allocator
    ensures ALLOCATOR.valid(),
{ Allocator::new(1) }

impl Allocator {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { self.valid() }
    pub(crate) closed spec fn valid(&self) -> bool {
        self.invariant@.constant().0 == self.counter.id()
    }
    pub(crate) closed spec fn initial(&self) -> u64 { self.invariant@.constant().2 }
    pub(crate) closed spec fn registry(&self) -> Loc { self.invariant@.constant().1 }

    pub(crate) const fn new(start: u64) -> (out: Self)
        ensures out.valid(), out.initial() == start,
    {
        #[cfg(verus_keep_ghost)] let mut permission = Tracked(None);
        let counter = RelaxedAtomic::new(start,
            #[cfg(verus_keep_ghost)] { &mut permission });
        let tracked (registry, unused) = GhostMapAuth::<u64, Loc>::new(Map::empty());
        let tracked invariant = AtomicInvariant::new((counter.id(), registry.id(), start),
            (permission.get().tracked_unwrap(), registry), 0);
        Self { counter, #[cfg(verus_keep_ghost)] invariant: Tracked(invariant) }
    }

    // Contention can retry indefinitely; only normal-return safety is claimed.
    #[verifier::exec_allows_no_decreases_clause]
    pub(crate) fn allocate(&self,
        #[cfg(verus_keep_ghost)] Ghost(lineage): Ghost<Loc>,
        #[cfg(verus_keep_ghost)] binding: &mut Tracked<Option<GhostPersistentPointsTo<u64, Loc>>>,
    ) -> (out: Option<u64>)
        ensures match out {
            None => final(binding)@ is None,
            Some(id) => self.initial() <= id < u64::MAX && final(binding)@ is Some
                && final(binding)@->Some_0.id() == self.registry()
                && final(binding)@->Some_0.key() == id
                && final(binding)@->Some_0.value() == lineage,
        },
    {
        proof { use_type_invariant(self); *binding.borrow_mut() = None; }
        let mut observed;
        open_atomic_invariant!(self.invariant.borrow() => state => {
            observed = self.counter.load(#[cfg(verus_keep_ghost)] { Tracked(&state.0) });
        });
        loop
            invariant self.valid(), binding@ is None,
        {
            if observed == u64::MAX { return None; }
            let next = observed + 1;
            let result;
            open_atomic_invariant!(self.invariant.borrow() => state => {
                result = self.counter.compare_exchange(observed, next,
                    #[cfg(verus_keep_ghost)] { Tracked(&mut state.0) });
                proof {
                    if result is Ok {
                        let tracked point = state.1.insert(observed, lineage).persist();
                        *binding.borrow_mut() = Some(point);
                    }
                }
            });
            match result {
                Ok(id) => return Some(id),
                Err(current) => observed = current,
            }
        }
    }
}

/// One u64 at runtime. The persistent binding is erased.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Identity {
    value: u64,
    #[cfg(verus_keep_ghost)] binding: Tracked<GhostPersistentPointsTo<u64, Loc>>,
}

impl Identity {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool {
        self.binding@.key() == self.value
            && self.binding@.id() == stdlib::global_allocator_ref().registry()
    }
    pub closed spec fn lineage(&self) -> Loc { self.binding@.value() }
    pub closed spec fn raw(&self) -> u64 { self.value }

    pub(crate) fn new(#[cfg(verus_keep_ghost)] Ghost(lineage): Ghost<Loc>) -> (out: Self)
        ensures out.lineage() == lineage,
    {
        let counter = stdlib::global_allocator();
        #[cfg(verus_keep_ghost)] let mut binding = Tracked(None);
        let value = counter.allocate(#[cfg(verus_keep_ghost)] { Ghost(lineage) },
            #[cfg(verus_keep_ghost)] { &mut binding });
        let value = stdlib::expect_some(value, "history id space");
        Self { value, #[cfg(verus_keep_ghost)] binding: Tracked(binding.get().tracked_unwrap()) }
    }

    pub(crate) fn value(&self) -> (out: u64) ensures out == self.raw(), { self.value }

    pub(crate) proof fn agreement(tracked &self, tracked other: &Self)
        ensures self.raw() == other.raw() ==> self.lineage() == other.lineage(),
    {
        use_type_invariant(self); use_type_invariant(other);
        let tracked mut copy = self.binding.borrow().duplicate();
        copy.intersection_agrees(other.binding.borrow());
    }

    pub(crate) fn same(&self, other: &Self) -> (out: bool)
        ensures out == (self.raw() == other.raw()), out ==> self.lineage() == other.lineage(),
    {
        proof { self.agreement(other); }
        self.value == other.value
    }
}

impl Clone for Identity {
    fn clone(&self) -> (out: Self)
        ensures out.raw() == self.raw(), out.lineage() == self.lineage(),
    {
        proof { use_type_invariant(self); }
        Self { value: self.value, #[cfg(verus_keep_ghost)] binding: Tracked(self.binding.borrow().duplicate()) }
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn exhaustion_never_wraps_or_reuses_an_id() {
        let counter = Allocator::new(u64::MAX - 1);
        assert_eq!(counter.allocate(), Some(u64::MAX - 1));
        assert_eq!(counter.counter.load(), u64::MAX);
        for _ in 0..2 {
            assert!(
                std::panic::catch_unwind(|| {
                    stdlib::expect_some(counter.allocate(), "history id space")
                })
                .is_err()
            );
            assert_eq!(counter.counter.load(), u64::MAX);
            assert_eq!(counter.allocate(), None);
        }
    }

    #[test]
    fn concurrent_allocations_are_unique_including_at_exhaustion() {
        for (start, workers, per_worker) in [(1, 8, 512), (u64::MAX - 31, 8, 8)] {
            let allocator = Arc::new(Allocator::new(start));
            let barrier = Arc::new(Barrier::new(workers));
            let jobs: Vec<_> = (0..workers)
                .map(|_| {
                    let allocator = Arc::clone(&allocator);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        (0..per_worker)
                            .filter_map(|_| allocator.allocate())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut ids: Vec<_> = jobs
                .into_iter()
                .flat_map(|job| job.join().unwrap())
                .collect();
            ids.sort_unstable();
            let count = (workers * per_worker) as u64;
            let end = start.saturating_add(count);
            assert_eq!(ids, (start..end).collect::<Vec<_>>());
            assert_eq!(allocator.counter.load(), end);
        }
    }

    #[test]
    fn cloned_identities_match_and_new_identities_do_not() {
        assert_eq!(std::mem::size_of::<Identity>(), std::mem::size_of::<u64>());
        assert_eq!(
            std::mem::size_of::<Allocator>(),
            std::mem::size_of::<std::sync::atomic::AtomicU64>()
        );
        let first = Identity::new();
        let same = first.clone();
        let other = Identity::new();
        assert!(first.same(&same));
        assert_eq!(first.value(), same.value());
        assert!(!first.same(&other));
    }
}
