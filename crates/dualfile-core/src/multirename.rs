//! Multi-rename (Phase 3): renames several entries of ONE folder in two phases, so swaps,
//! chains and cycles work. Nothing is ever overwritten: every rename uses `NoReplace`.
//!
//! The core does not trust the UI's preview: names, duplicates, collisions and sources are
//! checked again here. Problems of the plan itself (invalid or duplicate names, collision with
//! an entry that is NOT being renamed) reject the whole run before anything is touched. A
//! missing source only fails that one item; the rest runs.

use crate::error::{Error, Result};
use crate::names::validate_name;
use crate::ops::{JobHandle, OpReport, ProgressEvent, RenameResult};
use crate::vfs::{EntryKind, ReplaceMode, VPath, Vfs};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

const TEMP_PREFIX: &str = ".dualfile-rename-";
/// Tries for a free intermediate name before the item is given up.
const TEMP_ATTEMPTS: usize = 8;

#[derive(Default)]
pub struct RenameManyReport {
    /// Renamed entries (old and new path), in input order.
    pub renamed: Vec<RenameResult>,
    /// Failed items by their OLD path. A `RenameLeftover` error means the entry still lives
    /// under its intermediate name.
    pub failed: Vec<(VPath, Error)>,
    /// Items that were not changed on purpose (same name, cancelled, or the run was rejected
    /// because of another item); old paths.
    pub skipped: Vec<VPath>,
    pub cancelled: bool,
}

impl RenameManyReport {
    /// For the job system: `done` holds the NEW paths, `failed` and `skipped` the old ones.
    pub fn into_op_report(self) -> OpReport {
        OpReport {
            done: self.renamed.into_iter().map(|r| r.new).collect(),
            failed: self.failed,
            skipped: self.skipped,
            warnings: Vec::new(),
            cancelled: self.cancelled,
            ..OpReport::default()
        }
    }
}

struct Mover {
    old: VPath,
    new: VPath,
    old_name: String,
}

struct Staged {
    mover: Mover,
    temp: VPath,
}

/// Random enough to avoid clashes without a new dependency: std's per-instance random hasher
/// keys plus a process-wide counter. Collisions are caught anyway (`NoReplace`, retry).
fn temp_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("{TEMP_PREFIX}{:016x}", hasher.finish())
}

