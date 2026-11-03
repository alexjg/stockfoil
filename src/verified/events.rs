//! Shared visibility classification and materialization emission decisions.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;

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

pub fn should_emit(newly_delivered: bool, visibility: Vis) -> (result: bool)
    ensures result == (newly_delivered || before(visibility) != after(visibility)),
{
    newly_delivered || matches!(visibility, Vis::Before | Vis::After)
}

} // verus!
