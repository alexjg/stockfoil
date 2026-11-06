//! Shared persistent clock representation and verified lookup.
//! Nonempty construction, rotations and enumeration remain proof obligations.
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
