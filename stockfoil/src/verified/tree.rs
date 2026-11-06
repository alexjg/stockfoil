//! Shared persistent clock representation, construction, lookup and balancing.
//! Update callers must still establish ordering, balance and capacity premises.
use super::clock::NodeIdx;
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
pub(crate) type ChainId = u32;

pub(crate) struct Clock(pub(crate) Option<Arc<Entry>>);

pub(crate) struct Entry {
    pub(crate) chain: ChainId,
    pub(crate) tip: NodeIdx,
    pub(crate) height: u8,
    pub(crate) len: u32,
    pub(crate) left: Clock,
    pub(crate) right: Clock,
}

pub(crate) struct Entries<'a> {
    stack: Vec<&'a Entry>,
    node: &'a Clock,
}

impl<'a> Entries<'a> {
    pub closed spec fn pending_stack(stack: Seq<&Entry>) -> Seq<(ChainId, NodeIdx)>
        decreases stack.len(),
    {
        if stack.len() == 0 { Seq::empty() } else {
            let e = stack.last();
            seq![(e.chain, e.tip)] + e.right.sequence() + Self::pending_stack(stack.drop_last())
        }
    }

    pub closed spec fn remaining(&self) -> Seq<(ChainId, NodeIdx)> {
        self.node.sequence() + Self::pending_stack(self.stack@)
    }
}

#[cfg(verus_keep_ghost)]
impl<'a> vstd::std_specs::iter::IteratorSpecImpl for Entries<'a> {
    open spec fn obeys_prophetic_iter_laws(&self) -> bool { true }
    open spec fn remaining(&self) -> Seq<(ChainId, NodeIdx)> { Entries::remaining(self) }
    open spec fn produces(&self) -> Seq<bool> { Seq::new(Entries::remaining(self).len(), |i: int| true) }
    open spec fn decrease(&self) -> Option<nat> { Some(Entries::remaining(self).len()) }
    open spec fn peek(&self, index: int) -> Option<(ChainId, NodeIdx)> {
        if 0 <= index < Entries::remaining(self).len() { Some(Entries::remaining(self)[index]) } else { None }
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = (ChainId, NodeIdx);

    fn next(&mut self) -> (out: Option<Self::Item>)
        ensures
            match out {
                None => old(self).remaining().len() == 0 && final(self).remaining() == old(self).remaining(),
                Some(item) => old(self).remaining().len() > 0 && item == old(self).remaining()[0]
                    && final(self).remaining() == old(self).remaining().drop_first(),
            },
    {
        let ghost remaining = self.remaining();
        loop
            invariant self.remaining() == remaining,
            ensures self.node.0 is None, self.remaining() == remaining,
            decreases self.node,
        {
            match &self.node.0 {
                None => break,
                Some(e) => {
                    let ghost stack = self.stack@;
                    self.stack.push(&**e);
                    assert(self.stack@.last() == &**e);
                    assert(self.stack@.drop_last() =~= stack);
                    assert(Self::pending_stack(self.stack@) =~= seq![(e.chain, e.tip)] + e.right.sequence() + Self::pending_stack(stack));
                    self.node = &e.left;
                    assert(self.remaining() =~= remaining);
                }
            }
        }
        assert(self.node.sequence() == Seq::<(ChainId, NodeIdx)>::empty());
        let ghost stack = self.stack@;
        assert(remaining == Self::pending_stack(stack));
        let e = match self.stack.pop() {
            None => {
                assert(stack.len() == 0);
                assert(remaining.len() == 0);
                return None;
            },
            Some(e) => e,
        };
        assert(stack.last() == e);
        assert(stack.drop_last() =~= self.stack@);
        assert(remaining == seq![(e.chain, e.tip)] + e.right.sequence() + Self::pending_stack(self.stack@));
        self.node = &e.right;
        assert(self.remaining() =~= remaining.drop_first());
        Some((e.chain, e.tip))
    }
}

impl Default for Clock {
    fn default() -> (out: Self)
        ensures out.ordered(), out.contents() == Map::<ChainId, NodeIdx>::empty(),
            out.metadata(), out.balanced(), out.size() == 0, out.depth() == 0,
    {
        Self(None)
    }
}

impl Clone for Clock {
    fn clone(&self) -> (out: Self)
        ensures out == *self,
    {
        match &self.0 {
            None => Self(None),
            Some(e) => Self(Some(Arc::clone(e))),
        }
    }
}

impl Clock {
    pub closed spec fn sequence(&self) -> Seq<(ChainId, NodeIdx)>
        decreases self,
    {
        match self.0 {
            None => Seq::empty(),
            Some(e) => e.left.sequence() + seq![(e.chain, e.tip)] + e.right.sequence(),
        }
    }

