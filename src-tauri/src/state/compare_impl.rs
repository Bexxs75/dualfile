//! Commands for comparing folders, synchronising and the text diff. Part of `AppState`;
//! the stores and DTOs live in `crate::compare`.

use super::{AppState, JobPaths};
use crate::compare::{
    check_filter, diff_dto, sync_plan_dto, sync_plan_page, CompareFilterDto, CompareOptionsDto,
    ComparePageDto, DiffDto, DiffOptionsDto, SyncModeDto, SyncOptionsDto, SyncPlanDto,
    SyncPlanPageDto, MAX_COMPARE_ENTRIES,
};
use crate::convert::{vpath_text, JobTitleDto, UiError};
use crate::validate::{reject_archive_writes, reject_remote, MSG_NO_REMOTE_SYNC_TARGET};
use dualfile_core::compare::{self, CompareOptions, DiffOptions, SyncAction, SyncOptions};
use dualfile_core::ops::OpReport;
use dualfile_core::vfs::{EntryKind, LocalVfs, VPath};
use std::collections::{HashMap, HashSet};

/// The text diff answers in one piece, so its input is smaller than the core default
/// (5 MiB, 100 000 lines): at most 2 MiB and 50 000 lines per side.
pub const DIFF_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const DIFF_MAX_LINES: usize = 50_000;

fn not_found_compare() -> UiError {
    UiError::new(
        "notFound",
        "Das Vergleichsergebnis ist nicht (mehr) vorhanden",
    )
}

fn invalid_token() -> UiError {
    UiError::new(
        "invalidToken",
        "Die Bestätigung gehört nicht zu diesem Synchronisierungsplan",
    )
}

fn name_of(path: &VPath) -> String {
    path.path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.path().to_string_lossy().into_owned())
}

impl AppState {
    fn compare_root(&self, raw: &str) -> Result<VPath, UiError> {
        let path = self.path(raw)?;
        let entry = self.vfs_for_job(&path)?.stat(&path)?;
        if entry.kind != EntryKind::Dir {
            return Err(UiError::new(
                "notDirectory",
                format!("Der Pfad ist kein Ordner: {}", path.path().display()),
            ));
        }
        Ok(path)
    }

    /// Compares two folders (local or inside an archive) as a job. The job number is the id
    /// of the result: `compare_page` reads it once the job is done, `compare_close` frees it.
    /// A cancelled comparison leaves no result.
    pub fn compare_dirs(
        &self,
        left: &str,
        right: &str,
        options: &CompareOptionsDto,
    ) -> Result<u64, UiError> {
        let left = self.compare_root(left)?;
        let right = self.compare_root(right)?;
        let left_vfs = self.vfs_for_job(&left)?;
        let right_vfs = self.vfs_for_job(&right)?;
        let opts = CompareOptions {
            mode: options.mode.to_core(),
            recursive: options.recursive,
            max_entries: MAX_COMPARE_ENTRIES,
            ..CompareOptions::default()
        };
        let reservation = self.compares.reserve()?;
        let name = format!("Vergleichen: {} ↔ {}", name_of(&left), name_of(&right));
        self.submit_job(
            name,
            JobPaths::read(vec![vpath_text(&left), vpath_text(&right)]),
            move |resolver| {
                Box::new(move |job| {
                    let result =
                        compare::compare_dirs(&*left_vfs, &left, &*right_vfs, &right, &opts, job);
                    let report = OpReport {
                        cancelled: result.cancelled,
                        ..OpReport::default()
                    };
                    // Delivered before the job counts as finished, so the UI can read it then.
                    match resolver.job.get() {
                        Some(id) => reservation.finish(id, result),
                        None => drop(reservation),
                    }
                    report
                })
            },
        )
    }

    pub fn compare_page(
        &self,
        id: u64,
        offset: u64,
        limit: u64,
        filter: &CompareFilterDto,
    ) -> Result<ComparePageDto, UiError> {
        check_filter(filter)?;
        let stored = self.compares.get(id).ok_or_else(not_found_compare)?;
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(stored.page(id, offset, limit, filter))
    }

    /// Frees a result (and the sync plans made from it).
    pub fn compare_close(&self, id: u64) -> Result<(), UiError> {
        if !self.compares.close(id) {
            return Err(not_found_compare());
        }
        self.syncs.forget_compare(id);
        Ok(())
    }

