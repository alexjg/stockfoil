//! Checked captured materialization planning.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) use crate::verified::change_plan::plan_changes_at;
