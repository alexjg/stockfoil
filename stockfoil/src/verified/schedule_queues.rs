//! Production lazy-scheduler ready and blocked queues.
//! Causal readiness and the partition of pending frontiers are caller obligations.
use super::{clock::NodeIdx, commit_hash::CommitHash};
use std::collections::{BTreeSet, HashMap};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) type ReadyKey = (CommitHash, NodeIdx);

pub(crate) open spec fn queued(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>, node: NodeIdx) -> bool {
    (exists|key: ReadyKey| ready.contains(key) && key.1 == node)
        || (exists|dependency: NodeIdx| blocked.contains_key(dependency) && blocked[dependency].contains(node))
}

pub(crate) open spec fn valid(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>) -> bool {
    &&& forall|a: ReadyKey, b: ReadyKey| ready.contains(a) && ready.contains(b) && a.1 == b.1 ==> a == b
    &&& forall|d: NodeIdx| blocked.contains_key(d) ==> blocked[d].len() > 0 && blocked[d].no_duplicates()
    &&& forall|d: NodeIdx, i: int| blocked.contains_key(d) && 0 <= i < blocked[d].len() ==> d@ < blocked[d][i]@
    &&& forall|d: NodeIdx, e: NodeIdx, n: NodeIdx| blocked.contains_key(d) && blocked.contains_key(e)
        && blocked[d].contains(n) && blocked[e].contains(n) ==> d == e
    &&& forall|key: ReadyKey, d: NodeIdx| ready.contains(key) && blocked.contains_key(d) ==> !blocked[d].contains(key.1)
}

#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub(crate) struct Queues {
    ready: BTreeSet<ReadyKey>,
    blocked: HashMap<NodeIdx, Vec<NodeIdx>>,
}

impl Queues {
    pub(crate) closed spec fn ready(&self) -> Set<ReadyKey> { self.ready@ }
    pub(crate) closed spec fn blocked(&self) -> Map<NodeIdx, Seq<NodeIdx>> {
        self.blocked@.map_values(|row: Vec<NodeIdx>| row@)
    }
    pub(crate) open spec fn valid(&self) -> bool { valid(self.ready(), self.blocked()) }
    pub(crate) open spec fn queued(&self, node: NodeIdx) -> bool { queued(self.ready(), self.blocked(), node) }

    pub(crate) fn new() -> (out: Self)
        ensures out.ready() == Set::<ReadyKey>::empty(), out.blocked() == Map::<NodeIdx, Seq<NodeIdx>>::empty(), out.valid(),
    { Self { ready: BTreeSet::new(), blocked: HashMap::new() } }

    /// Caller has established that this unscheduled frontier is causally ready.
    pub(crate) fn make_ready(&mut self, hash: CommitHash, node: NodeIdx)
        requires old(self).valid(), !old(self).queued(node),
        ensures final(self).valid(), final(self).ready() == old(self).ready().insert((hash, node)),
            final(self).blocked() == old(self).blocked(),
            forall|n: NodeIdx| final(self).queued(n) == (old(self).queued(n) || n == node),
    {
        proof { super::stdlib::ready_key_model(); }
        let ghost ready = self.ready();
        let ghost blocked = self.blocked();
        self.ready.insert((hash, node));
        proof { ready_insert(ready, blocked, (hash, node)); }
    }

    /// Register exactly one waiter without losing earlier waiters on this dependency.
    pub(crate) fn block(&mut self, dependency: NodeIdx, waiter: NodeIdx)
        requires old(self).valid(), !old(self).queued(waiter), dependency@ < waiter@,
        ensures final(self).valid(), final(self).ready() == old(self).ready(),
            final(self).blocked() == old(self).blocked().insert(dependency,
                (if old(self).blocked().contains_key(dependency) { old(self).blocked()[dependency] } else { Seq::empty() }).push(waiter)),
            forall|n: NodeIdx| final(self).queued(n) == (old(self).queued(n) || n == waiter),
    {
        proof { super::stdlib::node_key_model(); }
        let ghost ready = self.ready();
        let ghost blocked = self.blocked();
        let row = self.blocked.entry(dependency).or_insert(Vec::new());
        row.push(waiter);
        assert(self.blocked() =~= blocked.insert(dependency,
            (if blocked.contains_key(dependency) { blocked[dependency] } else { Seq::empty() }).push(waiter)));
        proof { blocked_insert(ready, blocked, dependency, waiter); }
    }

