use super::*;
use crate::error::Error;
use crate::ops::{Conflict, ConflictResolver, JobHandle, ProgressEvent, ProgressSink, Resolution};
use crate::vfs::{EntryKind, LocalVfs, VPath};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const BASE: u64 = 1_700_000_000;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(BASE + secs)
}
fn put(path: impl AsRef<Path>, content: &str, secs: u64) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
    set_time(path, secs);
}
fn set_time(path: &Path, secs: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at(secs))
        .unwrap();
}
fn vp(path: impl AsRef<Path>) -> VPath {
    VPath::local(path).unwrap()
}
fn quiet() -> JobHandle {
    JobHandle::silent()
}
fn compare(l: &Path, r: &Path, opts: &CompareOptions) -> CompareResult {
    compare_dirs(&LocalVfs, &vp(l), &LocalVfs, &vp(r), opts, &quiet())
}
fn state(result: &CompareResult, rel: &str) -> CompareState {
    result
        .entries
        .iter()
        .find(|e| e.rel == rel)
        .unwrap_or_else(|| panic!("no entry {rel}: {:?}", result.entries))
        .state
}
fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}
fn run(plan: &SyncPlan) -> crate::ops::OpReport {
    run_sync(&LocalVfs, &LocalVfs, plan, plan.token, None, &quiet())
}
fn trash_count(data_home: &Path) -> usize {
    fs::read_dir(data_home.join("Trash/files"))
        .map(|d| d.count())
        .unwrap_or(0)
}