    /// Builds a sync plan from a compare result and keeps it under its token. `selection`
    /// (paths relative to the target root, as `SyncActionDto.rel`) keeps only those actions
    /// plus the folders they need; without it the plan holds everything. Nothing is changed
    /// here.
    pub fn plan_sync(
        &self,
        compare_id: u64,
        mode: SyncModeDto,
        options: &SyncOptionsDto,
        selection: Option<&[String]>,
    ) -> Result<SyncPlanDto, UiError> {
        let stored = self
            .compares
            .get(compare_id)
            .ok_or_else(not_found_compare)?;
        let result = &stored.result;
        let target = match mode {
            SyncModeDto::RightToLeft => &result.left,
            SyncModeDto::LeftToRight | SyncModeDto::Mirror => &result.right,
        };
        // Archives are only ever a source, and synchronising writes to local folders only
        // (a server may be read from, never written to by a sync).
        reject_archive_writes(std::slice::from_ref(target))?;
        reject_remote(std::slice::from_ref(target), MSG_NO_REMOTE_SYNC_TARGET)?;
        let core_options = SyncOptions {
            replace_newer: options.replace_newer,
            ..SyncOptions::default()
        };
        let mut plan = compare::plan_sync(result, mode.to_core(), &core_options)?;
        if let Some(selection) = selection {
            plan = select_actions(&plan, selection)?;
        }
        let dto = sync_plan_dto(&plan);
        self.syncs.insert(plan, compare_id);
        Ok(dto)
    }

    /// More actions of a stored plan (the plan stays valid).
    pub fn sync_plan_page(
        &self,
        token: &str,
        offset: u64,
        limit: u64,
    ) -> Result<SyncPlanPageDto, UiError> {
        let stored = self.syncs.peek(token).ok_or_else(invalid_token)?;
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(sync_plan_page(&stored.plan, offset, limit))
    }

    /// Runs a confirmed plan as a job: only with the token of a stored plan, and a plan
    /// counts once. Conflicts go through the usual conflict bridge; removals only to the trash.
    pub fn run_sync(&self, token: &str) -> Result<u64, UiError> {
        let stored = self.syncs.take(token).ok_or_else(invalid_token)?;
        reject_archive_writes(std::slice::from_ref(&stored.plan.target))?;
        reject_remote(
            std::slice::from_ref(&stored.plan.target),
            MSG_NO_REMOTE_SYNC_TARGET,
        )?;
        let source_vfs = self.vfs_for_job(&stored.plan.source)?;
        let name = format!("Synchronisieren: {}", name_of(&stored.plan.source));
        let dirs = JobPaths {
            dirs: vec![vpath_text(&stored.plan.target)],
            touched: vec![
                vpath_text(&stored.plan.target),
                vpath_text(&stored.plan.source),
            ],
        };
        let title = JobTitleDto::new("sync", name_of(&stored.plan.source), 0);
        self.submit_job_titled(name, title, dirs, move |resolver| {
            Box::new(move |job| {
                let plan = &stored.plan;
                compare::run_sync(
                    &*source_vfs,
                    &LocalVfs,
                    plan,
                    plan.token,
                    Some(&resolver),
                    job,
                )
            })
        })
    }

    /// Line diff of two text files. The answer is limited (see `DIFF_MAX_BYTES`); binary or
    /// too large files are refused with `unsupported` and a message that says which.
    pub fn diff_files(
        &self,
        left: &str,
        right: &str,
        options: &DiffOptionsDto,
    ) -> Result<DiffDto, UiError> {
        let left = self.path(left)?;
        let right = self.path(right)?;
        let left_vfs = self.vfs_for_read(&left)?;
        let right_vfs = self.vfs_for_read(&right)?;
        let opts = DiffOptions {
            ignore_whitespace: options.ignore_whitespace,
            ignore_case: options.ignore_case,
            max_bytes: DIFF_MAX_BYTES,
            max_lines: DIFF_MAX_LINES,
        };
        let hunks = compare::diff_files(&*left_vfs, &left, &*right_vfs, &right, &opts)?;
        Ok(diff_dto(&hunks))
    }

    #[cfg(test)]
    pub fn open_compares(&self) -> usize {
        self.compares.len()
    }

    #[cfg(test)]
    pub fn open_sync_plans(&self) -> usize {
        self.syncs.len()
    }
}

