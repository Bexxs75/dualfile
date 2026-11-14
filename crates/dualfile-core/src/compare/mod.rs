//! Directory comparison, synchronisation plans and line-based text diff.
//!
//! Safety rule for everything in here: nothing is overwritten or removed unless it is an
//! entry of a confirmed plan, and removal only ever goes through `Vfs::trash`.

mod diff;
mod dirs;
mod sync;

#[cfg(test)]
mod tests;

pub use diff::{diff_files, diff_text, DiffHunk, DiffOptions, HunkKind, MSG_BINARY, MSG_TOO_LARGE};
pub use dirs::{
    compare_dirs, CompareEntry, CompareMode, CompareOptions, CompareResult, CompareState, SideInfo,
    TIME_TOLERANCE,
};
pub use sync::{
    plan_sync, plan_sync_with_job, run_sync, PlanNote, SyncAction, SyncMode, SyncOptions, SyncPlan,
    TargetVersion, MSG_REMOTE_TARGET,
};