struct XdgGuard {
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl XdgGuard {
    fn set(root: &Path) -> (Self, PathBuf) {
        let lock = crate::vfs::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let guard = Self {
            xdg: std::env::var_os("XDG_DATA_HOME"),
            home: std::env::var_os("HOME"),
            _lock: lock,
        };
        let data_home = root.join("data-home");
        let home = root.join("home");
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&home).unwrap();
        std::env::set_var("XDG_DATA_HOME", &data_home);
        std::env::set_var("HOME", &home);
        (guard, data_home)
    }
}
impl Drop for XdgGuard {
    fn drop(&mut self) {
        for (key, value) in [
            ("XDG_DATA_HOME", self.xdg.take()),
            ("HOME", self.home.take()),
        ] {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct Choose(Resolution);
impl ConflictResolver for Choose {
    fn resolve(&self, _: &Conflict) -> Resolution {
        self.0
    }
}

fn pair() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let (l, r) = (dir.path().join("left"), dir.path().join("right"));
    fs::create_dir(&l).unwrap();
    fs::create_dir(&r).unwrap();
    (dir, l, r)
}

// ---------------------------------------------------------------- compare_dirs

#[test]
fn every_state_has_a_matching_pair() {
    let (_d, l, r) = pair();
    put(l.join("only_left.txt"), "a", 0);
    put(r.join("only_right.txt"), "b", 0);
    put(l.join("same.txt"), "same", 10);
    put(r.join("same.txt"), "same", 10);
    put(l.join("left_newer.txt"), "x", 100);
    put(r.join("left_newer.txt"), "x", 10);
    put(l.join("right_newer.txt"), "x", 10);
    put(r.join("right_newer.txt"), "x", 100);
    put(l.join("different.txt"), "short", 10);
    put(r.join("different.txt"), "longer text", 10);
    put(l.join("mismatch"), "file", 10);
    fs::create_dir(r.join("mismatch")).unwrap();
    fs::create_dir(l.join("dir_both")).unwrap();
    fs::create_dir(r.join("dir_both")).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    assert!(!result.incomplete, "{:?}", result.issues);
    assert_eq!(state(&result, "only_left.txt"), CompareState::OnlyLeft);
    assert_eq!(state(&result, "only_right.txt"), CompareState::OnlyRight);
    assert_eq!(state(&result, "same.txt"), CompareState::Same);
    assert_eq!(state(&result, "left_newer.txt"), CompareState::LeftNewer);
    assert_eq!(state(&result, "right_newer.txt"), CompareState::RightNewer);
    assert_eq!(state(&result, "different.txt"), CompareState::Different);
    assert_eq!(state(&result, "mismatch"), CompareState::TypeMismatch);
    assert_eq!(state(&result, "dir_both"), CompareState::Same);
}

#[test]
fn unknown_state_exists_for_unreadable_entries() {
    if is_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let (_d, l, r) = pair();
    fs::create_dir(l.join("locked")).unwrap();
    fs::create_dir(r.join("locked")).unwrap();
    fs::set_permissions(l.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    fs::set_permissions(l.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(state(&result, "locked"), CompareState::Unknown);
    assert!(result.incomplete);
}

#[test]
fn time_tolerance_is_two_seconds() {
    let (_d, l, r) = pair();
    put(l.join("near"), "x", 11);
    put(r.join("near"), "x", 10);
    put(l.join("edge"), "x", 12);
    put(r.join("edge"), "x", 10);
    put(l.join("far"), "x", 13);
    put(r.join("far"), "x", 10);
    put(l.join("near_diff"), "xx", 11);
    put(r.join("near_diff"), "x", 10);
    let result = compare(&l, &r, &CompareOptions::default());
    assert_eq!(state(&result, "near"), CompareState::Same);
    assert_eq!(state(&result, "edge"), CompareState::Same);
    assert_eq!(state(&result, "far"), CompareState::LeftNewer);
    assert_eq!(state(&result, "near_diff"), CompareState::Different);
}

#[test]
fn content_mode_looks_at_bytes_not_at_time() {
    let (_d, l, r) = pair();
    put(l.join("swapped"), "aaaa", 10);
    put(r.join("swapped"), "aaab", 10);
    put(l.join("moved_time"), "same", 100);
    put(r.join("moved_time"), "same", 10);
    put(l.join("sized"), "a", 10);
    put(r.join("sized"), "ab", 10);
    let by_time = compare(&l, &r, &CompareOptions::default());
    assert_eq!(state(&by_time, "swapped"), CompareState::Same);
    assert_eq!(state(&by_time, "moved_time"), CompareState::LeftNewer);
    let content = compare(
        &l,
        &r,
        &CompareOptions {
            mode: CompareMode::Content,
            ..Default::default()
        },
    );
    assert_eq!(state(&content, "swapped"), CompareState::Different);
    assert_eq!(state(&content, "moved_time"), CompareState::Same);
    assert_eq!(state(&content, "sized"), CompareState::Different);
    assert!(!content.incomplete);
}

#[test]
fn content_mode_finds_a_late_difference_in_big_files() {
    let (_d, l, r) = pair();
    let mut a = vec![b'x'; 1_000_000];
    let b = a.clone();
    a[900_000] = b'y';
    fs::write(l.join("big"), &a).unwrap();
    fs::write(r.join("big"), &b).unwrap();
    set_time(&l.join("big"), 5);
    set_time(&r.join("big"), 5);
    let opts = CompareOptions {
        mode: CompareMode::Content,
        ..Default::default()
    };
    assert_eq!(
        state(&compare(&l, &r, &opts), "big"),
        CompareState::Different
    );
}

#[test]
fn links_are_compared_as_links_and_never_followed() {
    use std::os::unix::fs::symlink;
    let (_d, l, r) = pair();
    let outside = tempfile::tempdir().unwrap();
    put(outside.path().join("secret/inner.txt"), "s", 0);
    symlink("same-target", l.join("same_link")).unwrap();
    symlink("same-target", r.join("same_link")).unwrap();
    symlink("one", l.join("diff_link")).unwrap();
    symlink("two", r.join("diff_link")).unwrap();
    symlink("/nonexistent/place", l.join("broken")).unwrap();
    symlink("/nonexistent/place", r.join("broken")).unwrap();
    symlink(outside.path().join("secret"), l.join("to_dir")).unwrap();
    put(r.join("to_dir"), "now a file", 0);
    symlink(outside.path().join("secret"), l.join("only_link")).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    assert_eq!(state(&result, "same_link"), CompareState::Same);
    assert_eq!(state(&result, "diff_link"), CompareState::Different);
    assert_eq!(state(&result, "broken"), CompareState::Same);
    assert_eq!(state(&result, "to_dir"), CompareState::TypeMismatch);
    assert_eq!(state(&result, "only_link"), CompareState::OnlyLeft);
    assert!(result.entries.iter().all(|e| !e.rel.contains("inner.txt")));
    assert!(!result.incomplete);
}

#[test]
fn recursion_lists_whole_one_sided_trees() {
    let (_d, l, r) = pair();
    put(l.join("a/b/c.txt"), "c", 0);
    put(l.join("a/d.txt"), "d", 0);
    fs::create_dir(r.join("a")).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    assert_eq!(state(&result, "a"), CompareState::Same);
    assert_eq!(state(&result, "a/b"), CompareState::OnlyLeft);
    assert_eq!(state(&result, "a/b/c.txt"), CompareState::OnlyLeft);
    assert_eq!(state(&result, "a/d.txt"), CompareState::OnlyLeft);
    let order: Vec<_> = result.entries.iter().map(|e| e.rel.as_str()).collect();
    assert_eq!(order, ["a", "a/b", "a/b/c.txt", "a/d.txt"]);
    let flat = compare(
        &l,
        &r,
        &CompareOptions {
            recursive: false,
            ..Default::default()
        },
    );
    assert_eq!(flat.entries.len(), 1);
    assert!(!flat.entries[0].listed);
    assert!(!flat.incomplete);
}

#[test]
fn budgets_make_the_result_incomplete() {
    let (_d, l, r) = pair();
    for i in 0..5 {
        put(l.join(format!("f{i}")), "x", 0);
    }
    let few = compare(
        &l,
        &r,
        &CompareOptions {
            max_entries: 2,
            ..Default::default()
        },
    );
    assert!(few.incomplete);
    assert_eq!(few.entries.len(), 2);
    assert!(!few.issues.is_empty());

    put(l.join("a/b/c/deep.txt"), "x", 0);
    let shallow = compare(
        &l,
        &r,
        &CompareOptions {
            max_depth: 1,
            ..Default::default()
        },
    );
    assert!(shallow.incomplete);
    assert!(shallow.entries.iter().all(|e| e.rel != "a/b/c/deep.txt"));

    put(l.join("big.bin"), &"y".repeat(4000), 0);
    put(r.join("big.bin"), &"y".repeat(4000), 0);
    let bytes = compare(
        &l,
        &r,
        &CompareOptions {
            mode: CompareMode::Content,
            max_content_bytes: 100,
            ..Default::default()
        },
    );
    assert!(bytes.incomplete);
    assert_eq!(state(&bytes, "big.bin"), CompareState::Unknown);
}

#[test]
fn cancelling_stops_the_walk_and_marks_it_incomplete() {
    let (_d, l, r) = pair();
    for i in 0..4 {
        put(l.join(format!("d{i}/f")), "x", 0);
    }
    let job = quiet();
    job.cancel();
    let result = compare_dirs(
        &LocalVfs,
        &vp(&l),
        &LocalVfs,
        &vp(&r),
        &Default::default(),
        &job,
    );
    assert!(result.cancelled && result.incomplete);
    assert!(result.entries.is_empty());

    struct CancelAtFirstFolder(Mutex<Option<JobHandle>>);
    impl ProgressSink for CancelAtFirstFolder {
        fn event(&self, event: ProgressEvent) {
            if let ProgressEvent::ItemDone(_) = event {
                if let Some(job) = self.0.lock().unwrap().as_ref() {
                    job.cancel();
                }
            }
        }
    }
    let sink = Arc::new(CancelAtFirstFolder(Mutex::new(None)));
    let job = JobHandle::new(sink.clone());
    *sink.0.lock().unwrap() = Some(job.clone());
    let result = compare_dirs(
        &LocalVfs,
        &vp(&l),
        &LocalVfs,
        &vp(&r),
        &Default::default(),
        &job,
    );
    assert!(result.cancelled && result.incomplete);
    assert!(result.entries.len() < 8);
}

// ------------------------------------------------------------------- plan_sync

fn mixed_trees() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (d, l, r) = pair();
    put(l.join("missing.txt"), "new", 0);
    put(l.join("newdir/inside.txt"), "in", 0);
    put(l.join("newer.txt"), "left new", 100);
    put(r.join("newer.txt"), "old", 10);
    put(l.join("older.txt"), "left old", 10);
    put(r.join("older.txt"), "right new", 100);
    put(r.join("extra.txt"), "extra", 0);
    put(r.join("extradir/a.txt"), "a", 0);
    put(r.join("extradir/sub/b.txt"), "b", 0);
    (d, l, r)
}
fn kinds(plan: &SyncPlan) -> Vec<String> {
    plan.actions
        .iter()
        .map(|a| match a {
            SyncAction::MakeDir { path } => format!("mkdir {}", name(path)),
            SyncAction::Copy {
                to,
                overwrite: true,
                ..
            } => format!("replace {}", name(to)),
            SyncAction::Copy { to, .. } => format!("copy {}", name(to)),
            SyncAction::Trash { path, .. } => format!("trash {}", name(path)),
        })
        .collect()
}
fn name(p: &VPath) -> String {
    p.path().file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn plan_left_to_right_only_adds_what_is_missing_by_default() {
    let (_d, l, r) = mixed_trees();
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::LeftToRight, &SyncOptions::default()).unwrap();
    assert_eq!(
        kinds(&plan),
        ["copy missing.txt", "mkdir newdir", "copy inside.txt"]
    );
    assert_eq!(
        (plan.copies, plan.replaces, plan.trashes, plan.make_dirs),
        (2, 0, 0, 1)
    );
    assert!(plan.notes.iter().any(|n| n.rel == "newer.txt"));
    let replace = SyncOptions {
        replace_newer: true,
        ..Default::default()
    };
    let plan = plan_sync(&result, SyncMode::LeftToRight, &replace).unwrap();
    assert_eq!(plan.replaces, 1);
    assert!(kinds(&plan).contains(&"replace newer.txt".to_string()));
    assert!(!kinds(&plan).iter().any(|k| k.contains("older.txt")));
    assert_eq!(plan.trashes, 0);
}

#[test]
fn plan_right_to_left_mirrors_the_direction() {
    let (_d, l, r) = mixed_trees();
    let result = compare(&l, &r, &CompareOptions::default());
    let replace = SyncOptions {
        replace_newer: true,
        ..Default::default()
    };
    let plan = plan_sync(&result, SyncMode::RightToLeft, &replace).unwrap();
    let k = kinds(&plan);
    assert!(k.contains(&"copy extra.txt".to_string()));
    assert!(k.contains(&"mkdir extradir".to_string()));
    assert!(k.contains(&"replace older.txt".to_string()));
    assert!(!k
        .iter()
        .any(|x| x.contains("newer.txt") || x.contains("missing")));
    assert_eq!(plan.trashes, 0);
    assert_eq!(plan.target.path(), l);
}

#[test]
fn mirror_plans_trash_entry_by_entry_bottom_up() {
    let (_d, l, r) = mixed_trees();
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::Mirror, &SyncOptions::default()).unwrap();
    let trashed: Vec<String> = kinds(&plan)
        .into_iter()
        .filter(|k| k.starts_with("trash"))
        .collect();
    assert_eq!(
        trashed,
        [
            "trash b.txt",
            "trash sub",
            "trash a.txt",
            "trash extradir",
            "trash extra.txt"
        ]
    );
    // Folders carry the planned content; every action names exactly one entry.
    for action in &plan.actions {
        if let SyncAction::Trash { path, expected } = action {
            if path.path().ends_with("extradir") {
                assert_eq!(expected.children, ["a.txt", "sub"]);
            }
        }
    }
    // Without "replace" a mirror leaves differing files alone.
    assert_eq!(plan.replaces, 0);
    let both = plan_sync(
        &result,
        SyncMode::Mirror,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(both.replaces, 2);
}

#[test]
fn unknown_and_incomplete_parts_get_no_actions_and_no_removals() {
    if is_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let (_d, l, r) = pair();
    put(l.join("ok.txt"), "x", 0);
    put(r.join("gone_in_left.txt"), "x", 0);
    put(l.join("locked/secret.txt"), "x", 0);
    put(r.join("locked/other.txt"), "x", 0);
    fs::set_permissions(l.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    fs::set_permissions(l.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.incomplete);
    assert_eq!(state(&result, "locked"), CompareState::Unknown);
    assert!(result.entries.iter().all(|e| !e.rel.starts_with("locked/")));
    let plan = plan_sync(&result, SyncMode::Mirror, &SyncOptions::default()).unwrap();
    assert!(plan.incomplete);
    assert_eq!(kinds(&plan), ["copy ok.txt"]);
    assert_eq!(plan.trashes, 0);
}

#[test]
fn budget_limited_result_blocks_removals_too() {
    let (_d, l, r) = pair();
    put(r.join("only_right.txt"), "x", 0);
    for i in 0..3 {
        put(l.join(format!("f{i}")), "x", 0);
    }
    let result = compare(
        &l,
        &r,
        &CompareOptions {
            max_entries: 3,
            ..Default::default()
        },
    );
    assert!(result.incomplete);
    let plan = plan_sync(&result, SyncMode::Mirror, &SyncOptions::default()).unwrap();
    assert_eq!(plan.trashes, 0);
}

#[test]
fn same_place_and_nested_places_are_refused() {
    let (_d, l, _r) = pair();
    put(l.join("sub/f"), "x", 0);
    let same = compare(&l, &l, &CompareOptions::default());
    assert!(matches!(
        plan_sync(&same, SyncMode::LeftToRight, &Default::default()),
        Err(Error::SameFile)
    ));
    let nested = compare(&l, &l.join("sub"), &CompareOptions::default());
    for mode in [
        SyncMode::LeftToRight,
        SyncMode::RightToLeft,
        SyncMode::Mirror,
    ] {
        assert!(matches!(
            plan_sync(&nested, mode, &Default::default()),
            Err(Error::DestinationInsideSource)
        ));
    }
    let link = l.parent().unwrap().join("link_to_left");
    std::os::unix::fs::symlink(&l, &link).unwrap();
    let via_link = compare(&l, &link, &CompareOptions::default());
    assert!(matches!(
        plan_sync(&via_link, SyncMode::Mirror, &Default::default()),
        Err(Error::SameFile)
    ));
}

#[test]
fn an_archive_is_never_a_write_target() {
    let (_d, l, _r) = pair();
    let archive = VPath::archive(vp(l.join("x.zip")), "/").unwrap();
    let result = CompareResult {
        left: vp(&l),
        right: archive,
        mode: CompareMode::SizeTime,
        recursive: true,
        entries: Vec::new(),
        incomplete: false,
        cancelled: false,
        issues: Vec::new(),
    };
    for mode in [SyncMode::LeftToRight, SyncMode::Mirror] {
        assert!(matches!(
            plan_sync(&result, mode, &Default::default()),
            Err(Error::Unsupported(crate::archive::MSG_READ_ONLY))
        ));
    }
}

#[test]
fn select_makes_a_new_plan_with_a_new_token() {
    let (_d, l, r) = mixed_trees();
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::LeftToRight, &Default::default()).unwrap();
    let picked = plan.select(&[0]);
    assert_eq!(picked.actions.len(), 1);
    assert_ne!(picked.token, plan.token);
    assert_eq!(picked.copies, 1);
    let report = run_sync(&LocalVfs, &LocalVfs, &picked, plan.token, None, &quiet());
    assert!(report.done.is_empty() && !report.failed.is_empty());
    assert!(!r.join("missing.txt").exists());
}

// -------------------------------------------------------------------- run_sync

#[test]
fn run_copies_missing_entries_with_content_links_and_folders() {
    let (_d, l, r) = mixed_trees();
    std::os::unix::fs::symlink("missing.txt", l.join("alias")).unwrap();
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::LeftToRight, &Default::default()).unwrap();
    let report = run(&plan);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(fs::read_to_string(r.join("missing.txt")).unwrap(), "new");
    assert_eq!(
        fs::read_to_string(r.join("newdir/inside.txt")).unwrap(),
        "in"
    );
    assert_eq!(
        fs::read_link(r.join("alias")).unwrap(),
        Path::new("missing.txt")
    );
    // Nothing outside the plan changed.
    assert_eq!(fs::read_to_string(r.join("newer.txt")).unwrap(), "old");
    assert!(r.join("extra.txt").exists());
    let again = compare(&l, &r, &CompareOptions::default());
    assert_eq!(state(&again, "missing.txt"), CompareState::Same);
}

#[test]
fn wrong_token_copies_and_deletes_nothing() {
    let (_d, l, r) = mixed_trees();
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(
        &result,
        SyncMode::Mirror,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    let report = run_sync(&LocalVfs, &LocalVfs, &plan, plan.token ^ 1, None, &quiet());
    assert!(report.done.is_empty());
    assert_eq!(report.failed.len(), 1);
    assert!(!r.join("missing.txt").exists());
    assert!(r.join("extra.txt").exists());
    assert_eq!(fs::read_to_string(r.join("newer.txt")).unwrap(), "old");
}

#[test]
fn mirror_moves_extras_to_the_trash_and_never_deletes_permanently() {
    let (d, l, r) = mixed_trees();
    let (_env, data_home) = XdgGuard::set(d.path());
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::Mirror, &Default::default()).unwrap();
    let report = run(&plan);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(report.skipped.is_empty(), "{:?}", report.warnings);
    assert!(!r.join("extra.txt").exists());
    assert!(!r.join("extradir").exists());
    assert!(r.join("missing.txt").exists());
    // extra.txt, a.txt, b.txt go to the trash one by one; the emptied folders (sub,
    // extradir) are removed directly with rmdir, never as a tree.
    assert_eq!(trash_count(&data_home), 3);
    let trashed_names: Vec<String> = fs::read_dir(data_home.join("Trash/files"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(trashed_names.iter().any(|n| n.starts_with("b.txt")));
    // The differing files were not part of a mirror without "replace".
    assert_eq!(fs::read_to_string(r.join("newer.txt")).unwrap(), "old");
    assert_eq!(
        fs::read_to_string(r.join("older.txt")).unwrap(),
        "right new"
    );
}

#[test]
fn stale_overwrite_target_with_same_size_and_time_is_skipped() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old data!", 10);
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(
        &result,
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plan.replaces, 1);
    // Replace the target in place with other bytes of the same length and the same time.
    fs::write(r.join("f.txt"), "NEW data!").unwrap();
    set_time(&r.join("f.txt"), 10);
    let report = run(&plan);
    assert_eq!(
        report.skipped.len(),
        1,
        "{report:?}",
        report = report.warnings
    );
    assert!(report.done.is_empty());
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "NEW data!");

    // Same again with a swapped file (new inode, same bytes, same time).
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    fs::write(r.join("swap.tmp"), "NEW data!").unwrap();
    set_time(&r.join("swap.tmp"), 10);
    fs::rename(r.join("swap.tmp"), r.join("f.txt")).unwrap();
    let report = run(&plan);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "NEW data!");
}

#[test]
fn unchanged_overwrite_target_is_replaced_and_keeps_the_source_time() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(
        &result,
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    let report = run(&plan);
    assert!(report.failed.is_empty() && report.skipped.is_empty());
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "from left");
    assert_eq!(
        fs::metadata(r.join("f.txt")).unwrap().modified().unwrap(),
        at(100)
    );
}

