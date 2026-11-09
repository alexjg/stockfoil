//! A real saturated allocator cannot issue an ID; its storage is still an atomic.
use vstd::prelude::*;
use vstd::resource::Loc;
use crate::identity::{Allocator, Identity};

verus! {

pub fn saturated_allocator_returns_no_id(Ghost(lineage): Ghost<Loc>) {
    let counter = Allocator::new(u64::MAX);
    let mut binding = Tracked(None);
    let result = counter.allocate(Ghost(lineage), &mut binding);
    assert(result is None);
    assert(binding@ is None);
}

pub fn identity_clone_preserves_origin(Ghost(lineage): Ghost<Loc>) {
    let original = Identity::new(Ghost(lineage));
    let cloned = original.clone();
    let matches = original.same(&cloned);
    assert(matches && cloned.lineage() == lineage);
}

} // verus!