    pub(crate) proof fn sequence_correct(&self)
        requires self.ordered(),
        ensures
            self.sequence().len() == self.size(),
            forall|i: int| 0 <= i < self.sequence().len() ==>
                self.contents().contains_key(#[trigger] self.sequence()[i].0)
                && self.contents()[self.sequence()[i].0] == self.sequence()[i].1,
            forall|c: ChainId| self.contents().contains_key(c) ==>
                exists|i: int| 0 <= i < self.sequence().len() && self.sequence()[i].0 == c,
            forall|i: int, j: int| 0 <= i < j < self.sequence().len() ==>
                self.sequence()[i].0 < self.sequence()[j].0,
        decreases self,
    {
        self.unfold();
        if let Some(e) = self.0 {
            e.left.sequence_correct();
            e.right.sequence_correct();
            let l = e.left.sequence();
            let r = e.right.sequence();
            let s = self.sequence();
            assert(s =~= l + seq![(e.chain, e.tip)] + r);
            assert forall|i: int| 0 <= i < s.len() implies
                self.contents().contains_key(#[trigger] s[i].0) && self.contents()[s[i].0] == s[i].1 by {
                if i < l.len() {
                    assert(s[i] == l[i]);
                } else if i == l.len() {
                    assert(s[i] == (e.chain, e.tip));
                } else {
                    assert(s[i] == r[i - l.len() - 1]);
                }
            }
            assert forall|c: ChainId| self.contents().contains_key(c) implies
                exists|i: int| 0 <= i < s.len() && s[i].0 == c by {
                if c == e.chain {
                    assert(s[l.len() as int].0 == c);
                } else if e.left.contents().contains_key(c) {
                    let i = choose|i: int| 0 <= i < l.len() && l[i].0 == c;
                    assert(s[i].0 == c);
                } else {
                    let i = choose|i: int| 0 <= i < r.len() && r[i].0 == c;
                    assert(s[i + l.len() + 1].0 == c);
                }
            }
            assert forall|i: int, j: int| 0 <= i < j < s.len() implies s[i].0 < s[j].0 by {
                if j < l.len() {
                    assert(s[i] == l[i] && s[j] == l[j]);
                } else if i > l.len() {
                    assert(s[i] == r[i - l.len() - 1] && s[j] == r[j - l.len() - 1]);
                } else {
                    if i < l.len() { assert(s[i] == l[i]); }
                    if j > l.len() { assert(s[j] == r[j - l.len() - 1]); }
                }
            }
        }
    }

    pub fn entries(&self) -> (out: Entries<'_>)
        ensures out.remaining() == self.sequence(),
    {
        Entries { stack: Vec::new(), node: self }
    }

    pub closed spec fn size(&self) -> nat
        decreases self,
    {
        match self.0 { None => 0, Some(e) => 1 + e.left.size() + e.right.size() }
    }

    pub closed spec fn depth(&self) -> nat
        decreases self,
    {
        match self.0 {
            None => 0,
            Some(e) => 1 + if e.left.depth() >= e.right.depth() { e.left.depth() } else { e.right.depth() },
        }
    }

    pub closed spec fn metadata(&self) -> bool
        decreases self,
    {
        match self.0 {
            None => true,
            Some(e) => e.left.metadata() && e.right.metadata()
                && e.len == self.size() && e.height == self.depth(),
        }
    }

    pub closed spec fn balanced(&self) -> bool
        decreases self,
    {
        match self.0 {
            None => true,
            Some(e) => e.left.balanced() && e.right.balanced()
                && e.left.depth() <= e.right.depth() + 1
                && e.right.depth() <= e.left.depth() + 1,
        }
    }

    pub closed spec fn stored_height(&self) -> u8 {
        match self.0 { None => 0, Some(e) => e.height }
    }

    pub closed spec fn stored_len(&self) -> u32 {
        match self.0 { None => 0, Some(e) => e.len }
    }

    pub(crate) fn height(&self) -> (out: u8)
        ensures out == self.stored_height(), self.metadata() ==> out == self.depth(),
    {
        match &self.0 { None => 0, Some(e) => e.height }
    }

    pub fn len(&self) -> (out: usize)
        ensures out == self.stored_len(), self.metadata() ==> out == self.size(),
    {
        match &self.0 { None => 0, Some(e) => e.len as usize }
    }

    // This constructor also serves temporary unbalanced nodes during rotations;
    // balance is intentionally not a premise or conclusion here.
    pub(crate) fn node(chain: ChainId, tip: NodeIdx, left: Self, right: Self) -> (out: Self)
        requires
            left.ordered(), right.ordered(), left.metadata(), right.metadata(),
            forall|c: ChainId| left.contents().contains_key(c) ==> c < chain,
            forall|c: ChainId| right.contents().contains_key(c) ==> chain < c,
            1 + left.size() + right.size() <= u32::MAX,
            left.depth() < u8::MAX, right.depth() < u8::MAX,
        ensures
            match out.0 {
                None => false,
                Some(e) => e.chain == chain && e.tip == tip && e.left == left && e.right == right,
            },
            out.ordered(), out.metadata(),
            out.balanced() == (left.balanced() && right.balanced()
                && left.depth() <= right.depth() + 1 && right.depth() <= left.depth() + 1),
            out.contents() == left.contents().union_prefer_right(right.contents()).insert(chain, tip),
            out.size() == 1 + left.size() + right.size(),
            out.depth() == 1 + if left.depth() >= right.depth() { left.depth() } else { right.depth() },
    {
        let lh = left.height();
        let rh = right.height();
        let height = 1 + if lh >= rh { lh } else { rh };
        let len = 1 + left.len() as u32 + right.len() as u32;
        Self(Some(Arc::new(Entry { chain, tip, height, len, left, right })))
    }

    // Rebalance after a one-level change to a child. Updates still need to
    // establish these premises, including capacity, before calling this routine.
    pub(crate) fn balance(chain: ChainId, tip: NodeIdx, left: Self, right: Self) -> (out: Self)
        requires
            left.ordered(), right.ordered(), left.metadata(), right.metadata(),
            left.balanced(), right.balanced(),
            forall|c: ChainId| left.contents().contains_key(c) ==> c < chain,
            forall|c: ChainId| right.contents().contains_key(c) ==> chain < c,
            1 + left.size() + right.size() <= u32::MAX,
            left.depth() < u8::MAX, right.depth() < u8::MAX,
            left.depth() <= right.depth() + 2, right.depth() <= left.depth() + 2,
        ensures
            (left.depth() <= right.depth() + 1 && right.depth() <= left.depth() + 1) ==>
                out.depth() == 1 + if left.depth() >= right.depth() { left.depth() } else { right.depth() },
            out.ordered(), out.metadata(), out.balanced(),
            out.contents() == left.contents().union_prefer_right(right.contents()).insert(chain, tip),
            out.size() == 1 + left.size() + right.size(),
            left.depth() <= out.depth(), right.depth() <= out.depth(),
            out.depth() <= 1 + if left.depth() >= right.depth() { left.depth() } else { right.depth() },
    {
        if left.height() > right.height() + 1 {
            return Self::balance_left(chain, tip, left, right);
        }
        if right.height() > left.height() + 1 {
            return Self::balance_right(chain, tip, left, right);
        }
        Self::node(chain, tip, left, right)
    }

    #[verifier::rlimit(30)]
    fn balance_left(chain: ChainId, tip: NodeIdx, mut left: Self, right: Self) -> (out: Self)
        requires
            left.ordered(), right.ordered(), left.metadata(), right.metadata(),
            left.balanced(), right.balanced(),
            forall|c: ChainId| left.contents().contains_key(c) ==> c < chain,
            forall|c: ChainId| right.contents().contains_key(c) ==> chain < c,
            1 + left.size() + right.size() <= u32::MAX,
            left.depth() < u8::MAX, right.depth() < u8::MAX,
            left.depth() <= right.depth() + 2, right.depth() <= left.depth() + 2,
            left.depth() == right.depth() + 2,
        ensures
            out.ordered(), out.metadata(), out.balanced(),
            out.contents() == left.contents().union_prefer_right(right.contents()).insert(chain, tip),
            out.size() == 1 + left.size() + right.size(),
            left.depth() <= out.depth(), right.depth() <= out.depth(),
            out.depth() <= 1 + if left.depth() >= right.depth() { left.depth() } else { right.depth() },
    {
        proof { left.unfold(); right.unfold(); }
        let ghost original_left = left;
        let l = left.0.as_ref().unwrap();
        proof { l.left.unfold(); l.right.unfold(); }
        if l.right.height() > l.left.height() {
            let r = l.right.0.as_ref().unwrap();
            proof {
                assert forall|c: ChainId| r.left.contents().contains_key(c) implies l.chain < c by {
                    assert(l.right.contents().contains_key(c));
                }
            }
            left = Self::node(
                r.chain,
                r.tip,
                Self::node(l.chain, l.tip, l.left.clone(), r.left.clone()),
                r.right.clone(),
            );
            assert(left.contents() =~= original_left.contents());
            assert(left.size() == original_left.size());
        }
        proof { left.unfold(); }
        let l = left.0.as_ref().unwrap();
        proof {
            assert forall|c: ChainId| l.right.contents().contains_key(c) implies c < chain by {
                assert(left.contents().contains_key(c));
            }
        }
        return Self::node(
            l.chain,
            l.tip,
            l.left.clone(),
            Self::node(chain, tip, l.right.clone(), right),
        );
    }

    #[verifier::rlimit(30)]
    fn balance_right(chain: ChainId, tip: NodeIdx, left: Self, mut right: Self) -> (out: Self)
        requires
            left.ordered(), right.ordered(), left.metadata(), right.metadata(),
            left.balanced(), right.balanced(),
            forall|c: ChainId| left.contents().contains_key(c) ==> c < chain,
            forall|c: ChainId| right.contents().contains_key(c) ==> chain < c,
            1 + left.size() + right.size() <= u32::MAX,
            left.depth() < u8::MAX, right.depth() < u8::MAX,
            left.depth() <= right.depth() + 2, right.depth() <= left.depth() + 2,
            right.depth() == left.depth() + 2,
        ensures
            out.ordered(), out.metadata(), out.balanced(),
            out.contents() == left.contents().union_prefer_right(right.contents()).insert(chain, tip),
            out.size() == 1 + left.size() + right.size(),
            left.depth() <= out.depth(), right.depth() <= out.depth(),
            out.depth() <= 1 + if left.depth() >= right.depth() { left.depth() } else { right.depth() },
    {
        proof { left.unfold(); right.unfold(); }
        let ghost original_right = right;
        let r = right.0.as_ref().unwrap();
        proof { r.left.unfold(); r.right.unfold(); }
        if r.left.height() > r.right.height() {
            let l = r.left.0.as_ref().unwrap();
            proof {
                assert forall|c: ChainId| l.right.contents().contains_key(c) implies c < r.chain by {
                    assert(r.left.contents().contains_key(c));
                }
            }
            right = Self::node(
                l.chain,
                l.tip,
                l.left.clone(),
                Self::node(r.chain, r.tip, l.right.clone(), r.right.clone()),
            );
            assert(right.contents() =~= original_right.contents());
            assert(right.size() == original_right.size());
        }
        proof { right.unfold(); }
        let r = right.0.as_ref().unwrap();
        proof {
            assert forall|c: ChainId| r.left.contents().contains_key(c) implies chain < c by {
                assert(right.contents().contains_key(c));
            }
        }
        return Self::node(
            r.chain,
            r.tip,
            Self::node(chain, tip, left, r.left.clone()),
            r.right.clone(),
        );
    }

    // A deliberately loose lower bound, sufficient to rule out u8 overflow
    // for any balanced tree whose cached node count fits u32.
    pub closed spec fn minimum_size(height: nat) -> nat
        decreases height,
    {
        if height <= 1 { height } else { 1 + 2 * Self::minimum_size((height - 2) as nat) }
    }

    proof fn tall_tree_has_nodes(&self, height: nat)
        requires self.balanced(), self.depth() >= height,
        ensures self.size() >= Self::minimum_size(height),
        decreases height,
    {
        self.unfold();
        if height > 0 {
            let e = self.0.unwrap();
            if height > 1 {
                e.left.tall_tree_has_nodes((height - 2) as nat);
                e.right.tall_tree_has_nodes((height - 2) as nat);
            }
        }
    }

    pub(crate) proof fn height_fits(&self)
        requires self.balanced(), self.metadata(),
        ensures self.depth() < 65,
    {
        self.unfold();
        if self.depth() >= 65 {
            self.tall_tree_has_nodes(65);
            assert(Self::minimum_size(65) == 8589934591nat) by (compute);
        }
    }

    pub open spec fn valid(&self) -> bool {
        self.ordered() && self.metadata() && self.balanced()
    }

    pub(crate) fn set(&self, chain: ChainId, tip: NodeIdx) -> (out: Self)
        requires self.valid(),
            self.size() + (if self.contents().contains_key(chain) { 0nat } else { 1nat }) <= u32::MAX,
        ensures out.valid(), out.contents() == self.contents().insert(chain, tip),
            out.size() == self.size() + (if self.contents().contains_key(chain) { 0nat } else { 1nat }),
            self.depth() <= out.depth() <= self.depth() + 1,
        decreases self,
    {
        proof { self.unfold(); self.height_fits(); }
        let Some(e) = &self.0 else {
            return Self::node(chain, tip, Self::default(), Self::default());
        };
        if chain < e.chain {
            let left = e.left.set(chain, tip);
            proof { left.height_fits(); }
            let out = Self::balance(e.chain, e.tip, left, e.right.clone());
            assert(out.contents() =~= self.contents().insert(chain, tip));
            out
        } else if chain > e.chain {
            let right = e.right.set(chain, tip);
            proof { right.height_fits(); }
            let out = Self::balance(e.chain, e.tip, e.left.clone(), right);
            assert(out.contents() =~= self.contents().insert(chain, tip));
            out
        } else {
            Self::node(chain, tip, e.left.clone(), e.right.clone())
        }
    }

    pub open spec fn raised(&self, chain: ChainId, position: NodeIdx) -> Map<ChainId, NodeIdx> {
        if self.contents().contains_key(chain) && position@ <= self.contents()[chain]@ {
            self.contents()
        } else { self.contents().insert(chain, position) }
    }

    /// Raise a chain prefix without lowering an existing tip.
    pub fn include(&mut self, chain: ChainId, position: NodeIdx)
        requires old(self).valid(),
            old(self).size() + (if old(self).contents().contains_key(chain) { 0nat } else { 1nat }) <= u32::MAX,
        ensures final(self).valid(), final(self).contents() == old(self).raised(chain, position),
            final(self).size() == old(self).size() + (if old(self).contents().contains_key(chain) { 0nat } else { 1nat }),
            old(self).depth() <= final(self).depth() <= old(self).depth() + 1,
    {
        if super::clock::contains(self.position(chain), position) {
            return;
        }
        *self = self.set(chain, position);
    }

    pub(crate) proof fn map_size(&self)
        requires self.ordered(),
        ensures self.contents().len() == self.size(),
        decreases self,
    {
        self.unfold();
        if let Some(e) = self.0 {
            e.left.map_size();
            e.right.map_size();
            assert(e.left.contents().dom().disjoint(e.right.contents().dom()));
            vstd::map_lib::lemma_disjoint_union_size(e.left.contents(), e.right.contents());
            assert(!e.left.contents().union_prefer_right(e.right.contents()).contains_key(e.chain));
        }
    }

    pub open spec fn prefix_union(out: Map<ChainId, NodeIdx>, base: Map<ChainId, NodeIdx>, entries: Seq<(ChainId, NodeIdx)>, n: nat) -> bool {
        &&& n <= entries.len()
        &&& forall|c: ChainId| out.contains_key(c) <==>
            base.contains_key(c) || exists|i: int| 0 <= i < n && entries[i].0 == c
        &&& forall|c: ChainId| base.contains_key(c) ==> out[c]@ >= base[c]@
        &&& forall|i: int| 0 <= i < n ==> out[entries[i].0]@ >= entries[i].1@
        &&& forall|c: ChainId| out.contains_key(c) ==>
            (base.contains_key(c) && out[c] == base[c]) ||
            exists|i: int| 0 <= i < n && entries[i].0 == c && out[c] == entries[i].1
    }

    pub open spec fn joins(out: Map<ChainId, NodeIdx>, a: Map<ChainId, NodeIdx>, b: Map<ChainId, NodeIdx>) -> bool {
        &&& out.dom() == a.dom().union(b.dom())
        &&& forall|c: ChainId| a.contains_key(c) ==> out[c]@ >= a[c]@
        &&& forall|c: ChainId| b.contains_key(c) ==> out[c]@ >= b[c]@
        &&& forall|c: ChainId| out.contains_key(c) ==>
            (a.contains_key(c) && out[c] == a[c]) || (b.contains_key(c) && out[c] == b[c])
    }

    proof fn prefix_union_step(&self, base: Map<ChainId, NodeIdx>, entries: Seq<(ChainId, NodeIdx)>, n: nat)
        requires Self::prefix_union(self.contents(), base, entries, n), n < entries.len(),
        ensures Self::prefix_union(self.raised(entries[n as int].0, entries[n as int].1), base, entries, n + 1),
    {
        let c = entries[n as int].0;
        let p = entries[n as int].1;
        let out = self.raised(c, p);
        assert forall|k: ChainId| out.contains_key(k) implies
            (base.contains_key(k) && out[k] == base[k]) ||
            exists|i: int| 0 <= i < n + 1 && entries[i].0 == k && out[k] == entries[i].1 by {
            if k == c && (!self.contents().contains_key(c) || self.contents()[c]@ < p@) {
                assert(entries[n as int].0 == k && out[k] == entries[n as int].1);
            } else if !(base.contains_key(k) && out[k] == base[k]) {
                let i = choose|i: int| 0 <= i < n && entries[i].0 == k && self.contents()[k] == entries[i].1;
                assert(out[k] == entries[i].1);
            }
        }
    }

    /// Pointwise maximum, retaining the larger side and visiting only the smaller.
    pub fn union(&self, other: &Self) -> (out: Self)
        requires self.valid(), other.valid(), self.contents().dom().union(other.contents().dom()).len() <= u32::MAX,
        ensures out.valid(), Self::joins(out.contents(), self.contents(), other.contents()),
            out.size() == self.contents().dom().union(other.contents().dom()).len(),
            out.size() <= self.size() + other.size(),
    {
        let (mut big, small) = if self.len() >= other.len() {
            (self.clone(), other)
        } else {
            (other.clone(), self)
        };
        let ghost base = big.contents();
        let ghost base_size = big.size();
        proof { small.sequence_correct(); }
        let ghost sequence = small.sequence();
        let mut iter = small.entries();
        let ghost mut done: nat = 0;
        loop
            invariant
                big.valid(), small.valid(), sequence == small.sequence(), sequence.len() == small.size(),
                self.contents().dom().union(other.contents().dom()).len() <= u32::MAX,
                (base == self.contents() && small.contents() == other.contents()) ||
                    (base == other.contents() && small.contents() == self.contents()),
                base_size + small.size() == self.size() + other.size(),
                done <= sequence.len(), iter.remaining() == sequence.skip(done as int),
                big.size() <= base_size + done,
                Self::prefix_union(big.contents(), base, sequence, done),
            ensures big.valid(), Self::joins(big.contents(), self.contents(), other.contents()),
                big.size() <= self.size() + other.size(),
            decreases iter.remaining().len(),
        {
            let next = iter.next();
            match next {
                None => {
                    proof {
                        small.sequence_correct();
                        assert(done == sequence.len());
                        assert(big.contents().dom() =~= base.dom().union(small.contents().dom()));
                        assert(Self::joins(big.contents(), base, small.contents()));
                        assert(base.dom().union(small.contents().dom()) =~= self.contents().dom().union(other.contents().dom()));
                        assert(Self::joins(big.contents(), self.contents(), other.contents()));
                    }
                    break;
                },
                Some((chain, tip)) => {
                    proof {
                        big.prefix_union_step(base, sequence, done);
                        small.sequence_correct();
                        big.map_size();
                        let next_keys = big.contents().dom().insert(chain);
                        let all_keys = self.contents().dom().union(other.contents().dom());
                        assert(next_keys.subset_of(all_keys));
                        vstd::set_lib::lemma_len_subset(next_keys, all_keys);
                    }
                    big.include(chain, tip);
                    proof { done = done + 1; }
                },
            }
        }
        proof { big.map_size(); }
        big
    }

    pub open spec fn sorted_entries(entries: Seq<(ChainId, NodeIdx)>) -> bool {
        forall|i: int, j: int| 0 <= i < j < entries.len() ==> entries[i].0 < entries[j].0
    }

    pub closed spec fn reconstructed_height(n: nat) -> nat
        decreases n,
    {
        if n == 0 { 0 } else { 1 + Self::reconstructed_height(n / 2) }
    }

    proof fn reconstructed_height_step(n: nat)
        ensures Self::reconstructed_height(n) <= Self::reconstructed_height(n + 1)
            <= Self::reconstructed_height(n) + 1,
        decreases n,
    {
        if n > 0 {
            Self::reconstructed_height_step(n / 2);
            assert(n / 2 <= (n + 1) / 2 <= n / 2 + 1);
        }
    }

    /// Build a balanced tree from strictly sorted entries in linear time.
    #[verifier::rlimit(30)]
    pub(crate) fn from_sorted(entries: &[(ChainId, NodeIdx)]) -> (out: Self)
        requires Self::sorted_entries(entries@), entries.len() <= u32::MAX,
        ensures out.valid(), out.sequence() == entries@, out.size() == entries.len(),
            out.depth() == Self::reconstructed_height(entries.len() as nat),
        decreases entries.len(),
    {
        if entries.is_empty() {
            return Self::default();
        }
        let mid = entries.len() / 2;
        let (chain, tip) = entries[mid];
        let left = Self::from_sorted(&entries[..mid]);
        let right = Self::from_sorted(&entries[mid + 1..]);
        proof {
            left.sequence_correct(); right.sequence_correct();
            left.height_fits(); right.height_fits();
            if mid > 0 { Self::reconstructed_height_step((mid - 1) as nat); }
            assert forall|c: ChainId| left.contents().contains_key(c) implies c < chain by {
                let i = choose|i: int| 0 <= i < left.sequence().len() && left.sequence()[i].0 == c;
                assert(entries@[i].0 == c);
            }
            assert forall|c: ChainId| right.contents().contains_key(c) implies chain < c by {
                let i = choose|i: int| 0 <= i < right.sequence().len() && right.sequence()[i].0 == c;
                assert(entries@[i + mid + 1].0 == c);
            }
        }
        let out = Self::node(chain, tip, left, right);
        assert(out.sequence() =~= entries@);
        out
    }

    proof fn tip_table_contents(&self, tips: Seq<NodeIdx>, c: ChainId)
        requires self.ordered(), tips.len() <= u32::MAX,
            self.sequence() == Seq::new(tips.len(), |i: int| (i as u32, tips[i])),
        ensures self.contents().contains_key(c) <==> c < tips.len(),
            c < tips.len() ==> self.contents().contains_key(c) && self.contents()[c] == tips[c as int],
    {
        self.sequence_correct();
        if c < tips.len() {
            assert(self.sequence()[c as int] == (c, tips[c as int]));
            assert(self.contents().contains_key(self.sequence()[c as int].0));
            assert(self.contents()[self.sequence()[c as int].0] == self.sequence()[c as int].1);
        } else if self.contents().contains_key(c) {
            let i = choose|i: int| 0 <= i < self.sequence().len() && self.sequence()[i].0 == c;
            assert(self.sequence()[i] == (i as u32, tips[i]));
        }
    }

    /// A clock containing every node, from a table of chain tips.
    pub fn from_tips(tips: &[NodeIdx]) -> (out: Self)
        requires tips.len() <= u32::MAX,
        ensures out.valid(), out.size() == tips.len(),
            forall|c: ChainId| #[trigger] out.contents().contains_key(c) <==> c < tips.len(),
            forall|c: ChainId| c < tips.len() ==> out.contents().contains_key(c) && #[trigger] out.contents()[c] == tips[c as int],
    {
        let mut entries = Vec::with_capacity(tips.len());
        let mut i = 0;
        while i < tips.len()
            invariant i <= tips.len(), tips.len() <= u32::MAX, entries.len() == i,
                forall|j: int| 0 <= j < entries.len() ==> entries[j] == (j as u32, tips[j]),
            decreases tips.len() - i,
        {
            entries.push((i as ChainId, tips[i]));
            i += 1;
        }
        let out = Self::from_sorted(&entries);
        proof {
            assert(out.sequence() =~= Seq::new(tips.len() as nat, |i: int| (i as u32, tips@[i])));
            assert forall|c: ChainId| #[trigger] out.contents().contains_key(c) <==> c < tips.len() by {
                out.tip_table_contents(tips@, c);
            }
            assert forall|c: ChainId| c < tips.len() implies out.contents().contains_key(c) && #[trigger] out.contents()[c] == tips[c as int] by {
                out.tip_table_contents(tips@, c);
            }
        }
        out
    }