#[test]
fn a_target_that_appeared_after_planning_is_not_overwritten() {
    let (_d, l, r) = pair();
    put(l.join("new.txt"), "from left", 0);
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::LeftToRight, &Default::default()).unwrap();
    put(r.join("new.txt"), "precious", 5);
    let report = run(&plan);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(fs::read_to_string(r.join("new.txt")).unwrap(), "precious");
}

#[test]
fn removed_entry_swapped_after_planning_is_not_trashed() {
    let (d, l, r) = pair();
    let (_env, data_home) = XdgGuard::set(d.path());
    put(l.join("keep"), "k", 0);
    put(r.join("extra.txt"), "same size", 10);
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::Mirror, &Default::default()).unwrap();
    assert_eq!(plan.trashes, 1);
    fs::write(r.join("swap.tmp"), "same size").unwrap();
    set_time(&r.join("swap.tmp"), 10);
    fs::rename(r.join("swap.tmp"), r.join("extra.txt")).unwrap();
    let report = run(&plan);
    assert!(report.done.iter().all(|p| !p.path().ends_with("extra.txt")));
    assert!(r.join("extra.txt").exists());
    assert_eq!(trash_count(&data_home), 0);
}

#[test]
fn folder_with_a_new_file_after_planning_is_not_trashed() {
    let (d, l, r) = pair();
    let (_env, data_home) = XdgGuard::set(d.path());
    put(l.join("keep"), "k", 0);
    put(r.join("old_dir/planned.txt"), "p", 0);
    let result = compare(&l, &r, &CompareOptions::default());
    let plan = plan_sync(&result, SyncMode::Mirror, &Default::default()).unwrap();
    assert_eq!(plan.trashes, 2);
    put(r.join("old_dir/late.txt"), "added after planning", 0);
    let report = run(&plan);
    assert!(r.join("old_dir").is_dir());
    assert_eq!(
        fs::read_to_string(r.join("old_dir/late.txt")).unwrap(),
        "added after planning"
    );
    assert!(!r.join("old_dir/planned.txt").exists());
    assert_eq!(trash_count(&data_home), 1);
    assert!(report
        .warnings
        .iter()
        .any(|(p, why)| p.path().ends_with("old_dir") && why.contains("neuer Eintrag")));
    assert!(report.skipped.iter().any(|p| p.path().ends_with("old_dir")));
}