/// Keeps the actions whose target path (relative to the target root) is in `selection`,
/// plus the `MakeDir` actions of their parent folders. Unknown entries are refused: the
/// selection must come from the plan the UI saw.
fn select_actions(
    plan: &compare::SyncPlan,
    selection: &[String],
) -> Result<compare::SyncPlan, UiError> {
    let rels: Vec<String> = plan
        .actions
        .iter()
        .map(|a| crate::compare::rel_to(&plan.target, a.target_path()))
        .collect();
    let known: HashSet<&str> = rels.iter().map(String::as_str).collect();
    if let Some(unknown) = selection.iter().find(|s| !known.contains(s.as_str())) {
        return Err(UiError::new(
            "invalidInput",
            format!("Die Auswahl enthält einen Eintrag, der nicht im Plan steht: {unknown}"),
        ));
    }
    let wanted: HashSet<&str> = selection.iter().map(String::as_str).collect();
    let dir_index: HashMap<&str, usize> = plan
        .actions
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, SyncAction::MakeDir { .. }))
        .map(|(i, _)| (rels[i].as_str(), i))
        .collect();
    let mut keep: HashSet<usize> = HashSet::new();
    for (i, rel) in rels.iter().enumerate() {
        if !wanted.contains(rel.as_str()) {
            continue;
        }
        keep.insert(i);
        // A copy needs its folder; removals do not create anything.
        if matches!(plan.actions[i], SyncAction::Trash { .. }) {
            continue;
        }
        let mut parent = rel.as_str();
        while let Some((up, _)) = parent.rsplit_once('/') {
            if let Some(index) = dir_index.get(up) {
                keep.insert(*index);
            }
            parent = up;
        }
    }
    let mut keep: Vec<usize> = keep.into_iter().collect();
    keep.sort_unstable();
    Ok(plan.select(&keep))
}