    pub closed spec fn contents(&self) -> Map<ChainId, NodeIdx>
        decreases self,
    {
        match self.0 {
            None => Map::empty(),
            Some(e) => e.left.contents().union_prefer_right(e.right.contents()).insert(e.chain, e.tip),
        }
    }

    pub closed spec fn ordered(&self) -> bool
        decreases self,
    {
        match self.0 {
            None => true,
            Some(e) => e.left.ordered() && e.right.ordered()
                && (forall|c: ChainId| e.left.contents().contains_key(c) ==> c < e.chain)
                && (forall|c: ChainId| e.right.contents().contains_key(c) ==> e.chain < c),
        }
    }

    // Expose one structural step for subsequent construction/rotation proofs.
    pub(crate) proof fn unfold(&self)
        ensures match self.0 {
            None => self.ordered() && self.metadata() && self.balanced()
                && self.size() == 0 && self.depth() == 0
                && self.contents() == Map::<ChainId, NodeIdx>::empty(),
            Some(e) => self.size() == 1 + e.left.size() + e.right.size()
                && self.depth() == 1 + if e.left.depth() >= e.right.depth() { e.left.depth() } else { e.right.depth() }
                && (self.metadata() == (e.left.metadata() && e.right.metadata()
                    && e.len == self.size() && e.height == self.depth()))
                && (self.balanced() == (e.left.balanced() && e.right.balanced()
                    && e.left.depth() <= e.right.depth() + 1 && e.right.depth() <= e.left.depth() + 1))
                && self.contents() == e.left.contents().union_prefer_right(e.right.contents()).insert(e.chain, e.tip)
                && self.contents().contains_key(e.chain)
                && (forall|c: ChainId| e.left.contents().contains_key(c) ==> self.contents().contains_key(c))
                && (forall|c: ChainId| e.right.contents().contains_key(c) ==> self.contents().contains_key(c))
                && (self.ordered() == (e.left.ordered() && e.right.ordered()
                    && (forall|c: ChainId| e.left.contents().contains_key(c) ==> c < e.chain)
                    && (forall|c: ChainId| e.right.contents().contains_key(c) ==> e.chain < c))),
        },
    {}

