//! Production clock equality and its extensional map meaning for ordered clocks.
#[cfg(verus_keep_ghost)]
use super::clock::NodeIdx;
use super::tree::Clock;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg(verus_keep_ghost)]
impl vstd::std_specs::cmp::PartialEqSpecImpl for Clock {
    open spec fn obeys_eq_spec() -> bool { true }
    open spec fn eq_spec(&self, other: &Self) -> bool {
        self.stored_len() == other.stored_len() && self.sequence() == other.sequence()
    }
}

impl PartialEq for Clock {
    fn eq(&self, other: &Self) -> (out: bool)
        ensures out == (self.stored_len() == other.stored_len() && self.sequence() == other.sequence()),
    {
        if self.len() != other.len() { return false; }
        let mut left = self.entries();
        let mut right = other.entries();
        let ghost mut i: int = 0;
        loop
            invariant self.stored_len() == other.stored_len(), 0 <= i <= self.sequence().len(), i <= other.sequence().len(),
                left.remaining() == self.sequence().skip(i), right.remaining() == other.sequence().skip(i),
                self.sequence().take(i) == other.sequence().take(i),
            decreases left.remaining().len() + right.remaining().len(),
        {
            let a = left.next();
            let b = right.next();
            match (a, b) {
                (None, None) => {
                    assert(i == self.sequence().len() && i == other.sequence().len());
                    assert(self.sequence().take(i) =~= self.sequence());
                    assert(other.sequence().take(i) =~= other.sequence());
                    return true;
                },
                (Some(a), Some(b)) => {
                    if a.0 != b.0 || a.1.index() != b.1.index() { return false; }
                    proof { NodeIdx::same_index(a.1, b.1); }
                    assert(self.sequence().take(i + 1) =~= self.sequence().take(i).push(a));
                    assert(other.sequence().take(i + 1) =~= other.sequence().take(i).push(b));
                    proof { i = i + 1; }
                },
                _ => { return false; },
            }
        }
    }
}
impl Eq for Clock {}

impl Clock {
    proof fn equal_map_prefix(&self, other: &Self, n: nat)
        requires self.ordered(), other.ordered(), self.contents() == other.contents(),
            n <= self.sequence().len(), n <= other.sequence().len(),
        ensures self.sequence().take(n as int) == other.sequence().take(n as int),
        decreases n,
    {
        self.sequence_correct(); other.sequence_correct();
        if n > 0 {
            self.equal_map_prefix(other, (n - 1) as nat);
            let i = n as int - 1;
            let a = self.sequence()[i]; let b = other.sequence()[i];
            let j = choose|j: int| 0 <= j < self.sequence().len() && self.sequence()[j].0 == b.0;
            let k = choose|k: int| 0 <= k < other.sequence().len() && other.sequence()[k].0 == a.0;
            if j < i { assert(self.sequence().take(i)[j] == self.sequence()[j]); assert(other.sequence().take(i)[j] == other.sequence()[j]); }
            if k < i { assert(self.sequence().take(i)[k] == self.sequence()[k]); assert(other.sequence().take(i)[k] == other.sequence()[k]); }
            assert(j >= i && k >= i);
            assert(a.0 <= b.0 && b.0 <= a.0);
            assert(a == b);
            assert(self.sequence().take(n as int) =~= self.sequence().take(i).push(a));
            assert(other.sequence().take(n as int) =~= other.sequence().take(i).push(b));
        }
    }

    pub(crate) proof fn sequence_map_equality(&self, other: &Self)
        requires self.ordered(), other.ordered(),
        ensures (self.sequence() == other.sequence()) == (self.contents() == other.contents()),
    {
        self.sequence_correct(); other.sequence_correct();
        if self.contents() == other.contents() {
            let n = if self.sequence().len() < other.sequence().len() { self.sequence().len() } else { other.sequence().len() };
            self.equal_map_prefix(other, n);
            if self.sequence().len() < other.sequence().len() {
                let c = other.sequence()[n as int].0;
                let i = choose|i: int| 0 <= i < self.sequence().len() && self.sequence()[i].0 == c;
                assert(self.sequence().take(n as int)[i] == self.sequence()[i]);
                assert(other.sequence().take(n as int)[i] == other.sequence()[i]);
            }
            if other.sequence().len() < self.sequence().len() {
                let c = self.sequence()[n as int].0;
                let i = choose|i: int| 0 <= i < other.sequence().len() && other.sequence()[i].0 == c;
                assert(self.sequence().take(n as int)[i] == self.sequence()[i]);
                assert(other.sequence().take(n as int)[i] == other.sequence()[i]);
            }
            assert(self.sequence().take(n as int) =~= self.sequence());
            assert(other.sequence().take(n as int) =~= other.sequence());
        }
        if self.sequence() == other.sequence() {
            assert(self.contents() =~= other.contents()) by {
                assert forall|c: u32| self.contents().contains_key(c) == other.contents().contains_key(c)
                    && (self.contents().contains_key(c) ==> self.contents()[c] == other.contents()[c]) by {
                    if self.contents().contains_key(c) {
                        let i = choose|i: int| 0 <= i < self.sequence().len() && self.sequence()[i].0 == c;
                        assert(other.sequence()[i].0 == c);
                    }
                    if other.contents().contains_key(c) {
                        let i = choose|i: int| 0 <= i < other.sequence().len() && other.sequence()[i].0 == c;
                        assert(self.sequence()[i].0 == c);
                    }
                }
            }
        }
    }

    pub(crate) fn same_map(&self, other: &Self) -> (out: bool)
        requires self.valid(), other.valid(),
        ensures out == (self.contents() == other.contents()),
    {
        proof { self.sequence_map_equality(other); self.sequence_correct(); other.sequence_correct(); }
        let same = self.eq(other);
        let _left_len = self.len();
        let _right_len = other.len();
        same
    }
}

} // verus!
