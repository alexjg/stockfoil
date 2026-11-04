//! Shared persistent clock representation and verified lookup.
//! Shared node construction and lookup. Rotation and update callers must still
//! establish the constructor's ordering and metadata-capacity preconditions.
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

impl Default for Clock {
    fn default() -> (out: Self)
        ensures out.ordered(), out.contents() == Map::<ChainId, NodeIdx>::empty(),
            out.metadata(), out.size() == 0, out.depth() == 0,
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
            out.ordered(), out.metadata(),
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
            None => self.ordered() && self.contents() == Map::<ChainId, NodeIdx>::empty(),
            Some(e) => self.contents() == e.left.contents().union_prefer_right(e.right.contents()).insert(e.chain, e.tip)
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