    pub open spec fn lookup(&self, chain: ChainId) -> Option<NodeIdx> {
        if self.contents().contains_key(chain) { Some(self.contents()[chain]) } else { None }
    }

    pub fn position(&self, chain: ChainId) -> (out: Option<NodeIdx>)
        requires self.ordered(),
        ensures out == self.lookup(chain),
    {
        let mut node = self;
        loop
            invariant node.ordered(), node.lookup(chain) == self.lookup(chain),
            decreases node,
        {
            let Some(e) = &node.0 else {
                assert(node.contents() == Map::<ChainId, NodeIdx>::empty());
                return None;
            };
            assert(node.contents() == e.left.contents().union_prefer_right(e.right.contents()).insert(e.chain, e.tip));
            if chain < e.chain {
                assert(!e.right.contents().contains_key(chain));
                node = &e.left;
            } else if chain > e.chain {
                assert(!e.left.contents().contains_key(chain));
                node = &e.right;
            } else {
                return Some(e.tip);
            }
        }
    }

    pub fn contains(&self, chain: ChainId, position: NodeIdx) -> (out: bool)
        requires self.ordered(),
        ensures out == (self.contents().contains_key(chain) && position@ <= self.contents()[chain]@),
    {
        super::clock::contains(self.position(chain), position)
    }
}
} // verus!