    /// Remove the smallest bytewise hash, with local node index as the tie-breaker.
    pub(crate) fn pop_ready(&mut self) -> (out: Option<ReadyKey>)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).blocked() == old(self).blocked(),
            match out {
                None => old(self).ready().is_empty() && final(self).ready() == old(self).ready(),
                Some(key) => old(self).ready().contains(key) && final(self).ready() == old(self).ready().remove(key)
                    && (forall|other: ReadyKey| old(self).ready().contains(other) ==> super::stdlib::ready_le(key, other))
                    && !final(self).queued(key.1)
                    && (forall|n: NodeIdx| final(self).queued(n) == (old(self).queued(n) && n != key.1)),
            },
    {
        let ghost ready = self.ready();
        let ghost blocked = self.blocked();
        let out = super::stdlib::pop_ready(&mut self.ready);
        proof { if let Some(key) = out { ready_remove(ready, blocked, key); } }
        out
    }

    /// Detach this dependency's complete waiter list before rescheduling any of it.
    pub(crate) fn take_waiters(&mut self, dependency: NodeIdx) -> (out: Vec<NodeIdx>)
        requires old(self).valid(),
        ensures final(self).valid(), final(self).ready() == old(self).ready(),
            final(self).blocked() == old(self).blocked().remove(dependency),
            out@ == (if old(self).blocked().contains_key(dependency) { old(self).blocked()[dependency] } else { Seq::empty() }),
            out@.no_duplicates(),
            forall|n: NodeIdx| final(self).queued(n) == (old(self).queued(n) && !out@.contains(n)),
    {
        proof { super::stdlib::node_key_model(); }
        let ghost ready = self.ready();
        let ghost blocked = self.blocked();
        let out = match self.blocked.remove(&dependency) { Some(row) => row, None => Vec::new() };
        assert(self.blocked() =~= blocked.remove(dependency));
        proof { blocked_remove(ready, blocked, dependency); }
        out
    }
}

proof fn ready_insert(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>, key: ReadyKey)
    requires valid(ready, blocked), !queued(ready, blocked, key.1),
    ensures valid(ready.insert(key), blocked),
        forall|n: NodeIdx| queued(ready.insert(key), blocked, n) == (queued(ready, blocked, n) || n == key.1),
{
    assert forall|n: NodeIdx| queued(ready.insert(key), blocked, n) == (queued(ready, blocked, n) || n == key.1) by {
        if n == key.1 { assert(ready.insert(key).contains(key)); }
        if queued(ready.insert(key), blocked, n) && !(exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n)) {
            let k = choose|k: ReadyKey| ready.insert(key).contains(k) && k.1 == n;
        }
        if queued(ready, blocked, n) && !(exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n)) {
            let k = choose|k: ReadyKey| ready.contains(k) && k.1 == n;
            assert(ready.insert(key).contains(k));
        }
    }
}

proof fn ready_remove(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>, key: ReadyKey)
    requires valid(ready, blocked), ready.contains(key),
    ensures valid(ready.remove(key), blocked), !queued(ready.remove(key), blocked, key.1),
        forall|n: NodeIdx| queued(ready.remove(key), blocked, n) == (queued(ready, blocked, n) && n != key.1),
{
    assert forall|n: NodeIdx| queued(ready.remove(key), blocked, n) == (queued(ready, blocked, n) && n != key.1) by {
        if queued(ready, blocked, n) && n != key.1 && !(exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n)) {
            let k = choose|k: ReadyKey| ready.contains(k) && k.1 == n;
            assert(ready.remove(key).contains(k));
        }
        if queued(ready.remove(key), blocked, n) && !(exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n)) {
            let k = choose|k: ReadyKey| ready.remove(key).contains(k) && k.1 == n;
            assert(ready.contains(k));
        }
    }
}

proof fn blocked_insert(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>, dependency: NodeIdx, waiter: NodeIdx)
    requires valid(ready, blocked), !queued(ready, blocked, waiter), dependency@ < waiter@,
    ensures valid(ready, blocked.insert(dependency, (if blocked.contains_key(dependency) { blocked[dependency] } else { Seq::empty() }).push(waiter))),
        forall|n: NodeIdx| queued(ready, blocked.insert(dependency, (if blocked.contains_key(dependency) { blocked[dependency] } else { Seq::empty() }).push(waiter)), n)
            == (queued(ready, blocked, n) || n == waiter),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    let row = if blocked.contains_key(dependency) { blocked[dependency] } else { Seq::empty() };
    let next = blocked.insert(dependency, row.push(waiter));
    assert(!row.contains(waiter));
    assert(row.push(waiter).no_duplicates());
    assert forall|n: NodeIdx| queued(ready, next, n) == (queued(ready, blocked, n) || n == waiter) by {
        if exists|d: NodeIdx| next.contains_key(d) && next[d].contains(n) {
            let d = choose|d: NodeIdx| next.contains_key(d) && next[d].contains(n);
            if d != dependency { assert(blocked.contains_key(d) && blocked[d].contains(n)); }
        }
        if exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n) {
            let d = choose|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n);
            assert(next.contains_key(d) && next[d].contains(n));
        }
        if n == waiter { assert(next.contains_key(dependency)); assert(next[dependency].contains(n)); }
        if exists|key: ReadyKey| ready.contains(key) && key.1 == n {
            let key = choose|key: ReadyKey| ready.contains(key) && key.1 == n;
            assert(queued(ready, next, n));
        }
    }
}

