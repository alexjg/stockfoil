//! Shared visibility classification and materialization batch advancement.
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Which side of a comparison a commit contributes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    Before,
    After,
    Both,
    Neither,
}

pub open spec fn before(vis: Vis) -> bool {
    matches!(vis, Vis::Before | Vis::Both)
}

pub open spec fn after(vis: Vis) -> bool {
    matches!(vis, Vis::After | Vis::Both)
}

pub fn classify(was_visible: bool, is_visible: bool) -> (result: Vis)
    ensures before(result) == was_visible, after(result) == is_visible,
{
    match (was_visible, is_visible) {
        (true, false) => Vis::Before,
        (false, true) => Vis::After,
        (true, true) => Vis::Both,
        (false, false) => Vis::Neither,
    }
}

/// Advance a captured traversal by at most `max`, without overflowing even if
/// the caller supplies usize::MAX. A zero budget leaves progress unchanged.
pub fn batch_end(next: usize, total: usize, max: usize) -> (out: usize)
    requires next <= total,
    ensures
        next <= out <= total,
        out - next == if max <= total - next { max as int } else { total - next },
        out == total <==> total - next <= max,
        max > 0 && next < total ==> next < out,
{
    let remaining = total - next;
    next + if max <= remaining { max } else { remaining }
}

} // verus!
