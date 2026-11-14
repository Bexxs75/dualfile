//! Directory comparison, sync plans and text diff as the UI sees them: DTOs plus the two
//! stores that keep results and plans on the Rust side. The UI only ever holds numbers
//! (result id) and tokens; what a sync does is decided by the stored plan alone.

use crate::convert::{kind_name, millis, token_text, vpath_text, UiError};
use dualfile_core::compare::{
    CompareEntry, CompareMode, CompareResult, CompareState, DiffHunk, HunkKind, PlanNote, SideInfo,
    SyncAction, SyncMode, SyncPlan,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Finished or running compare results kept at the same time.
pub const MAX_COMPARES: usize = 4;
/// Unconfirmed sync plans kept at most; the oldest is dropped first.
pub const MAX_SYNC_PLANS: usize = 8;
/// Largest page of `compare_page` / `sync_plan_page`.
pub const MAX_PAGE: usize = 1000;
/// Actions and notes that come with the plan itself (the rest is fetched in pages).
pub const PLAN_PREVIEW: usize = 200;
/// Most entries a compare result holds (more makes it incomplete, never silent).
pub const MAX_COMPARE_ENTRIES: usize = 500_000;
/// Longest text filter in bytes.
pub const MAX_FILTER_BYTES: usize = 1024;

// ---------------------------------------------------------------- Options

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CompareModeDto {
    #[default]
    SizeTime,
    Content,
}

impl CompareModeDto {
    pub fn to_core(self) -> CompareMode {
        match self {
            Self::SizeTime => CompareMode::SizeTime,
            Self::Content => CompareMode::Content,
        }
    }
    fn name(mode: CompareMode) -> &'static str {
        match mode {
            CompareMode::SizeTime => "sizeTime",
            CompareMode::Content => "content",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompareOptionsDto {
    pub mode: CompareModeDto,
    pub recursive: bool,
}

impl Default for CompareOptionsDto {
    fn default() -> Self {
        Self {
            mode: CompareModeDto::SizeTime,
            recursive: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SyncModeDto {
    LeftToRight,
    RightToLeft,
    Mirror,
}

impl SyncModeDto {
    pub fn to_core(self) -> SyncMode {
        match self {
            Self::LeftToRight => SyncMode::LeftToRight,
            Self::RightToLeft => SyncMode::RightToLeft,
            Self::Mirror => SyncMode::Mirror,
        }
    }
    fn name(mode: SyncMode) -> &'static str {
        match mode {
            SyncMode::LeftToRight => "leftToRight",
            SyncMode::RightToLeft => "rightToLeft",
            SyncMode::Mirror => "mirror",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncOptionsDto {
    pub replace_newer: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffOptionsDto {
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
}

// ---------------------------------------------------------------- Compare results

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareStateDto {
    OnlyLeft,
    OnlyRight,
    Same,
    LeftNewer,
    RightNewer,
    Different,
    TypeMismatch,
    Unknown,
}

impl From<CompareState> for CompareStateDto {
    fn from(state: CompareState) -> Self {
        match state {
            CompareState::OnlyLeft => Self::OnlyLeft,
            CompareState::OnlyRight => Self::OnlyRight,
            CompareState::Same => Self::Same,
            CompareState::LeftNewer => Self::LeftNewer,
            CompareState::RightNewer => Self::RightNewer,
            CompareState::Different => Self::Different,
            CompareState::TypeMismatch => Self::TypeMismatch,
            CompareState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompareFilterDto {
    /// Only these states; `None` or empty: all.
    pub states: Option<Vec<CompareStateDto>>,
    /// Case-insensitive part of the relative path.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SideDto {
    pub kind: &'static str,
    pub size: u64,
    pub modified: Option<i64>,
    pub link_target: Option<String>,
}

fn side_dto(side: &SideInfo) -> SideDto {
    SideDto {
        kind: kind_name(side.kind),
        size: side.size,
        modified: side.modified.map(millis),
        link_target: side.link_target.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareEntryDto {
    pub rel: String,
    pub state: CompareStateDto,
    pub left: Option<SideDto>,
    pub right: Option<SideDto>,
    pub note: Option<String>,
    pub listed: bool,
}

fn entry_dto(entry: &CompareEntry) -> CompareEntryDto {
    CompareEntryDto {
        rel: entry.rel.clone(),
        state: entry.state.into(),
        left: entry.left.as_ref().map(side_dto),
        right: entry.right.as_ref().map(side_dto),
        note: entry.note.clone(),
        listed: entry.listed,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareCountsDto {
    pub only_left: u64,
    pub only_right: u64,
    pub same: u64,
    pub left_newer: u64,
    pub right_newer: u64,
    pub different: u64,
    pub type_mismatch: u64,
    pub unknown: u64,
}

fn counts_of(result: &CompareResult) -> CompareCountsDto {
    let mut counts = CompareCountsDto::default();
    for entry in &result.entries {
        let slot = match entry.state {
            CompareState::OnlyLeft => &mut counts.only_left,
            CompareState::OnlyRight => &mut counts.only_right,
            CompareState::Same => &mut counts.same,
            CompareState::LeftNewer => &mut counts.left_newer,
            CompareState::RightNewer => &mut counts.right_newer,
            CompareState::Different => &mut counts.different,
            CompareState::TypeMismatch => &mut counts.type_mismatch,
            CompareState::Unknown => &mut counts.unknown,
        };
        *slot += 1;
    }
    counts
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSummaryDto {
    pub left: String,
    pub right: String,
    pub mode: &'static str,
    pub recursive: bool,
    /// All entries of the result (not only those matching a filter).
    pub entries: u64,
    pub counts: CompareCountsDto,
    /// Something was not examined: sync from this result never removes anything.
    pub incomplete: bool,
    pub cancelled: bool,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparePageDto {
    pub id: u64,
    pub offset: u64,
    /// Entries matching the filter.
    pub total: u64,
    pub entries: Vec<CompareEntryDto>,
    pub summary: CompareSummaryDto,
}

/// A finished result with its counts (computed once).
pub struct StoredCompare {
    pub result: CompareResult,
    pub counts: CompareCountsDto,
}

impl StoredCompare {
    pub fn new(result: CompareResult) -> Self {
        let counts = counts_of(&result);
        Self { result, counts }
    }

    pub fn summary(&self) -> CompareSummaryDto {
        let r = &self.result;
        CompareSummaryDto {
            left: vpath_text(&r.left),
            right: vpath_text(&r.right),
            mode: CompareModeDto::name(r.mode),
            recursive: r.recursive,
            entries: r.entries.len() as u64,
            counts: self.counts.clone(),
            incomplete: r.incomplete,
            cancelled: r.cancelled,
            issues: r.issues.clone(),
        }
    }

    pub fn page(
        &self,
        id: u64,
        offset: usize,
        limit: usize,
        filter: &CompareFilterDto,
    ) -> ComparePageDto {
        let limit = limit.clamp(1, MAX_PAGE);
        let states = filter.states.as_deref().filter(|s| !s.is_empty());
        let text = filter
            .text
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase);
        let matches = |entry: &&CompareEntry| {
            states.is_none_or(|s| s.contains(&entry.state.into()))
                && text
                    .as_deref()
                    .is_none_or(|t| entry.rel.to_lowercase().contains(t))
        };
        let mut total = 0u64;
        let mut entries = Vec::new();
        for entry in self.result.entries.iter().filter(matches) {
            if total as usize >= offset && entries.len() < limit {
                entries.push(entry_dto(entry));
            }
            total += 1;
        }
        ComparePageDto {
            id,
            offset: offset as u64,
            total,
            entries,
            summary: self.summary(),
        }
    }
}

/// Validates a filter coming from the UI.
pub fn check_filter(filter: &CompareFilterDto) -> Result<(), UiError> {
    if let Some(text) = &filter.text {
        if text.len() > MAX_FILTER_BYTES || text.contains('\0') {
            return Err(UiError::new("invalidInput", "Der Filtertext ist ungültig"));
        }
    }
    Ok(())
}

#[derive(Default)]
struct CompareInner {
    results: HashMap<u64, Arc<StoredCompare>>,
    /// Compares that were reserved but have not delivered their result yet.
    running: usize,
}

/// Compare results by job number. At most `MAX_COMPARES` exist (running ones count); the
/// UI frees one with `compare_close`.
#[derive(Default)]
pub struct CompareStore {
    inner: Mutex<CompareInner>,
}

impl CompareStore {
    fn guard(&self) -> std::sync::MutexGuard<'_, CompareInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes one of the slots or refuses. The slot is given back when the guard is
    /// dropped without a result (job cancelled while queued, panic, ...).
    pub fn reserve(self: &Arc<Self>) -> Result<Reservation, UiError> {
        let mut inner = self.guard();
        if inner.results.len() + inner.running >= MAX_COMPARES {
            return Err(UiError::new(
                "invalidInput",
                format!(
                    "Es sind schon {MAX_COMPARES} Vergleiche offen; bitte erst einen schließen"
                ),
            ));
        }
        inner.running += 1;
        Ok(Reservation {
            store: Arc::clone(self),
            used: false,
        })
    }

    pub fn get(&self, id: u64) -> Option<Arc<StoredCompare>> {
        self.guard().results.get(&id).cloned()
    }

    /// Frees the result; `false` if there was none.
    pub fn close(&self, id: u64) -> bool {
        self.guard().results.remove(&id).is_some()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.guard().results.len()
    }
}

pub struct Reservation {
    store: Arc<CompareStore>,
    used: bool,
}

impl Reservation {
    /// Hands the result over under its job number. A cancelled compare is not kept.
    pub fn finish(mut self, id: u64, result: CompareResult) {
        self.used = true;
        let mut inner = self.store.guard();
        inner.running = inner.running.saturating_sub(1);
        if !result.cancelled {
            inner
                .results
                .insert(id, Arc::new(StoredCompare::new(result)));
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.used {
            let mut inner = self.store.guard();
            inner.running = inner.running.saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------- Sync plans

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncActionDto {
    /// Position in the plan.
    pub index: u64,
    /// `makeDir`, `copy` (target is new), `replace` (target is overwritten), `trash`.
    pub kind: &'static str,
    /// Path of the changed entry, relative to the target root.
    pub rel: String,
    /// What is copied / what is removed: `file`, `dir`, `symlink`.
    pub entry_kind: &'static str,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNoteDto {
    pub rel: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlanDto {
    pub token: String,
    pub mode: &'static str,
    pub source: String,
    pub target: String,
    pub make_dirs: u64,
    pub copies: u64,
    pub replaces: u64,
    pub trashes: u64,
    pub bytes: u64,
    pub total_actions: u64,
    /// The first `PLAN_PREVIEW` actions; `sync_plan_page` fetches the rest.
    pub actions: Vec<SyncActionDto>,
    pub actions_truncated: bool,
    pub notes: Vec<PlanNoteDto>,
    pub notes_total: u64,
    /// The compare result was incomplete: no removals are planned.
    pub incomplete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlanPageDto {
    pub total: u64,
    pub offset: u64,
    pub actions: Vec<SyncActionDto>,
}

/// Path of `path` below the target root, `/`-separated (empty for the root itself).
pub fn rel_to(root: &dualfile_core::vfs::VPath, path: &dualfile_core::vfs::VPath) -> String {
    match path.path().strip_prefix(root.path()) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => vpath_text(path),
    }
}

pub fn action_dto(plan: &SyncPlan, index: usize, action: &SyncAction) -> SyncActionDto {
    let rel = rel_to(&plan.target, action.target_path());
    match action {
        SyncAction::MakeDir { .. } => SyncActionDto {
            index: index as u64,
            kind: "makeDir",
            rel,
            entry_kind: "dir",
            size: 0,
        },
        SyncAction::Copy {
            source_kind,
            size,
            overwrite,
            ..
        } => SyncActionDto {
            index: index as u64,
            kind: if *overwrite { "replace" } else { "copy" },
            rel,
            entry_kind: kind_name(*source_kind),
            size: *size,
        },
        SyncAction::Trash { expected, .. } => SyncActionDto {
            index: index as u64,
            kind: "trash",
            rel,
            entry_kind: kind_name(expected.kind),
            size: expected.size,
        },
    }
}

fn note_dto(note: &PlanNote) -> PlanNoteDto {
    PlanNoteDto {
        rel: note.rel.clone(),
        reason: note.reason.clone(),
    }
}

pub fn sync_plan_dto(plan: &SyncPlan) -> SyncPlanDto {
    let actions: Vec<_> = plan
        .actions
        .iter()
        .enumerate()
        .take(PLAN_PREVIEW)
        .map(|(i, a)| action_dto(plan, i, a))
        .collect();
    SyncPlanDto {
        token: token_text(plan.token),
        mode: SyncModeDto::name(plan.mode),
        source: vpath_text(&plan.source),
        target: vpath_text(&plan.target),
        make_dirs: plan.make_dirs as u64,
        copies: plan.copies as u64,
        replaces: plan.replaces as u64,
        trashes: plan.trashes as u64,
        bytes: plan.bytes,
        total_actions: plan.actions.len() as u64,
        actions_truncated: plan.actions.len() > actions.len(),
        actions,
        notes: plan.notes.iter().take(PLAN_PREVIEW).map(note_dto).collect(),
        notes_total: plan.notes.len() as u64,
        incomplete: plan.incomplete,
    }
}

pub fn sync_plan_page(plan: &SyncPlan, offset: usize, limit: usize) -> SyncPlanPageDto {
    let limit = limit.clamp(1, MAX_PAGE);
    SyncPlanPageDto {
        total: plan.actions.len() as u64,
        offset: offset as u64,
        actions: plan
            .actions
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit)
            .map(|(i, a)| action_dto(plan, i, a))
            .collect(),
    }
}

/// A plan plus what the confirmed job needs later.
pub struct StoredSync {
    pub plan: SyncPlan,
    /// The compare result the plan came from; closing it drops the plan.
    pub compare_id: u64,
}

#[derive(Default)]
pub struct SyncStore {
    plans: Mutex<VecDeque<(String, Arc<StoredSync>)>>,
}

impl SyncStore {
    fn guard(&self) -> std::sync::MutexGuard<'_, VecDeque<(String, Arc<StoredSync>)>> {
        self.plans.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, plan: SyncPlan, compare_id: u64) -> String {
        let token = token_text(plan.token);
        let mut plans = self.guard();
        plans.retain(|(known, _)| *known != token);
        plans.push_back((token.clone(), Arc::new(StoredSync { plan, compare_id })));
        while plans.len() > MAX_SYNC_PLANS {
            plans.pop_front();
        }
        token
    }

    /// Looks at a plan without using it up.
    pub fn peek(&self, token: &str) -> Option<Arc<StoredSync>> {
        self.guard()
            .iter()
            .find(|(known, _)| known == token)
            .map(|(_, stored)| Arc::clone(stored))
    }

    /// Removes and returns the plan; a second call with the same token returns `None`.
    pub fn take(&self, token: &str) -> Option<Arc<StoredSync>> {
        let mut plans = self.guard();
        let index = plans.iter().position(|(known, _)| known == token)?;
        plans.remove(index).map(|(_, stored)| stored)
    }

    /// Drops the plans that came from a compare result.
    pub fn forget_compare(&self, compare_id: u64) {
        self.guard().retain(|(_, s)| s.compare_id != compare_id);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.guard().len()
    }
}

// ---------------------------------------------------------------- Text diff

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunkDto {
    pub kind: &'static str,
    pub left_start: u64,
    pub left: Vec<String>,
    pub right_start: u64,
    pub right: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffDto {
    pub hunks: Vec<DiffHunkDto>,
    /// Hunks that are not `equal`.
    pub differences: u64,
    pub left_lines: u64,
    pub right_lines: u64,
}

pub fn diff_dto(hunks: &[DiffHunk]) -> DiffDto {
    let mut differences = 0;
    let (mut left_lines, mut right_lines) = (0, 0);
    let hunks = hunks
        .iter()
        .map(|hunk| {
            let kind = match hunk.kind {
                HunkKind::Equal => "equal",
                HunkKind::Delete => "delete",
                HunkKind::Insert => "insert",
                HunkKind::Replace => "replace",
            };
            if hunk.kind != HunkKind::Equal {
                differences += 1;
            }
            left_lines += hunk.left.len() as u64;
            right_lines += hunk.right.len() as u64;
            DiffHunkDto {
                kind,
                left_start: hunk.left_start as u64,
                left: hunk.left.clone(),
                right_start: hunk.right_start as u64,
                right: hunk.right.clone(),
            }
        })
        .collect();
    DiffDto {
        hunks,
        differences,
        left_lines,
        right_lines,
    }
}