#[cfg(test)]
mod tests {
    use super::super::{AppState, EventSink};
    use crate::compare::{
        CompareFilterDto, CompareModeDto, CompareOptionsDto, CompareStateDto, DiffOptionsDto,
        SyncModeDto, SyncOptionsDto, MAX_COMPARES, MAX_SYNC_PLANS,
    };
    use crate::convert::{ConflictAnswerDto, CoreEventDto, PackFormatDto, ResolutionDto, UiError};
    use dualfile_core::jobs::{JobState, JobStatus};
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    struct ChannelSink(Mutex<Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send(event);
        }
    }

    struct Fixture {
        state: AppState,
        events: Receiver<CoreEventDto>,
        root: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("home")).unwrap();
        let (tx, events) = channel();
        let state = AppState::new(
            Arc::new(ChannelSink(Mutex::new(tx))),
            root.path().join("home"),
            root.path().join("config"),
        );
        Fixture {
            state,
            events,
            root,
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
    }

    impl Fixture {
        fn p(&self, rel: &str) -> String {
            self.root.path().join(rel).to_string_lossy().into_owned()
        }
        fn put(&self, rel: &str, content: &str, secs: u64) -> String {
            let path = self.root.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at(secs))
                .unwrap();
            self.p(rel)
        }
        fn read(&self, rel: &str) -> Option<String> {
            fs::read_to_string(self.root.path().join(rel)).ok()
        }
        fn exists(&self, rel: &str) -> bool {
            fs::symlink_metadata(self.root.path().join(rel)).is_ok()
        }
        fn finish(&self, id: u64) -> JobStatus {
            self.state.jobs.wait(id).expect("job exists")
        }
        /// Compares `l` with `r` and waits; returns the result id.
        fn compare(&self, l: &str, r: &str, mode: CompareModeDto) -> u64 {
            let options = CompareOptionsDto {
                mode,
                recursive: true,
            };
            let id = self
                .state
                .compare_dirs(&self.p(l), &self.p(r), &options)
                .unwrap();
            assert_eq!(self.finish(id).state, JobState::Done);
            id
        }
        /// Left/right trees with every simple state.
        fn mixed(&self) {
            self.put("l/same.txt", "same", 0);
            self.put("r/same.txt", "same", 0);
            self.put("l/only-left.txt", "ol", 0);
            self.put("r/only-right.txt", "or", 0);
            self.put("l/newer.txt", "new", 100);
            self.put("r/newer.txt", "old", 0);
            self.put("l/dir/inner.txt", "inner", 0);
            self.put("r/other.txt", "x", 0);
        }
    }

    fn all() -> CompareFilterDto {
        CompareFilterDto::default()
    }

    fn rels(page: &crate::compare::ComparePageDto) -> Vec<String> {
        page.entries.iter().map(|e| e.rel.clone()).collect()
    }

    fn zip(f: &Fixture, files: &[&str], name: &str) -> String {
        let sources: Vec<String> = files.iter().map(|rel| f.p(rel)).collect();
        let id = f
            .state
            .pack_archive(&sources, &f.p(""), name, PackFormatDto::Zip)
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        f.p(name)
    }

    fn arch(file: &str, inner: &str) -> String {
        format!(
            "archive:{}!{inner}",
            file.replace('%', "%25").replace('!', "%21")
        )
    }

    #[test]
    fn compare_results_are_read_in_pages_with_filter_and_summary() {
        let f = fixture();
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let page = f.state.compare_page(id, 0, 100, &all()).unwrap();
        assert_eq!(page.id, id);
        assert_eq!(page.total, 7);
        assert_eq!(
            rels(&page),
            [
                "dir",
                "dir/inner.txt",
                "newer.txt",
                "only-left.txt",
                "only-right.txt",
                "other.txt",
                "same.txt"
            ]
        );
        let s = &page.summary;
        assert_eq!((s.entries, s.counts.same), (7, 1));
        assert_eq!(s.counts.only_left, 3); // dir, dir/inner.txt, only-left.txt
        assert_eq!(s.counts.only_right, 2);
        assert_eq!(s.counts.left_newer, 1);
        assert!(!s.incomplete && !s.cancelled);
        assert_eq!(s.left, f.p("l"));

        // paging
        let first = f.state.compare_page(id, 0, 2, &all()).unwrap();
        let second = f.state.compare_page(id, 2, 2, &all()).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_eq!(second.entries.len(), 2);
        assert_eq!(first.total, page.total);
        assert_eq!(first.entries[1].rel, page.entries[1].rel);
        assert_eq!(second.entries[0].rel, page.entries[2].rel);
        assert!(f
            .state
            .compare_page(id, 1000, 10, &all())
            .unwrap()
            .entries
            .is_empty());

        // filter by state and by text (case-insensitive)
        let only = CompareFilterDto {
            states: Some(vec![CompareStateDto::OnlyRight]),
            text: None,
        };
        let page = f.state.compare_page(id, 0, 100, &only).unwrap();
        assert_eq!(rels(&page), ["only-right.txt", "other.txt"]);
        assert_eq!(page.summary.entries, 7);
        let text = CompareFilterDto {
            states: None,
            text: Some("INNER".into()),
        };
        assert_eq!(
            rels(&f.state.compare_page(id, 0, 100, &text).unwrap()),
            ["dir/inner.txt"]
        );
        let bad = CompareFilterDto {
            states: None,
            text: Some("a\0b".into()),
        };
        assert_eq!(
            f.state.compare_page(id, 0, 10, &bad).unwrap_err().kind,
            "invalidInput"
        );

        // a page is never larger than the limit allows, even when asked for more
        let huge = f.state.compare_page(id, 0, u64::MAX, &all()).unwrap();
        assert_eq!(huge.entries.len(), 7);
    }

    #[test]
    fn content_mode_finds_equal_size_differences() {
        let f = fixture();
        f.put("l/a.txt", "aaaa", 0);
        f.put("r/a.txt", "bbbb", 0);
        let by_size = f.compare("l", "r", CompareModeDto::SizeTime);
        assert_eq!(
            f.state
                .compare_page(by_size, 0, 10, &all())
                .unwrap()
                .summary
                .counts
                .same,
            1
        );
        let by_content = f.compare("l", "r", CompareModeDto::Content);
        let page = f.state.compare_page(by_content, 0, 10, &all()).unwrap();
        assert_eq!(page.summary.counts.different, 1);
        assert_eq!(page.summary.mode, "content");
    }

    #[test]
    fn compare_checks_paths_before_starting_a_job() {
        let f = fixture();
        f.put("l/a", "x", 0);
        f.put("file", "x", 0);
        let options = CompareOptionsDto::default();
        let l = f.p("l");
        for (a, b, kind) in [
            ("relativ", l.as_str(), "invalidPath"),
            (l.as_str(), "", "invalidPath"),
            (l.as_str(), "/a/../b", "invalidPath"),
            (l.as_str(), "/a\0b", "invalidPath"),
            (l.as_str(), &format!("/{}", "x".repeat(5000)), "invalidPath"),
            (l.as_str(), &f.p("fehlt"), "notFound"),
            (l.as_str(), &f.p("file"), "notDirectory"),
        ] {
            let error = f.state.compare_dirs(a, b, &options).unwrap_err();
            assert_eq!(error.kind, kind, "{a:?} / {b:?}: {error:?}");
        }
        assert!(f.state.jobs().is_empty());
        assert_eq!(f.state.open_compares(), 0);
        // the reservations of the refused calls were never taken
        assert!(f.state.compare_dirs(&l, &l, &options).is_ok());
    }

    #[test]
    fn closed_unknown_and_cancelled_results_are_gone_and_slots_are_released() {
        let f = fixture();
        f.put("l/a", "x", 0);
        f.put("r/a", "x", 0);
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        assert_eq!(f.state.open_compares(), 1);
        assert_eq!(
            f.state
                .compare_page(id + 100, 0, 1, &all())
                .unwrap_err()
                .kind,
            "notFound"
        );
        f.state.compare_close(id).unwrap();
        assert_eq!(f.state.open_compares(), 0);
        assert_eq!(
            f.state.compare_page(id, 0, 1, &all()).unwrap_err().kind,
            "notFound"
        );
        assert_eq!(f.state.compare_close(id).unwrap_err().kind, "notFound");
    }

    #[test]
    fn at_most_four_results_exist_at_the_same_time() {
        let f = fixture();
        f.put("l/a", "x", 0);
        f.put("r/a", "x", 0);
        let ids: Vec<u64> = (0..MAX_COMPARES)
            .map(|_| f.compare("l", "r", CompareModeDto::SizeTime))
            .collect();
        let error = f
            .state
            .compare_dirs(&f.p("l"), &f.p("r"), &CompareOptionsDto::default())
            .unwrap_err();
        assert_eq!(error.kind, "invalidInput");
        assert!(error.message.contains(&MAX_COMPARES.to_string()));
        f.state.compare_close(ids[0]).unwrap();
        let again = f.compare("l", "r", CompareModeDto::SizeTime);
        assert_eq!(f.state.open_compares(), MAX_COMPARES);
        assert!(f.state.compare_page(again, 0, 1, &all()).is_ok());
    }

    #[test]
    fn a_cancelled_compare_leaves_no_result_and_frees_its_slot() {
        let f = fixture();
        for i in 0..50 {
            f.put(&format!("l/d{i}/a"), "x", 0);
        }
        // Queue behind two blocking jobs so the compare is cancelled before it starts.
        let (started, release) = std::sync::mpsc::channel::<()>();
        let release = Arc::new(std::sync::Mutex::new(release));
        let mut blockers = Vec::new();
        for _ in 0..2 {
            let release = Arc::clone(&release);
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            blockers.push(
                f.state
                    .submit_job("block".into(), vec![], move |_| {
                        Box::new(move |_| {
                            let _ = tx.send(());
                            let _ = release
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(10));
                            Default::default()
                        })
                    })
                    .unwrap(),
            );
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let id = f
            .state
            .compare_dirs(&f.p("l"), &f.p("l"), &CompareOptionsDto::default())
            .unwrap();
        f.state.cancel_job(id).unwrap();
        for _ in &blockers {
            let _ = started.send(());
        }
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Cancelled);
        for id in blockers {
            f.finish(id);
        }
        assert_eq!(f.state.open_compares(), 0);
        // the slot is free again
        let ok = f.compare("l", "l", CompareModeDto::SizeTime);
        assert!(f.state.compare_page(ok, 0, 1, &all()).is_ok());
        assert_eq!(f.state.compares.len(), 1);
    }

    #[test]
    fn a_plan_runs_only_with_its_token_and_only_once() {
        let f = fixture();
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let plan = f
            .state
            .plan_sync(
                id,
                SyncModeDto::LeftToRight,
                &SyncOptionsDto::default(),
                None,
            )
            .unwrap();
        assert_eq!(plan.mode, "leftToRight");
        assert_eq!(plan.source, f.p("l"));
        assert_eq!(plan.target, f.p("r"));
        // only what is missing: folder `dir`, `dir/inner.txt`, `only-left.txt`
        assert_eq!(
            (plan.make_dirs, plan.copies, plan.replaces, plan.trashes),
            (1, 2, 0, 0)
        );
        assert_eq!(plan.total_actions, 3);
        assert!(!plan.actions_truncated && !plan.incomplete);
        assert_eq!(plan.token.len(), 32);
        let dir = plan.actions.iter().find(|a| a.kind == "makeDir").unwrap();
        assert_eq!(dir.rel, "dir");

        // nothing happened yet, and wrong or forged tokens change nothing
        assert!(!f.exists("r/only-left.txt"));
        for bad in [
            "",
            "00ff",
            "abc",
            &"0".repeat(32),
            &plan.token.to_uppercase(),
        ] {
            if bad != plan.token {
                assert_eq!(
                    f.state.run_sync(bad).unwrap_err().kind,
                    "invalidToken",
                    "{bad}"
                );
            }
        }
        assert!(!f.exists("r/only-left.txt"));
        assert_eq!(f.state.open_sync_plans(), 1);

        let job = f.state.run_sync(&plan.token).unwrap();
        let status = f.finish(job);
        assert_eq!(
            status.state,
            JobState::Done,
            "{:?}",
            status.report.as_ref().map(|r| r.failed.len())
        );
        assert_eq!(f.read("r/only-left.txt").as_deref(), Some("ol"));
        assert_eq!(f.read("r/dir/inner.txt").as_deref(), Some("inner"));
        // not planned, not touched
        assert_eq!(f.read("r/newer.txt").as_deref(), Some("old"));
        assert!(f.exists("r/only-right.txt") && f.exists("r/other.txt"));
        assert_eq!(
            f.state.jobs().iter().find(|j| j.id == job).unwrap().name,
            "Synchronisieren: l"
        );

        // the plan counts once
        assert_eq!(
            f.state.run_sync(&plan.token).unwrap_err().kind,
            "invalidToken"
        );
        assert_eq!(f.state.open_sync_plans(), 0);
    }

    #[test]
    fn selection_keeps_only_chosen_actions_with_their_folders() {
        let f = fixture();
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let sel = |rels: &[&str]| -> Result<_, UiError> {
            let rels: Vec<String> = rels.iter().map(|s| s.to_string()).collect();
            f.state.plan_sync(
                id,
                SyncModeDto::LeftToRight,
                &SyncOptionsDto::default(),
                Some(&rels),
            )
        };
        assert_eq!(sel(&["nicht-im-plan"]).unwrap_err().kind, "invalidInput");
        let plan = sel(&["dir/inner.txt"]).unwrap();
        // the folder it needs comes along, `only-left.txt` does not
        let kinds: Vec<(&str, &str)> = plan
            .actions
            .iter()
            .map(|a| (a.kind, a.rel.as_str()))
            .collect();
        assert_eq!(kinds, [("makeDir", "dir"), ("copy", "dir/inner.txt")]);
        let none = sel(&[]).unwrap();
        assert_eq!(none.total_actions, 0);
        let job = f.state.run_sync(&plan.token).unwrap();
        assert_eq!(f.finish(job).state, JobState::Done);
        assert!(f.exists("r/dir/inner.txt"));
        assert!(!f.exists("r/only-left.txt"));
        // the unused plan stays valid until used or dropped
        assert_eq!(f.state.open_sync_plans(), 1);
    }

    #[test]
    fn replacing_goes_through_the_plan_and_a_changed_target_is_skipped() {
        let f = fixture();
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let options = SyncOptionsDto {
            replace_newer: true,
        };
        let plan = f
            .state
            .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
            .unwrap();
        assert_eq!(plan.replaces, 1);
        let replace = plan.actions.iter().find(|a| a.kind == "replace").unwrap();
        assert_eq!(replace.rel, "newer.txt");
        // target replaced by a file of the same size and time after planning
        f.put("r/newer.txt", "own", 0);
        let job = f.state.run_sync(&plan.token).unwrap();
        let status = f.finish(job);
        assert_eq!(
            f.read("r/newer.txt").as_deref(),
            Some("own"),
            "{:?}",
            status.state
        );
        assert!(!f
            .events
            .try_iter()
            .any(|e| matches!(e, CoreEventDto::Conflict { .. })));
        let report = status.report.unwrap();
        assert!(report
            .skipped
            .iter()
            .any(|p| p.path().ends_with("newer.txt")));
        assert!(report
            .warnings
            .iter()
            .any(|(_, w)| w.starts_with("Übersprungen")));
        // the rest of the plan ran
        assert!(f.exists("r/only-left.txt"));
    }

    #[test]
    fn a_planned_overwrite_asks_through_the_conflict_bridge_and_obeys_the_answer() {
        for (resolution, expected) in [
            (ResolutionDto::Skip, "old"),
            (ResolutionDto::Overwrite, "new"),
        ] {
            let f = fixture();
            f.mixed();
            let id = f.compare("l", "r", CompareModeDto::SizeTime);
            let options = SyncOptionsDto {
                replace_newer: true,
            };
            let plan = f
                .state
                .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
                .unwrap();
            let job = f.state.run_sync(&plan.token).unwrap();
            let conflict = loop {
                match f
                    .events
                    .recv_timeout(Duration::from_secs(5))
                    .expect("conflict event")
                {
                    CoreEventDto::Conflict { conflict } => break conflict,
                    _ => continue,
                }
            };
            assert_eq!(conflict.job_id, job);
            assert_eq!(conflict.destination_path, f.p("r/newer.txt"));
            // while the question is open nothing was replaced
            assert_eq!(f.read("r/newer.txt").as_deref(), Some("old"));
            f.state
                .answer_conflict(
                    job,
                    ConflictAnswerDto {
                        resolution,
                        apply_to_all: false,
                    },
                )
                .unwrap();
            assert_eq!(f.finish(job).state, JobState::Done);
            assert_eq!(
                f.read("r/newer.txt").as_deref(),
                Some(expected),
                "{resolution:?}"
            );
            assert!(f.exists("r/only-left.txt"));
        }
    }

    #[test]
    fn mirror_sends_extras_to_the_trash_only() {
        static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let f = fixture();
        let data_home = f.root.path().join("data-home");
        fs::create_dir_all(&data_home).unwrap();
        let old = (std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"));
        std::env::set_var("XDG_DATA_HOME", &data_home);
        std::env::set_var("HOME", f.root.path().join("home"));
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let plan = f
            .state
            .plan_sync(id, SyncModeDto::Mirror, &SyncOptionsDto::default(), None)
            .unwrap();
        assert_eq!(plan.trashes, 2);
        let job = f.state.run_sync(&plan.token).unwrap();
        let status = f.finish(job);
        for (key, value) in [("XDG_DATA_HOME", old.0), ("HOME", old.1)] {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let failed = status.report.as_ref().map(|r| r.failed.len());
        assert_eq!(status.state, JobState::Done, "{failed:?}");
        assert!(!f.exists("r/only-right.txt") && !f.exists("r/other.txt"));
        assert!(f.exists("r/only-left.txt") && f.exists("r/dir/inner.txt"));
        let trashed = fs::read_dir(data_home.join("Trash/files")).unwrap().count();
        assert_eq!(trashed, 2);
    }

    #[test]
    fn an_archive_is_only_ever_a_source() {
        let f = fixture();
        f.put("src/a.txt", "alpha", 0);
        f.put("dst/b.txt", "beta", 0);
        let archive = zip(&f, &["src"], "pack.zip");
        let inside = arch(&archive, "/src");
        // archive left, folder right
        let id = f
            .state
            .compare_dirs(&inside, &f.p("dst"), &CompareOptionsDto::default())
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        let page = f.state.compare_page(id, 0, 10, &all()).unwrap();
        assert_eq!(page.summary.left, inside);
        assert_eq!(page.summary.counts.only_left, 1);
        let to_folder = f
            .state
            .plan_sync(
                id,
                SyncModeDto::LeftToRight,
                &SyncOptionsDto::default(),
                None,
            )
            .unwrap();
        // extracting from the archive works
        let job = f.state.run_sync(&to_folder.token).unwrap();
        assert_eq!(f.finish(job).state, JobState::Done);
        assert_eq!(f.read("dst/a.txt").as_deref(), Some("alpha"));
        // writing into the archive does not
        let error = f
            .state
            .plan_sync(
                id,
                SyncModeDto::RightToLeft,
                &SyncOptionsDto::default(),
                None,
            )
            .unwrap_err();
        assert_eq!(error.kind, "readOnly", "{error:?}");
        assert_eq!(error.message, "Archive sind schreibgeschützt");
        // Mirror writes to the right side (a folder here), never into the archive
        assert!(f
            .state
            .plan_sync(id, SyncModeDto::Mirror, &SyncOptionsDto::default(), None)
            .is_ok());
        // and with the archive on the right
        let id2 = f
            .state
            .compare_dirs(&f.p("dst"), &inside, &CompareOptionsDto::default())
            .unwrap();
        assert_eq!(f.finish(id2).state, JobState::Done);
        for mode in [SyncModeDto::LeftToRight, SyncModeDto::Mirror] {
            let error = f
                .state
                .plan_sync(id2, mode, &SyncOptionsDto::default(), None)
                .unwrap_err();
            assert_eq!(error.kind, "readOnly");
        }
        assert!(f
            .state
            .plan_sync(
                id2,
                SyncModeDto::RightToLeft,
                &SyncOptionsDto::default(),
                None
            )
            .is_ok());
        assert!(Path::new(&archive).exists());
    }

    #[test]
    fn closing_a_result_drops_its_plans_and_old_plans_are_forgotten() {
        let f = fixture();
        f.mixed();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let options = SyncOptionsDto::default();
        let plan = f
            .state
            .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
            .unwrap();
        f.state.compare_close(id).unwrap();
        assert_eq!(
            f.state.run_sync(&plan.token).unwrap_err().kind,
            "invalidToken"
        );
        assert_eq!(
            f.state
                .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
                .unwrap_err()
                .kind,
            "notFound"
        );

        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let first = f
            .state
            .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
            .unwrap();
        for _ in 0..MAX_SYNC_PLANS {
            f.state
                .plan_sync(id, SyncModeDto::LeftToRight, &options, None)
                .unwrap();
        }
        assert_eq!(f.state.open_sync_plans(), MAX_SYNC_PLANS);
        assert_eq!(
            f.state.run_sync(&first.token).unwrap_err().kind,
            "invalidToken"
        );
    }

    #[test]
    fn plan_actions_come_in_pages_and_the_preview_is_cut() {
        let f = fixture();
        for i in 0..250 {
            f.put(&format!("l/f{i:03}.txt"), "x", 0);
        }
        fs::create_dir_all(f.root.path().join("r")).unwrap();
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let plan = f
            .state
            .plan_sync(
                id,
                SyncModeDto::LeftToRight,
                &SyncOptionsDto::default(),
                None,
            )
            .unwrap();
        assert_eq!(plan.total_actions, 250);
        assert_eq!(plan.actions.len(), crate::compare::PLAN_PREVIEW);
        assert!(plan.actions_truncated);
        assert_eq!(plan.copies, 250);
        let page = f.state.sync_plan_page(&plan.token, 200, 100).unwrap();
        assert_eq!(
            (page.total, page.offset, page.actions.len()),
            (250, 200, 50)
        );
        assert_eq!(page.actions[0].index, 200);
        assert_eq!(page.actions[0].rel, "f200.txt");
        assert_eq!(
            f.state.sync_plan_page("nope", 0, 10).unwrap_err().kind,
            "invalidToken"
        );
        // reading a page does not use the plan up
        assert!(f.state.run_sync(&plan.token).is_ok());
    }

    #[test]
    fn an_unreadable_part_is_unknown_and_blocks_removals() {
        let f = fixture();
        f.put("l/ok.txt", "x", 0);
        f.put("l/locked/a.txt", "x", 0);
        f.put("r/locked/b.txt", "x", 0);
        f.put("r/extra.txt", "x", 0);
        let locked = f.root.path().join("l/locked");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok(); // root reads anyway
        let id = f.compare("l", "r", CompareModeDto::SizeTime);
        let page = f.state.compare_page(id, 0, 100, &all()).unwrap();
        let plan = f
            .state
            .plan_sync(id, SyncModeDto::Mirror, &SyncOptionsDto::default(), None)
            .unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(page.summary.incomplete);
            assert_eq!(page.summary.counts.unknown, 1);
            assert!(plan.incomplete);
            assert_eq!(plan.trashes, 0);
            assert!(plan.actions.iter().all(|a| !a.rel.starts_with("locked")));
        }
    }

    #[test]
    fn diff_files_reports_hunks_and_refuses_binary_and_large_files() {
        let f = fixture();
        let a = f.put("a.txt", "eins\nzwei\ndrei\n", 0);
        let b = f.put("b.txt", "eins\nZWEI\ndrei\nvier\n", 0);
        let diff = f
            .state
            .diff_files(&a, &b, &DiffOptionsDto::default())
            .unwrap();
        assert_eq!(diff.differences, 2);
        assert_eq!((diff.left_lines, diff.right_lines), (3, 4));
        assert_eq!(diff.hunks[0].kind, "equal");
        assert_eq!(diff.hunks[1].kind, "replace");
        assert_eq!(diff.hunks[1].left, ["zwei"]);
        assert_eq!(diff.hunks[1].right, ["ZWEI"]);
        let ignore = DiffOptionsDto {
            ignore_whitespace: false,
            ignore_case: true,
        };
        assert_eq!(f.state.diff_files(&a, &b, &ignore).unwrap().differences, 1);

        // binary
        let bin = f.p("bin.dat");
        fs::write(&bin, [1u8, 0, 2]).unwrap();
        let error = f
            .state
            .diff_files(&a, &bin, &DiffOptionsDto::default())
            .unwrap_err();
        assert_eq!(error.kind, "unsupported");
        assert_eq!(error.message, dualfile_core::compare::MSG_BINARY);
        // too large (over the limit of this command, below the core default)
        let big = f.p("big.txt");
        fs::write(&big, vec![b'x'; (super::DIFF_MAX_BYTES + 1) as usize]).unwrap();
        let error = f
            .state
            .diff_files(&big, &a, &DiffOptionsDto::default())
            .unwrap_err();
        assert_eq!(error.kind, "unsupported");
        assert_eq!(error.message, dualfile_core::compare::MSG_TOO_LARGE);
        // paths: validated, folders and missing files refused
        let opts = DiffOptionsDto::default();
        assert_eq!(
            f.state.diff_files("rel", &a, &opts).unwrap_err().kind,
            "invalidPath"
        );
        assert_eq!(
            f.state.diff_files(&a, "/a\0b", &opts).unwrap_err().kind,
            "invalidPath"
        );
        assert_eq!(
            f.state
                .diff_files(&a, &f.p("fehlt"), &opts)
                .unwrap_err()
                .kind,
            "notFound"
        );
        assert!(f.state.diff_files(&a, &f.p("home"), &opts).is_err());
        // a file inside an archive can be read
        f.put("src/in.txt", "eins\nzwei\ndrei\n", 0);
        let archive = zip(&f, &["src"], "p.zip");
        let inner = arch(&archive, "/src/in.txt");
        assert_eq!(
            f.state.diff_files(&inner, &a, &opts).unwrap().differences,
            0
        );
    }
}