proof fn blocked_remove(ready: Set<ReadyKey>, blocked: Map<NodeIdx, Seq<NodeIdx>>, dependency: NodeIdx)
    requires valid(ready, blocked),
    ensures valid(ready, blocked.remove(dependency)),
        forall|n: NodeIdx| queued(ready, blocked.remove(dependency), n)
            == (queued(ready, blocked, n) && !(blocked.contains_key(dependency) && blocked[dependency].contains(n))),
{
    let next = blocked.remove(dependency);
    assert forall|n: NodeIdx| queued(ready, next, n) == (queued(ready, blocked, n) && !(blocked.contains_key(dependency) && blocked[dependency].contains(n))) by {
        if exists|d: NodeIdx| next.contains_key(d) && next[d].contains(n) {
            let d = choose|d: NodeIdx| next.contains_key(d) && next[d].contains(n);
            assert(blocked.contains_key(d) && blocked[d].contains(n));
        }
        if (exists|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n)) && !(blocked.contains_key(dependency) && blocked[dependency].contains(n)) {
            let d = choose|d: NodeIdx| blocked.contains_key(d) && blocked[d].contains(n);
            assert(next.contains_key(d) && next[d].contains(n));
        }
    }
}

} // verus!

#[cfg(test)]
impl Queues {
    pub(crate) fn retained_counts(&self) -> (usize, usize) {
        (self.ready.len(), self.blocked.values().map(Vec::len).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn queue_transitions_match_independent_maps() {
        let mut queues = Queues::new();
        let mut ready: Vec<([u8; 32], u32)> = Vec::new();
        let mut blocked: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut state = 0x4d595df4d0f33173u64;
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..4000 {
            let node = (random() % 64) as u32;
            match random() % 4 {
                0 | 1 => {
                    if ready.iter().any(|&(_, n)| n == node)
                        || blocked.values().any(|row| row.contains(&node))
                    {
                        continue;
                    }
                    if node > 0 && random() % 2 == 0 {
                        let dependency = (random() % node as u64) as u32;
                        queues.block(NodeIdx::new(dependency), NodeIdx::new(node));
                        blocked.entry(dependency).or_default().push(node);
                    } else {
                        let mut hash = [0; 32];
                        hash[(random() % 32) as usize] = (random() % 256) as u8;
                        queues.make_ready(CommitHash(hash), NodeIdx::new(node));
                        ready.push((hash, node));
                    }
                }
                2 => {
                    ready.sort_unstable();
                    let expected = if ready.is_empty() {
                        None
                    } else {
                        Some(ready.remove(0))
                    };
                    assert_eq!(
                        queues
                            .pop_ready()
                            .map(|(hash, idx)| (hash.0, idx.index() as u32)),
                        expected
                    );
                }
                _ => {
                    let expected = blocked.remove(&node).unwrap_or_default();
                    assert_eq!(
                        queues
                            .take_waiters(NodeIdx::new(node))
                            .iter()
                            .map(|n| n.index() as u32)
                            .collect::<Vec<_>>(),
                        expected
                    );
                }
            }
            assert_eq!(
                queues.retained_counts(),
                (ready.len(), blocked.values().map(Vec::len).sum())
            );
        }
        ready.sort_unstable();
        for expected in ready {
            assert_eq!(
                queues.pop_ready().map(|(h, n)| (h.0, n.index() as u32)),
                Some(expected)
            );
        }
        assert!(queues.pop_ready().is_none());
        for (dependency, expected) in blocked {
            assert_eq!(
                queues
                    .take_waiters(NodeIdx::new(dependency))
                    .iter()
                    .map(|n| n.index() as u32)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(queues.retained_counts(), (0, 0));
    }

    #[test]
    fn ready_order_uses_all_hash_bytes_and_node_ties() {
        let mut queues = Queues::new();
        let mut expected = Vec::new();
        let mut node = 1;
        for offset in (0..32).rev() {
            for byte in [1, 128, 255] {
                let mut hash = [0; 32];
                hash[offset] = byte;
                queues.make_ready(CommitHash(hash), NodeIdx::new(node));
                expected.push((hash, node));
                node += 1;
            }
        }
        for node in [u32::MAX, 65536, 256, 0] {
            queues.make_ready(CommitHash([7; 32]), NodeIdx::new(node));
            expected.push(([7; 32], node));
        }
        expected.sort_unstable();
        for (hash, node) in expected {
            assert_eq!(
                queues.pop_ready(),
                Some((CommitHash(hash), NodeIdx::new(node)))
            );
        }
        assert!(queues.pop_ready().is_none());
    }
}