// ------------------------------------------------ Lücke zwischen Prüfung und Aktion

fn replace_plan(l: &Path, r: &Path) -> SyncPlan {
    plan_sync(
        &compare(l, r, &CompareOptions::default()),
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap()
}

fn no_staging_left(dir: &Path) {
    let names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains("dualfile-part")),
        "Hilfsreste: {names:?}"
    );
}

fn set_hook(
    slot: &'static std::thread::LocalKey<std::cell::RefCell<Option<sync::hooks::Hook>>>,
    hook: impl Fn(&Path) + 'static,
) {
    slot.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}
fn clear_hooks() {
    use sync::hooks::*;
    for slot in [&BEFORE_EXCHANGE, &AFTER_EXCHANGE, &BEFORE_RMDIR] {
        slot.with(|h| *h.borrow_mut() = None);
    }
}

#[test]
fn overwrite_replaces_content_and_leaves_no_staging() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = replace_plan(&l, &r);
    let report = run(&plan);
    assert!(
        report.failed.is_empty() && report.skipped.is_empty(),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert_eq!(report.done.len(), 1);
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "from left");
    no_staging_left(&r);
}

#[test]
fn target_replaced_by_other_content_before_the_exchange_is_swapped_back() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = replace_plan(&l, &r);
    // After the check, someone puts a new file at the target (other inode and content).
    set_hook(&sync::hooks::BEFORE_EXCHANGE, |to| {
        let tmp = to.with_file_name("other.tmp");
        fs::write(&tmp, "precious new file").unwrap();
        fs::rename(&tmp, to).unwrap();
    });
    let report = run(&plan);
    clear_hooks();
    assert!(
        report.done.is_empty() && report.failed.is_empty(),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert_eq!(report.skipped.len(), 1);
    assert!(report
        .warnings
        .iter()
        .any(|(_, why)| why.contains("Ziel hat sich geändert")));
    assert_eq!(
        fs::read_to_string(r.join("f.txt")).unwrap(),
        "precious new file"
    );
    no_staging_left(&r);
    assert!(!r.join("other.tmp").exists());
}