fn stage(v: &dyn Vfs, folder: &VPath, old: &VPath) -> Result<VPath> {
    let mut last = Error::InvalidInput("Zwischenname nicht verfügbar");
    for _ in 0..TEMP_ATTEMPTS {
        let temp = folder.join(&temp_name())?;
        match v.rename(old, &temp, ReplaceMode::NoReplace) {
            Ok(()) => return Ok(temp),
            Err(error @ Error::Exists(_)) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

fn exists(v: &dyn Vfs, path: &VPath) -> Result<bool> {
    match v.stat(path) {
        Ok(_) => Ok(true),
        Err(Error::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Puts a staged entry back under its original name (never replacing anything). If that
/// fails, the item is reported as left over under its intermediate name; returns whether
/// the entry is back.
fn restore(v: &dyn Vfs, staged: &Staged, report: &mut RenameManyReport, job: &JobHandle) -> bool {
    if v.rename(&staged.temp, &staged.mover.old, ReplaceMode::NoReplace)
        .is_ok()
    {
        return true;
    }
    job.emit(ProgressEvent::ItemFailed(staged.mover.old.clone()));
    report.failed.push((
        staged.mover.old.clone(),
        Error::RenameLeftover(staged.temp.path().to_path_buf()),
    ));
    false
}

/// Renames `items` (old name, new name) inside `folder`. `Err` only for problems with the
/// call itself (no items, folder unusable); everything else is reported per item.
pub fn rename_many(
    v: &dyn Vfs,
    folder: &VPath,
    items: &[(String, String)],
    job: &JobHandle,
) -> Result<RenameManyReport> {
    if items.is_empty() {
        return Err(Error::InvalidInput("Keine Einträge zum Umbenennen"));
    }
    if v.stat(folder)?.kind == EntryKind::File {
        return Err(Error::NotADirectory(folder.path().to_path_buf()));
    }
    let mut report = RenameManyReport::default();
    // Problems of the plan itself; any of them rejects the whole run.
    let mut plan_errors: Vec<(VPath, Error)> = Vec::new();
    let mut candidates: Vec<Mover> = Vec::new();
    let mut seen_old: HashSet<&str> = HashSet::new();
    let mut unchanged: Vec<VPath> = Vec::new();

    for (old_name, new_name) in items {
        let checked = validate_name(old_name)
            .and_then(|()| validate_name(new_name))
            .and_then(|()| {
                if seen_old.insert(old_name.as_str()) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput("Doppelter Quellname in der Auswahl"))
                }
            })
            .and_then(|()| Ok((folder.join(old_name)?, folder.join(new_name)?)));
        match checked {
            Err(error) => {
                // The old path may be invalid itself; use the folder then.
                let at = folder.join(old_name).unwrap_or_else(|_| folder.clone());
                plan_errors.push((at, error));
            }
            Ok((old, _)) if old_name == new_name => unchanged.push(old),
            Ok((old, new)) => candidates.push(Mover {
                old,
                new,
                old_name: old_name.clone(),
            }),
        }
    }

    // Duplicate targets: every item involved is rejected.
    let mut by_target: HashMap<std::path::PathBuf, usize> = HashMap::new();
    for mover in &candidates {
        *by_target.entry(mover.new.path().to_path_buf()).or_default() += 1;
    }
    let (clashing, candidates): (Vec<Mover>, Vec<Mover>) = candidates
        .into_iter()
        .partition(|m| by_target.get(m.new.path()).copied().unwrap_or(0) > 1);
    for mover in clashing {
        plan_errors.push((
            mover.old,
            Error::InvalidInput("Doppelter Zielname in der Auswahl"),
        ));
    }

    // Missing sources fail alone; the rest runs.
    let mut movers: Vec<Mover> = Vec::new();
    for mover in candidates {
        match v.stat(&mover.old) {
            Ok(_) => movers.push(mover),
            Err(error) => {
                job.emit(ProgressEvent::ItemFailed(mover.old.clone()));
                report.failed.push((mover.old, error));
            }
        }
    }

    // A target must be free or be the old name of an entry that is renamed as well.
    let renamed_names: HashSet<&str> = movers.iter().map(|m| m.old_name.as_str()).collect();
    for mover in &movers {
        let target_name = mover
            .new
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if renamed_names.contains(target_name) {
            continue;
        }
        match exists(v, &mover.new) {
            Ok(false) => {}
            Ok(true) => plan_errors.push((
                mover.old.clone(),
                Error::Exists(mover.new.path().to_path_buf()),
            )),
            Err(error) => plan_errors.push((mover.old.clone(), error)),
        }
    }

    if !plan_errors.is_empty() {
        let rejected: HashSet<VPath> = plan_errors.iter().map(|(p, _)| p.clone()).collect();
        for (path, _) in &plan_errors {
            job.emit(ProgressEvent::ItemFailed(path.clone()));
        }
        report.failed.extend(plan_errors);
        report.skipped = unchanged;
        report.skipped.extend(
            movers
                .into_iter()
                .map(|m| m.old)
                .filter(|p| !rejected.contains(p)),
        );
        return Ok(report);
    }
    report.skipped = unchanged;

    // Phase 1: everything moves to a unique intermediate name, so no target is occupied by
    // an entry that is about to be renamed.
    let mut staged: Vec<Staged> = Vec::new();
    let mut pending = movers.into_iter();
    for mover in pending.by_ref() {
        if job.is_cancelled() {
            report.cancelled = true;
            report.skipped.push(mover.old);
            break;
        }
        match stage(v, folder, &mover.old) {
            Ok(temp) => staged.push(Staged { mover, temp }),
            Err(error) => {
                job.emit(ProgressEvent::ItemFailed(mover.old.clone()));
                report.failed.push((mover.old, error));
            }
        }
    }
    report.skipped.extend(pending.map(|m| m.old));

    // Phase 2: intermediate name -> final name. A cancel from here on puts the remaining
    // entries back, so no intermediate names are left behind.
    for entry in staged {
        if report.cancelled || job.is_cancelled() {
            report.cancelled = true;
            if restore(v, &entry, &mut report, job) {
                report.skipped.push(entry.mover.old);
            }
            continue;
        }
        match v.rename(&entry.temp, &entry.mover.new, ReplaceMode::NoReplace) {
            Ok(()) => {
                job.emit(ProgressEvent::ItemDone(entry.mover.new.clone()));
                report.renamed.push(RenameResult {
                    old: entry.mover.old,
                    new: entry.mover.new,
                });
            }
            Err(error) => {
                // A failed restore already reported the item (leftover); otherwise report
                // the reason why the final rename failed.
                if restore(v, &entry, &mut report, job) {
                    job.emit(ProgressEvent::ItemFailed(entry.mover.old.clone()));
                    report.failed.push((entry.mover.old, error));
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{Caps, Entry, LocalVfs, WriteFinish, WriteMode};
    use std::fs;
    use std::io::Read;
    use std::path::Path;
    use std::sync::Mutex;

    fn setup(names: &[&str]) -> (tempfile::TempDir, VPath) {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            fs::write(dir.path().join(name), name).unwrap();
        }
        let folder = VPath::local(dir.path()).unwrap();
        (dir, folder)
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    fn run(folder: &VPath, items: &[(&str, &str)]) -> RenameManyReport {
        rename_many(&LocalVfs, folder, &pairs(items), &JobHandle::silent()).unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut all: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        all.sort();
        all
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap()
    }

    fn no_temp_names(dir: &Path) {
        assert!(
            names(dir).iter().all(|n| !n.starts_with(TEMP_PREFIX)),
            "{:?}",
            names(dir)
        );
    }

    /// Delegates to `LocalVfs`; `before_rename` sees every rename call (1-based count).
    struct Hooked<F: Fn(usize, &VPath, &VPath) + Send + Sync> {
        count: Mutex<usize>,
        before_rename: F,
    }
    impl<F: Fn(usize, &VPath, &VPath) + Send + Sync> Vfs for Hooked<F> {
        fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
            LocalVfs.list(p)
        }
        fn stat(&self, p: &VPath) -> Result<Entry> {
            LocalVfs.stat(p)
        }
        fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
            LocalVfs.open_read(p)
        }
        fn create_write(&self, p: &VPath, m: WriteMode) -> Result<Box<dyn WriteFinish>> {
            LocalVfs.create_write(p, m)
        }
        fn mkdir(&self, p: &VPath) -> Result<()> {
            LocalVfs.mkdir(p)
        }
        fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
            let n = {
                let mut count = self.count.lock().unwrap();
                *count += 1;
                *count
            };
            (self.before_rename)(n, a, b);
            LocalVfs.rename(a, b, m)
        }
        fn read_link(&self, p: &VPath) -> Result<String> {
            LocalVfs.read_link(p)
        }
        fn create_symlink(&self, t: &str, p: &VPath) -> Result<()> {
            LocalVfs.create_symlink(t, p)
        }
        fn trash(&self, p: &VPath) -> Result<()> {
            LocalVfs.trash(p)
        }
        fn remove_permanent(&self, p: &VPath) -> Result<()> {
            LocalVfs.remove_permanent(p)
        }
        fn capabilities(&self) -> Caps {
            LocalVfs.capabilities()
        }
    }
    fn hooked<F: Fn(usize, &VPath, &VPath) + Send + Sync>(f: F) -> Hooked<F> {
        Hooked {
            count: Mutex::new(0),
            before_rename: f,
        }
    }

    #[test]
    fn renames_simply_and_reports_old_and_new_paths() {
        let (dir, folder) = setup(&["a.txt", "keep.txt"]);
        let report = run(&folder, &[("a.txt", "b.txt")]);
        assert!(report.failed.is_empty() && !report.cancelled);
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(report.renamed[0].old.path(), dir.path().join("a.txt"));
        assert_eq!(report.renamed[0].new.path(), dir.path().join("b.txt"));
        assert_eq!(names(dir.path()), ["b.txt", "keep.txt"]);
        assert_eq!(read(dir.path(), "b.txt"), "a.txt");
    }

    #[test]
    fn swaps_two_entries() {
        let (dir, folder) = setup(&["a", "b"]);
        let report = run(&folder, &[("a", "b"), ("b", "a")]);
        assert!(report.failed.is_empty());
        assert_eq!(report.renamed.len(), 2);
        assert_eq!(read(dir.path(), "a"), "b");
        assert_eq!(read(dir.path(), "b"), "a");
        no_temp_names(dir.path());
    }

    #[test]
    fn renames_a_chain_a_to_b_to_c() {
        let (dir, folder) = setup(&["a", "b"]);
        let report = run(&folder, &[("a", "b"), ("b", "c")]);
        assert!(report.failed.is_empty());
        assert_eq!(names(dir.path()), ["b", "c"]);
        assert_eq!(read(dir.path(), "b"), "a");
        assert_eq!(read(dir.path(), "c"), "b");
    }

    #[test]
    fn renames_a_cycle_of_three() {
        let (dir, folder) = setup(&["a", "b", "c"]);
        let report = run(&folder, &[("a", "b"), ("b", "c"), ("c", "a")]);
        assert!(report.failed.is_empty());
        assert_eq!(read(dir.path(), "b"), "a");
        assert_eq!(read(dir.path(), "c"), "b");
        assert_eq!(read(dir.path(), "a"), "c");
        no_temp_names(dir.path());
    }

    #[test]
    fn works_for_folders_too() {
        let (dir, folder) = setup(&[]);
        fs::create_dir(dir.path().join("x")).unwrap();
        fs::write(dir.path().join("x").join("inner"), "i").unwrap();
        let report = run(&folder, &[("x", "y")]);
        assert!(report.failed.is_empty());
        assert_eq!(read(&dir.path().join("y"), "inner"), "i");
    }

    #[test]
    fn rejects_collision_with_a_not_renamed_entry_and_changes_nothing() {
        let (dir, folder) = setup(&["a", "b", "other"]);
        let report = run(&folder, &[("a", "other"), ("b", "c")]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0.path(), dir.path().join("a"));
        assert!(matches!(report.failed[0].1, Error::Exists(_)));
        assert!(report.renamed.is_empty());
        assert_eq!(
            report.skipped.len(),
            1,
            "the innocent item is not run either"
        );
        assert_eq!(names(dir.path()), ["a", "b", "other"]);
        assert_eq!(read(dir.path(), "other"), "other");
    }

    #[test]
    fn an_unchanged_entry_counts_as_not_renamed_for_collisions() {
        let (dir, folder) = setup(&["a", "b"]);
        let report = run(&folder, &[("a", "b"), ("b", "b")]);
        assert_eq!(report.failed.len(), 1);
        assert!(report.renamed.is_empty());
        assert_eq!(names(dir.path()), ["a", "b"]);
    }

    #[test]
    fn rejects_duplicate_target_names_and_changes_nothing() {
        let (dir, folder) = setup(&["a", "b", "c"]);
        let report = run(&folder, &[("a", "x"), ("b", "x"), ("c", "z")]);
        assert_eq!(report.failed.len(), 2);
        assert!(report
            .failed
            .iter()
            .all(|(_, e)| matches!(e, Error::InvalidInput(_))));
        assert!(report.renamed.is_empty());
        assert_eq!(names(dir.path()), ["a", "b", "c"]);
    }

    #[test]
    fn rejects_duplicate_source_names() {
        let (dir, folder) = setup(&["a"]);
        let report = run(&folder, &[("a", "x"), ("a", "y")]);
        assert_eq!(report.failed.len(), 1);
        assert!(report.renamed.is_empty());
        assert_eq!(names(dir.path()), ["a"]);
    }

    #[test]
    fn rejects_invalid_names_and_changes_nothing() {
        let (dir, folder) = setup(&["a", "b"]);
        let long = "x".repeat(256);
        for bad in ["", ".", "..", "x/y", "x\0y", "  ", long.as_str()] {
            let report = run(&folder, &[("a", bad), ("b", "ok")]);
            assert_eq!(report.failed.len(), 1, "{bad:?}");
            assert!(
                matches!(report.failed[0].1, Error::InvalidName(_)),
                "{bad:?}"
            );
            assert!(report.renamed.is_empty(), "{bad:?}");
            assert_eq!(names(dir.path()), ["a", "b"], "{bad:?}");
        }
        // Invalid source names (traversal) are refused as well.
        let report = run(&folder, &[("../a", "x")]);
        assert!(matches!(report.failed[0].1, Error::InvalidName(_)));
        assert_eq!(names(dir.path()), ["a", "b"]);
    }

    #[test]
    fn a_missing_source_fails_alone_and_the_rest_runs() {
        let (dir, folder) = setup(&["a", "b"]);
        let report = run(&folder, &[("a", "x"), ("gone", "y"), ("b", "z")]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0.path(), dir.path().join("gone"));
        assert!(matches!(report.failed[0].1, Error::NotFound(_)));
        assert_eq!(report.renamed.len(), 2);
        assert_eq!(names(dir.path()), ["x", "z"]);
    }

    #[test]
    fn rejects_empty_input_and_a_folder_that_is_a_file() {
        let (dir, folder) = setup(&["a"]);
        assert!(rename_many(&LocalVfs, &folder, &[], &JobHandle::silent()).is_err());
        let file = VPath::local(dir.path().join("a")).unwrap();
        let result = rename_many(
            &LocalVfs,
            &file,
            &pairs(&[("a", "b")]),
            &JobHandle::silent(),
        );
        assert!(matches!(result, Err(Error::NotADirectory(_))));
        let gone = VPath::local(dir.path().join("fehlt")).unwrap();
        let result = rename_many(
            &LocalVfs,
            &gone,
            &pairs(&[("a", "b")]),
            &JobHandle::silent(),
        );
        assert!(matches!(result, Err(Error::NotFound(_))));
    }

    #[test]
    fn a_target_created_after_the_check_is_never_overwritten_and_the_source_is_restored() {
        let (dir, folder) = setup(&["a", "b"]);
        let intruder = dir.path().join("c");
        let vfs = hooked({
            let intruder = intruder.clone();
            move |_, _, to| {
                if to.path() == intruder {
                    fs::write(&intruder, "intruder").unwrap();
                }
            }
        });
        let report = rename_many(
            &vfs,
            &folder,
            &pairs(&[("a", "c"), ("b", "d")]),
            &JobHandle::silent(),
        )
        .unwrap();
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0.path(), dir.path().join("a"));
        assert!(matches!(report.failed[0].1, Error::Exists(_)));
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(read(dir.path(), "c"), "intruder");
        assert_eq!(
            read(dir.path(), "a"),
            "a",
            "restored under the original name"
        );
        assert_eq!(read(dir.path(), "d"), "b");
        no_temp_names(dir.path());
    }

    #[test]
    fn a_failed_restore_reports_the_intermediate_name() {
        let (dir, folder) = setup(&["a"]);
        let target = dir.path().join("c");
        let original = dir.path().join("a");
        let vfs = hooked({
            let (target, original) = (target.clone(), original.clone());
            move |_, _, to| {
                // The final target and the original name both get taken meanwhile.
                if to.path() == target {
                    fs::write(&target, "intruder").unwrap();
                    fs::write(&original, "new a").unwrap();
                }
            }
        });
        let report =
            rename_many(&vfs, &folder, &pairs(&[("a", "c")]), &JobHandle::silent()).unwrap();
        assert_eq!(report.failed.len(), 1);
        let Error::RenameLeftover(temp) = &report.failed[0].1 else {
            panic!("expected leftover, got {:?}", report.failed[0].1);
        };
        assert_eq!(read(dir.path(), "c"), "intruder");
        assert_eq!(read(dir.path(), "a"), "new a");
        assert_eq!(fs::read_to_string(temp).unwrap(), "a");
        assert!(temp
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(TEMP_PREFIX));
    }

    #[test]
    fn cancel_in_phase_one_restores_everything() {
        let (dir, folder) = setup(&["a", "b", "c"]);
        let job = JobHandle::silent();
        let vfs = hooked({
            let job = job.clone();
            move |n, _, _| {
                if n == 2 {
                    job.cancel();
                }
            }
        });
        let report = rename_many(
            &vfs,
            &folder,
            &pairs(&[("a", "x"), ("b", "y"), ("c", "z")]),
            &job,
        )
        .unwrap();
        assert!(report.cancelled);
        assert!(report.renamed.is_empty() && report.failed.is_empty());
        assert_eq!(report.skipped.len(), 3);
        assert_eq!(names(dir.path()), ["a", "b", "c"]);
    }

    #[test]
    fn cancel_in_phase_two_keeps_done_items_and_restores_the_rest() {
        let (dir, folder) = setup(&["a", "b", "c"]);
        let job = JobHandle::silent();
        let vfs = hooked({
            let job = job.clone();
            // Three staging renames, then the first final rename.
            move |n, _, _| {
                if n == 4 {
                    job.cancel();
                }
            }
        });
        let report = rename_many(
            &vfs,
            &folder,
            &pairs(&[("a", "x"), ("b", "y"), ("c", "z")]),
            &job,
        )
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(report.skipped.len(), 2);
        assert!(report.failed.is_empty());
        assert_eq!(names(dir.path()), ["b", "c", "x"]);
        no_temp_names(dir.path());
    }

    #[test]
    fn no_intermediate_names_remain_after_normal_runs() {
        let (dir, folder) = setup(&["a", "b", "c", "d"]);
        let report = run(&folder, &[("a", "b"), ("b", "a"), ("c", "d"), ("d", "e")]);
        assert!(report.failed.is_empty());
        no_temp_names(dir.path());
        assert_eq!(names(dir.path()), ["a", "b", "d", "e"]);
    }

    #[test]
    fn op_report_carries_new_paths_as_done_and_old_paths_as_failed() {
        let (dir, folder) = setup(&["a"]);
        let report = run(&folder, &[("a", "b"), ("gone", "c")]).into_op_report();
        assert_eq!(report.done.len(), 1);
        assert_eq!(report.done[0].path(), dir.path().join("b"));
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0.path(), dir.path().join("gone"));
    }

    #[test]
    fn temp_names_are_valid_and_differ() {
        let a = temp_name();
        let b = temp_name();
        assert_ne!(a, b);
        assert!(a.starts_with(TEMP_PREFIX));
        assert!(validate_name(&a).is_ok());
    }
}
