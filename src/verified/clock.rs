//! Shared clock kernels. Ordinary Cargo builds erase specifications and proofs.
//! The graph wrappers remain responsible for establishing the preconditions.
use alloc::vec::Vec;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;

verus! {

pub type Entry = (usize, usize);

pub open spec fn sorted(entries: Seq<Entry>) -> bool {
    forall|i: int, j: int| 0 <= i < j < entries.len() ==>
        entries[i].0 < entries[j].0
}

pub open spec fn member(entries: Seq<Entry>, chain: usize, position: usize) -> bool {
    exists|i: int| 0 <= i < entries.len()
        && entries[i].0 == chain && position <= entries[i].1
}

pub open spec fn below(tip: Option<usize>, position: usize) -> bool {
    match tip {
        None => false,
        Some(tip) => position <= tip,
    }
}

pub fn max_position(left: usize, right: usize) -> (result: usize)
    ensures
        result == if left >= right { left } else { right },
        forall|p: usize| below(Some(result), p) == (p <= left || p <= right),
{
    if left >= right { left } else { right }
}

proof fn member_push(entries: Seq<Entry>, entry: Entry, c: usize, p: usize)
    ensures member(entries.push(entry), c, p)
        == (member(entries, c, p) || (entry.0 == c && p <= entry.1)),
{
    let extended = entries.push(entry);
    if member(extended, c, p) {
        let i = choose|i: int| 0 <= i < extended.len()
            && extended[i].0 == c && p <= extended[i].1;
        if i < entries.len() {
            assert(entries[i] == extended[i]);
            assert(member(entries, c, p));
        }
    }
    if member(entries, c, p) {
        let i = choose|i: int| 0 <= i < entries.len()
            && entries[i].0 == c && p <= entries[i].1;
        assert(extended[i] == entries[i]);
        assert(member(extended, c, p));
    }
    if entry.0 == c && p <= entry.1 {
        assert(extended[entries.len() as int] == entry);
        assert(member(extended, c, p));
    }
}

// The per-coordinate operation performed by ClockBuilder::include.
#[cfg(verus_keep_ghost)]
pub fn include(known: Option<usize>, position: usize) -> (result: Option<usize>)
    ensures
        result == Some(match known {
            None => position,
            Some(tip) => if tip >= position { tip } else { position },
        }),
        forall|p: usize| below(result, p) == (below(known, p) || p <= position),
{
    match known {
        None => Some(position),
        Some(tip) => Some(max_position(tip, position)),
    }
}

// One coordinate of ClockBuilder::merge, including absent coordinates.
#[cfg(verus_keep_ghost)]
pub fn union(left: Option<usize>, right: Option<usize>) -> (result: Option<usize>)
    ensures
        forall|p: usize| below(result, p) == (below(left, p) || below(right, p)),
{
    match right {
        None => left,
        Some(position) => include(left, position),
    }
}

// The min/None operation in AncestorClock::mask. Unbounded permission is
// represented by Some(history_tip), exactly as participation() does.
pub fn cap(tip: usize, bound: Option<usize>) -> (result: Option<usize>)
    ensures
        result == match bound {
            None => None,
            Some(limit) => Some(if tip <= limit { tip } else { limit }),
        },
        forall|p: usize| below(result, p) == (p <= tip && below(bound, p)),
{
    match bound {
        None => None,
        Some(limit) => Some(if tip <= limit { tip } else { limit }),
    }
}

// Explicit binary search in place of std's binary_search_by_key adapter.
// The proof establishes lookup soundness AND completeness, not just bounds safety.
pub fn position(entries: &Vec<Entry>, chain: usize) -> (result: Option<usize>)
    requires sorted(entries@),
    ensures
        match result {
            None => forall|i: int| 0 <= i < entries.len() ==> entries[i].0 != chain,
            Some(tip) => exists|i: int| 0 <= i < entries.len()
                && entries[i] == (chain, tip),
        },
        forall|p: usize| below(result, p) == member(entries@, chain, p),
{
    let mut lo: usize = 0;
    let mut hi: usize = entries.len();
    while lo < hi
        invariant
            sorted(entries@),
            0 <= lo <= hi <= entries.len(),
            forall|i: int| 0 <= i < lo ==> entries[i].0 < chain,
            forall|i: int| hi <= i < entries.len() ==> entries[i].0 > chain,
        decreases hi - lo,
    {
        let mid = lo + (hi - lo) / 2;
        let entry = entries[mid];
        if entry.0 < chain {
            assert forall|i: int| 0 <= i <= mid implies entries[i].0 < chain by {
                if i < mid { assert(entries[i].0 < entries[mid as int].0); }
            }
            lo = mid + 1;
        } else if entry.0 > chain {
            assert forall|i: int| mid <= i < entries.len() implies entries[i].0 > chain by {
                if i > mid { assert(entries[mid as int].0 < entries[i].0); }
            }
            hi = mid;
        } else {
            assert forall|i: int| 0 <= i < entries.len() && entries[i].0 == chain
                implies i == mid by {
                if i < mid { assert(entries[i].0 < entries[mid as int].0); }
                if i > mid { assert(entries[mid as int].0 < entries[i].0); }
            }
            return Some(entry.1);
        }
    }
    None
}

pub fn contains(entries: &Vec<Entry>, chain: usize, p: usize) -> (result: bool)
    requires sorted(entries@),
    ensures result == member(entries@, chain, p),
{
    match position(entries, chain) {
        None => false,
        Some(tip) => p <= tip,
    }
}

pub open spec fn permitted(
    entries: Seq<Entry>, bounds: Seq<Option<usize>>, chain: usize, p: usize,
) -> bool {
    exists|i: int| 0 <= i < entries.len() && entries[i].0 == chain
        && p <= entries[i].1 && below(bounds[i], p)
}

// Bounds are the results of the production FnMut, captured once per input entry.
// This verifies coordinate filtering; computing those bounds is a separate task.
#[cfg(verus_keep_ghost)]
pub fn mask(entries: &Vec<Entry>, bounds: &Vec<Option<usize>>) -> (out: Vec<Entry>)
    requires sorted(entries@), bounds.len() == entries.len(),
    ensures
        sorted(out@),
        out.len() <= entries.len(),
        forall|c: usize, p: usize| member(out@, c, p) == permitted(entries@, bounds@, c, p),
        forall|c: usize, p: usize| member(out@, c, p) ==> member(entries@, c, p),
{
    let mut out: Vec<Entry> = Vec::new();
    let mut i: usize = 0;
    while i < entries.len()
        invariant
            sorted(entries@), bounds.len() == entries.len(),
            i <= entries.len(), out.len() <= i, sorted(out@),
            forall|j: int, k: int| 0 <= j < out.len() && i <= k < entries.len()
                ==> out[j].0 < entries[k].0,
            forall|c: usize, p: usize| member(out@, c, p)
                == permitted(entries@.take(i as int), bounds@, c, p),
        decreases entries.len() - i,
    {
        let entry = entries[i];
        let bound = bounds[i];
        let ghost previous = out@;
        let capped = cap(entry.1, bound);
        match capped {
            None => {},
            Some(limit) => out.push((entry.0, limit)),
        }
        assert forall|c: usize, p: usize|
            member(out@, c, p)
            == permitted(entries@.take(i as int + 1), bounds@, c, p) by {
            assert(member(previous, c, p)
                == permitted(entries@.take(i as int), bounds@, c, p));
            assert(below(capped, p) == (p <= entry.1 && below(bound, p)));
            if let Some(limit) = capped {
                member_push(previous, (entry.0, limit), c, p);
            }
            assert(member(out@, c, p)
                == (member(previous, c, p) || (entry.0 == c && below(capped, p))));
            assert(permitted(entries@.take(i as int + 1), bounds@, c, p)
                == (permitted(entries@.take(i as int), bounds@, c, p)
                    || (entry.0 == c && p <= entry.1 && below(bound, p))));
        }
        i += 1;
    }
    out
}

// A down-set on one chain is exactly a prefix. This is the representation
// theorem needed to relate clock membership to graph reachability. It assumes
// the graph's chain-order invariant; it does not prove assign_chain maintains it.
pub proof fn chain_prefix_is_exact(
    chain_nodes: Set<usize>, history: Set<usize>, tip: usize,
)
    requires
        chain_nodes.contains(tip), history.contains(tip),
        forall|p: usize| chain_nodes.contains(p) && history.contains(p) ==> p <= tip,
        forall|p: usize, q: usize| chain_nodes.contains(p) && chain_nodes.contains(q)
            && p <= q && history.contains(q) ==> history.contains(p),
    ensures
        forall|p: usize| chain_nodes.contains(p) ==> (history.contains(p) == (p <= tip)),
{
    assert forall|p: usize| chain_nodes.contains(p)
        implies (history.contains(p) == (p <= tip)) by {
        if p <= tip {
            assert(history.contains(tip));
        }
    }
}

pub proof fn absent_is_not_zero()
    ensures !below(None, 0), below(Some(0), 0), !below(Some(0), 1),
{}

} // verus!