#[test]
fn target_changed_in_place_after_the_exchange_is_swapped_back() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = replace_plan(&l, &r);
    // Between exchange and check the displaced file gets other content (same inode).
    set_hook(&sync::hooks::AFTER_EXCHANGE, |displaced| {
        fs::write(displaced, "changed meanwhile").unwrap();
    });
    let report = run(&plan);
    clear_hooks();
    assert!(
        report.done.is_empty(),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        fs::read_to_string(r.join("f.txt")).unwrap(),
        "changed meanwhile"
    );
    no_staging_left(&r);
}

#[test]
fn target_swapped_to_other_file_after_the_exchange_is_swapped_back() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = replace_plan(&l, &r);
    set_hook(&sync::hooks::AFTER_EXCHANGE, |displaced| {
        fs::remove_file(displaced).unwrap();
        fs::write(displaced, "a different file").unwrap();
    });
    let report = run(&plan);
    clear_hooks();
    assert!(
        report.done.is_empty(),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert_eq!(
        fs::read_to_string(r.join("f.txt")).unwrap(),
        "a different file"
    );
    no_staging_left(&r);
}

#[test]
fn cancelled_overwrite_leaves_no_staging() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = replace_plan(&l, &r);
    let job = JobHandle::silent();
    job.cancel();
    let report = run_sync(&LocalVfs, &LocalVfs, &plan, plan.token, None, &job);
    assert!(report.cancelled);
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "old");
    no_staging_left(&r);
}

