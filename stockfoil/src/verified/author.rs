//! Author identity used by the checked chain selector. No signature claim.
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

pub use super::author_key::Author;

verus! {

impl Author {
    pub(crate) fn same_as(&self, other: &Self) -> (out: bool)
        ensures out == (*self == *other),
    {
        let mut i = 0;
        while i < 32
            invariant i <= 32,
                forall|j: int| 0 <= j < i ==> self.0@[j] == other.0@[j],
            decreases 32 - i,
        {
            if self.0[i] != other.0[i] { return false; }
            i += 1;
        }
        assert(self.0@ =~= other.0@);
        assert(self.0 =~= other.0);
        true
    }
}

} // verus!