#[test]
fn folder_that_gets_an_entry_after_the_check_is_not_removed() {
    let (_d, l, r) = pair();
    put(l.join("keep"), "k", 0);
    fs::create_dir(r.join("old_dir")).unwrap();
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::Mirror,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(plan.trashes, 1);
    set_hook(&sync::hooks::BEFORE_RMDIR, |dir| {
        fs::write(dir.join("late.txt"), "arrived just now").unwrap();
    });
    let report = run(&plan);
    clear_hooks();
    assert!(report.done.iter().all(|p| !p.path().ends_with("old_dir")));
    assert!(report.skipped.iter().any(|p| p.path().ends_with("old_dir")));
    assert!(report
        .warnings
        .iter()
        .any(|(p, why)| p.path().ends_with("old_dir") && why.contains("neuer Eintrag")));
    assert_eq!(
        fs::read_to_string(r.join("old_dir/late.txt")).unwrap(),
        "arrived just now"
    );
}

#[test]
fn emptied_planned_folder_is_removed_directly_not_trashed() {
    let (d, l, r) = pair();
    let (_env, data_home) = XdgGuard::set(d.path());
    put(l.join("keep"), "k", 0);
    put(r.join("old_dir/inner/planned.txt"), "p", 0);
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::Mirror,
        &Default::default(),
    )
    .unwrap();
    let report = run(&plan);
    assert!(
        report.failed.is_empty(),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert!(!r.join("old_dir").exists());
    // Only the file went to the trash; the two folders were plain rmdir.
    assert_eq!(trash_count(&data_home), 1);
}

#[test]
fn folder_swapped_for_a_link_after_the_check_keeps_link_and_target() {
    let (d, l, r) = pair();
    put(l.join("keep"), "k", 0);
    fs::create_dir(r.join("old_dir")).unwrap();
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::Mirror,
        &Default::default(),
    )
    .unwrap();
    let elsewhere = d.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    set_hook(&sync::hooks::BEFORE_RMDIR, {
        let elsewhere = elsewhere.clone();
        move |dir| {
            fs::remove_dir(dir).unwrap();
            std::os::unix::fs::symlink(&elsewhere, dir).unwrap();
        }
    });
    let report = run(&plan);
    clear_hooks();
    assert!(
        report.done.iter().all(|p| !p.path().ends_with("old_dir")),
        "{:?} {:?}",
        report.warnings,
        report.failed
    );
    assert!(fs::symlink_metadata(r.join("old_dir"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(elsewhere.is_dir());
}

#[test]
fn conflict_flow_decides_planned_overwrites() {
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, SyncPlan) {
        let (d, l, r) = pair();
        put(l.join("f.txt"), "from left", 100);
        put(r.join("f.txt"), "old", 10);
        let plan = plan_sync(
            &compare(&l, &r, &CompareOptions::default()),
            SyncMode::LeftToRight,
            &SyncOptions {
                replace_newer: true,
                ..Default::default()
            },
        )
        .unwrap();
        (d, l, r, plan)
    }
    let go = |plan: &SyncPlan, choice: Resolution| {
        run_sync(
            &LocalVfs,
            &LocalVfs,
            plan,
            plan.token,
            Some(&Choose(choice)),
            &quiet(),
        )
    };
    let (_d, _l, r, plan) = setup();
    let report = go(&plan, Resolution::Skip);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "old");

    let (_d, _l, r, plan) = setup();
    let report = go(&plan, Resolution::KeepBoth);
    assert!(report.failed.is_empty());
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "old");
    assert_eq!(fs::read_dir(&r).unwrap().count(), 2);

    let (_d, _l, r, plan) = setup();
    let report = go(&plan, Resolution::Cancel);
    assert!(report.cancelled);
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "old");

    let (_d, _l, r, plan) = setup();
    let report = go(&plan, Resolution::Overwrite);
    assert!(report.failed.is_empty());
    assert_eq!(fs::read_to_string(r.join("f.txt")).unwrap(), "from left");
}

#[test]
fn cancelling_a_run_stops_at_once() {
    let (_d, l, r) = pair();
    for i in 0..5 {
        put(l.join(format!("f{i}")), "x", 0);
    }
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::LeftToRight,
        &Default::default(),
    )
    .unwrap();
    let job = quiet();
    job.cancel();
    let report = run_sync(&LocalVfs, &LocalVfs, &plan, plan.token, None, &job);
    assert!(report.cancelled);
    assert_eq!(fs::read_dir(&r).unwrap().count(), 0);

    struct CancelAfterFirst(Mutex<Option<JobHandle>>);
    impl ProgressSink for CancelAfterFirst {
        fn event(&self, event: ProgressEvent) {
            if let ProgressEvent::ItemDone(_) = event {
                if let Some(job) = self.0.lock().unwrap().as_ref() {
                    job.cancel();
                }
            }
        }
    }
    let sink = Arc::new(CancelAfterFirst(Mutex::new(None)));
    let job = JobHandle::new(sink.clone());
    *sink.0.lock().unwrap() = Some(job.clone());
    let report = run_sync(&LocalVfs, &LocalVfs, &plan, plan.token, None, &job);
    assert!(report.cancelled);
    assert_eq!(fs::read_dir(&r).unwrap().count(), 1);
}

#[test]
fn the_plan_records_a_hash_and_identity_for_every_overwrite() {
    let (_d, l, r) = pair();
    put(l.join("f.txt"), "from left", 100);
    put(r.join("f.txt"), "old", 10);
    let plan = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            ..Default::default()
        },
    )
    .unwrap();
    let SyncAction::Copy {
        expected_target: Some(version),
        overwrite: true,
        ..
    } = &plan.actions[0]
    else {
        panic!("expected an overwrite");
    };
    assert_eq!(version.kind, EntryKind::File);
    assert_eq!(version.size, 3);
    assert!(version.sha256.is_some() && version.ino != 0);
    let tiny = plan_sync(
        &compare(&l, &r, &CompareOptions::default()),
        SyncMode::LeftToRight,
        &SyncOptions {
            replace_newer: true,
            max_hash_bytes: 1,
        },
    )
    .unwrap();
    assert!(tiny.actions.is_empty());
    assert!(tiny.notes.iter().any(|n| n.rel == "f.txt"));
}

// ------------------------------------------------------------------- diff_text

fn hunks(a: &str, b: &str) -> Vec<(HunkKind, Vec<String>, Vec<String>)> {
    diff_text(a.as_bytes(), b.as_bytes(), &DiffOptions::default())
        .unwrap()
        .into_iter()
        .map(|h| (h.kind, h.left, h.right))
        .collect()
}
fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn diff_reports_equal_changed_added_and_removed_lines() {
    let result = hunks("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
    assert_eq!(
        result,
        [
            (HunkKind::Equal, v(&["a"]), v(&["a"])),
            (HunkKind::Replace, v(&["b"]), v(&["B"])),
            (HunkKind::Equal, v(&["c", "d"]), v(&["c", "d"])),
            (HunkKind::Insert, v(&[]), v(&["e"])),
        ]
    );
    let result = hunks("a\nb\nc\n", "a\nc\n");
    assert_eq!(result[1], (HunkKind::Delete, v(&["b"]), v(&[])));
    let positions = diff_text(b"a\nb\nc\n", b"a\nc\n", &DiffOptions::default()).unwrap();
    assert_eq!((positions[1].left_start, positions[1].right_start), (1, 1));
    assert_eq!((positions[2].left_start, positions[2].right_start), (2, 1));
}

#[test]
fn diff_identical_and_empty_inputs() {
    assert!(hunks("", "").is_empty());
    assert_eq!(
        hunks("a\n", "a\n"),
        [(HunkKind::Equal, v(&["a"]), v(&["a"]))]
    );
    assert_eq!(
        hunks("", "x\ny\n"),
        [(HunkKind::Insert, v(&[]), v(&["x", "y"]))]
    );
    assert_eq!(
        hunks("x\ny\n", ""),
        [(HunkKind::Delete, v(&["x", "y"]), v(&[]))]
    );
    assert_eq!(hunks("\n", ""), [(HunkKind::Delete, v(&[""]), v(&[]))]);
}

#[test]
fn diff_sees_a_missing_final_line_break() {
    let result = hunks("a\nb", "a\nb\n");
    assert_eq!(
        result,
        [
            (HunkKind::Equal, v(&["a"]), v(&["a"])),
            (HunkKind::Replace, v(&["b"]), v(&["b"])),
        ]
    );
    assert_eq!(
        hunks("a\nb", "a\nb"),
        [(HunkKind::Equal, v(&["a", "b"]), v(&["a", "b"]))]
    );
    let relaxed = DiffOptions {
        ignore_whitespace: true,
        ..Default::default()
    };
    let result = diff_text(b"a\nb", b"a\nb\n", &relaxed).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].kind, HunkKind::Equal);
}

#[test]
fn diff_options_ignore_white_space_and_case() {
    let strict = hunks("a b\nX\n", "a  b \r\nx\n");
    assert!(strict.iter().any(|h| h.0 == HunkKind::Replace));
    let relaxed = DiffOptions {
        ignore_whitespace: true,
        ignore_case: true,
        ..Default::default()
    };
    let result = diff_text(b"a b\nX\n", b"a  b \r\nx\n", &relaxed).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].kind, HunkKind::Equal);
    assert_eq!(result[0].left, v(&["a b", "X"]));
    assert_eq!(result[0].right, v(&["a  b \r", "x"]));
    let only_case = DiffOptions {
        ignore_case: true,
        ..Default::default()
    };
    assert_eq!(
        diff_text(b"Hello\n", b"hello\n", &only_case).unwrap().len(),
        1
    );
    assert!(diff_text(b"a b\n", b"ab\n", &only_case)
        .unwrap()
        .iter()
        .any(|h| h.kind == HunkKind::Replace));
}

#[test]
fn diff_limits_and_binary_are_refused() {
    let small = DiffOptions {
        max_bytes: 10,
        ..Default::default()
    };
    assert!(matches!(
        diff_text(b"short\n", b"this is longer than ten bytes\n", &small),
        Err(Error::Unsupported(MSG_TOO_LARGE))
    ));
    assert!(diff_text(b"ten bytes!", b"x", &small).is_ok());
    let few = DiffOptions {
        max_lines: 3,
        ..Default::default()
    };
    assert!(diff_text(b"1\n2\n3\n", b"1\n", &few).is_ok());
    assert!(matches!(
        diff_text(b"1\n", b"1\n2\n3\n4\n", &few),
        Err(Error::Unsupported(MSG_TOO_LARGE))
    ));
    assert!(matches!(
        diff_text(b"ab\0cd", b"text", &DiffOptions::default()),
        Err(Error::Unsupported(MSG_BINARY))
    ));
    assert!(matches!(
        diff_text(b"text", b"\0", &DiffOptions::default()),
        Err(Error::Unsupported(MSG_BINARY))
    ));
}

#[test]
fn diff_handles_unicode_and_invalid_utf8_as_text() {
    let result = hunks("Grüße\n日本語\nñ\n", "Grüße\n日本人\nñ\n");
    assert_eq!(
        result[1],
        (HunkKind::Replace, v(&["日本語"]), v(&["日本人"]))
    );
    let lossy = diff_text(b"caf\xe9\n", b"caf\xe9\n", &DiffOptions::default()).unwrap();
    assert_eq!(lossy.len(), 1);
    assert_eq!(lossy[0].left, v(&["caf\u{fffd}"]));
}

fn rebuild(hunks: &[DiffHunk]) -> (Vec<String>, Vec<String>) {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for h in hunks {
        assert_eq!(h.left_start, l.len());
        assert_eq!(h.right_start, r.len());
        l.extend(h.left.clone());
        r.extend(h.right.clone());
    }
    (l, r)
}

#[test]
fn diff_hunks_always_reconstruct_both_sides() {
    let mut seed = 12345u64;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    for round in 0..200 {
        let make = |next: &mut dyn FnMut(u64) -> u64| {
            let count = next(30);
            (0..count)
                .map(|_| format!("line{}\n", next(6)))
                .collect::<String>()
        };
        let (a, b) = (make(&mut next), make(&mut next));
        let result = diff_text(a.as_bytes(), b.as_bytes(), &DiffOptions::default()).unwrap();
        let (l, r) = rebuild(&result);
        assert_eq!(l.join("\n"), a.trim_end_matches('\n'), "round {round}");
        assert_eq!(r.join("\n"), b.trim_end_matches('\n'), "round {round}");
        for h in &result {
            if h.kind == HunkKind::Equal {
                assert_eq!(h.left, h.right);
            }
        }
    }
}

#[test]
fn diff_finds_the_shortest_script_and_survives_huge_distances() {
    // ABCABBA -> CBABAC has edit distance 5 (insert + delete counted separately).
    let a = "A\nB\nC\nA\nB\nB\nA\n";
    let b = "C\nB\nA\nB\nA\nC\n";
    let result = diff_text(a.as_bytes(), b.as_bytes(), &DiffOptions::default()).unwrap();
    let changed: usize = result
        .iter()
        .filter(|h| h.kind != HunkKind::Equal)
        .map(|h| h.left.len() + h.right.len())
        .sum();
    assert_eq!(changed, 5);

    let left: String = (0..3000).map(|i| format!("l{i}\n")).collect();
    let right: String = (0..3000).map(|i| format!("r{i}\n")).collect();
    let result = diff_text(left.as_bytes(), right.as_bytes(), &DiffOptions::default()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].kind, HunkKind::Replace);
    let (l, r) = rebuild(&result);
    assert_eq!((l.len(), r.len()), (3000, 3000));
}

#[test]
fn diff_files_reads_through_the_vfs_and_refuses_special_files() {
    let (_d, l, r) = pair();
    put(l.join("a.txt"), "one\ntwo\n", 0);
    put(r.join("b.txt"), "one\n2\n", 0);
    let result = diff_files(
        &LocalVfs,
        &vp(l.join("a.txt")),
        &LocalVfs,
        &vp(r.join("b.txt")),
        &DiffOptions::default(),
    )
    .unwrap();
    assert_eq!(result[1].kind, HunkKind::Replace);
    let small = DiffOptions {
        max_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        diff_files(
            &LocalVfs,
            &vp(l.join("a.txt")),
            &LocalVfs,
            &vp(r.join("b.txt")),
            &small
        ),
        Err(Error::Unsupported(MSG_TOO_LARGE))
    ));
    assert!(diff_files(
        &LocalVfs,
        &vp(&l),
        &LocalVfs,
        &vp(r.join("b.txt")),
        &DiffOptions::default()
    )
    .is_err());
    let fifo = l.join("pipe");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(diff_files(
        &LocalVfs,
        &vp(&fifo),
        &LocalVfs,
        &vp(r.join("b.txt")),
        &DiffOptions::default()
    )
    .is_err());
}

#[test]
fn an_archive_can_be_the_source_of_a_sync() {
    use std::io::Write;
    let (_d, l, r) = pair();
    let zip_path = l.join("source.zip");
    {
        let mut writer = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("docs/readme.txt", options).unwrap();
        writer.write_all(b"from the archive").unwrap();
        writer.finish().unwrap();
    }
    put(r.join("own.txt"), "mine", 0);
    let root = VPath::archive(vp(&zip_path), "/").unwrap();
    let archive_vfs = crate::open_path(&root).unwrap();
    let result = compare_dirs(
        archive_vfs.as_ref(),
        &root,
        &LocalVfs,
        &vp(&r),
        &CompareOptions::default(),
        &quiet(),
    );
    assert_eq!(state(&result, "docs/readme.txt"), CompareState::OnlyLeft);
    let plan = plan_sync(&result, SyncMode::Mirror, &Default::default()).unwrap();
    assert_eq!(plan.trashes, 1);
    let copy_only = plan_sync(&result, SyncMode::LeftToRight, &Default::default()).unwrap();
    let report = run_sync(
        archive_vfs.as_ref(),
        &LocalVfs,
        &copy_only,
        copy_only.token,
        None,
        &quiet(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        fs::read_to_string(r.join("docs/readme.txt")).unwrap(),
        "from the archive"
    );
    assert!(r.join("own.txt").exists());
    // The other way round the archive is read-only.
    assert!(plan_sync(&result, SyncMode::RightToLeft, &Default::default()).is_err());
}
