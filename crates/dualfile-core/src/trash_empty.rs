//! Bounded empty-trash work, restricted to known local filesystems and owned directories.
use crate::{
    drives,
    ops::{JobHandle, OpReport, ProgressEvent},
    places_pool::{Pool, ProgressWatchdog},
    Error, VPath,
};
use rustix::fs::{openat, Mode, OFlags, CWD};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(4))
}
fn trash_local_type(fs: &str) -> bool {
    drives::local_space_type(fs) || fs == "fuseblk"
}
fn points(table: &str) -> Vec<(PathBuf, bool)> {
    table
        .lines()
        .filter_map(|line| {
            let (head, tail) = line.split_once(" - ")?;
            Some((
                drives::unescape(head.split_whitespace().nth(4)?).into(),
                trash_local_type(tail.split_whitespace().next()?),
            ))
        })
        .collect()
}
fn safe_path(path: &Path, points: &[(PathBuf, bool)]) -> bool {
    points
        .iter()
        .filter(|(p, _)| path.starts_with(p))
        .max_by_key(|(p, _)| p.components().count())
        .is_some_and(|(_, local)| *local)
        && !points.iter().any(|(p, _)| p != path && p.starts_with(path))
}
fn candidates(table: &str, data: Option<&Path>, uid: u32) -> Vec<PathBuf> {
    let points = points(table);
    let mut result: Vec<_> = data.map(|data| data.join("Trash")).into_iter().collect();
    for (mount, local) in &points {
        if *local {
            result.push(mount.join(format!(".Trash/{uid}")));
            result.push(mount.join(format!(".Trash-{uid}")));
        }
    }
    result.retain(|p| safe_path(p, &points));
    result.sort();
    result.dedup();
    result
}
fn open_dir(path: &Path) -> std::io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(CWD, "/", flags, Mode::empty())?;
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    for part in path.components() {
        match part {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => fd = openat(&fd, name, flags, Mode::empty())?,
            _ => return Err(std::io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(fd.into())
}
/// Why an owned directory was refused: it is writable by group or others.
#[derive(Debug)]
struct OtherWritable(u32);
impl std::fmt::Display for OtherWritable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "directory is writable by others (mode {:03o})", self.0)
    }
}
impl std::error::Error for OtherWritable {}
/// `Ok(None)` = fine, `Ok(Some(mode))` = owned but writable by others, `Err(())` = not ours.
fn check_owned(file: &File, uid: u32) -> Result<Option<u32>, ()> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().map_err(|_| ())?;
    if meta.uid() != uid {
        return Err(());
    }
    Ok((meta.mode() & 0o022 != 0).then_some(meta.mode() & 0o777))
}
fn open_trash(path: &Path, uid: u32) -> std::io::Result<(File, File, File)> {
    let root = open_dir(path)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let files = File::from(openat(&root, "files", flags, Mode::empty())?);
    let info = File::from(openat(&root, "info", flags, Mode::empty())?);
    let mut loose = None;
    for dir in [&root, &files, &info] {
        match check_owned(dir, uid) {
            Ok(mode) => loose = loose.or(mode),
            Err(()) => return Err(std::io::ErrorKind::PermissionDenied.into()),
        }
    }
    if let Some(mode) = loose {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OtherWritable(mode),
        ));
    }
    Ok((root, files, info))
}
/// Why a trash directory was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Group or others may write to it.
    OtherWritable,
    /// Our own directory (or one of its parts) cannot be opened, e.g. mode 000.
    NoAccess,
}
/// A trash directory that was left out; nothing in it is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedTrash {
    pub path: PathBuf,
    pub reason: SkipReason,
    /// Permission bits, e.g. `0o775` (for `NoAccess` the first part that lacks owner access).
    pub mode: u32,
}
/// Usable trash directories plus the ones refused for their permissions.
#[derive(Debug, Default)]
pub struct Discovery {
    pub found: Vec<PathBuf>,
    pub skipped: Vec<SkippedTrash>,
}
enum Probe {
    Usable,
    Unusable,
    OtherWritable(u32),
    NoAccess(u32),
}
/// The mode of the first of root, `files`, `info` that the owner cannot read and search;
/// the root's mode when all look fine. `None` when the root is gone or not ours.
fn denied_mode(path: &Path, uid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let root = std::fs::symlink_metadata(path).ok()?;
    if !root.is_dir() || root.uid() != uid {
        return None;
    }
    let lacking = |mode: u32| mode & 0o500 != 0o500;
    if lacking(root.mode()) {
        return Some(root.mode() & 0o777);
    }
    for name in ["files", "info"] {
        if let Ok(meta) = std::fs::symlink_metadata(path.join(name)) {
            if meta.uid() == uid && lacking(meta.mode()) {
                return Some(meta.mode() & 0o777);
            }
        }
    }
    Some(root.mode() & 0o777)
}
/// Missing directories are ignored; a timed-out probe fails closed before queueing work.
pub fn discover(table: &str, data: &Path) -> crate::Result<Vec<PathBuf>> {
    discover_report(table, data).map(|d| d.found)
}
/// Like [`discover`], but also names owned directories that were refused because group or
/// others may write to them (they stay untouched; the caller tells the user).
pub fn discover_report(table: &str, data: &Path) -> crate::Result<Discovery> {
    let uid = unsafe { libc::geteuid() };
    let mut found = Discovery::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    // Home and XDG_DATA_HOME are trusted roots and may contain symlinks. Resolve
    // only that root, never Trash or its children, before classifying its mount.
    let trusted = data.to_owned();
    let data = match pool().run_keyed(
        data.to_string_lossy().into_owned(),
        deadline.saturating_duration_since(std::time::Instant::now()),
        move || std::fs::canonicalize(trusted),
    ) {
        Some(Ok(path)) => Some(path),
        Some(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err(Error::Unsupported("trashPurgeFailed")),
    };
    for path in candidates(table, data.as_deref(), uid) {
        let probe = path.clone();
        match pool().run_keyed(
            path.to_string_lossy().into_owned(),
            deadline.saturating_duration_since(std::time::Instant::now()),
            move || match open_trash(&probe, uid) {
                Ok(_) => Probe::Usable,
                Err(e) => match e.get_ref().and_then(|i| i.downcast_ref::<OtherWritable>()) {
                    Some(loose) => Probe::OtherWritable(loose.0),
                    // A real EACCES from open (not our own "not owned" refusal, which has no
                    // OS error): report it instead of dropping it silently.
                    None if e.raw_os_error() == Some(libc::EACCES) => {
                        denied_mode(&probe, uid).map_or(Probe::Unusable, Probe::NoAccess)
                    }
                    None => Probe::Unusable,
                },
            },
        ) {
            Some(Probe::Usable) => found.found.push(path),
            Some(Probe::OtherWritable(mode)) => found.skipped.push(SkippedTrash {
                path,
                reason: SkipReason::OtherWritable,
                mode,
            }),
            Some(Probe::NoAccess(mode)) => found.skipped.push(SkippedTrash {
                path,
                reason: SkipReason::NoAccess,
                mode,
            }),
            Some(Probe::Unusable) => {}
            None => return Err(Error::Unsupported("trashPurgeFailed")),
        }
    }
    Ok(found)
}
/// Walk pinned directory handles so every unlink is cancellable and reports progress,
/// including children of a large trashed directory. Never follow a replacement symlink.
fn remove_entry(
    parent: &File,
    name: &std::ffi::OsStr,
    job: &JobHandle,
    progress: &ProgressWatchdog,
    root_device: u64,
) -> std::io::Result<bool> {
    remove_entry_with_device(parent, name, job, progress, root_device, device)
}
fn device(file: &File) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.dev())
}
fn same_device(file: &File, root_device: u64) -> std::io::Result<()> {
    if device(file)? != root_device {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}
fn remove_entry_with_device(
    parent: &File,
    name: &std::ffi::OsStr,
    job: &JobHandle,
    progress: &ProgressWatchdog,
    root_device: u64,
    device: impl Fn(&File) -> std::io::Result<u64>,
) -> std::io::Result<bool> {
    use rustix::fs::{statat, unlinkat, AtFlags, FileType};
    use std::os::unix::ffi::OsStrExt;
    struct Frame {
        directory: File,
        entries: rustix::fs::Dir,
        name: std::ffi::OsString,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut pending = Some(name.to_os_string());
    loop {
        if progress.checkpoint(|| job.checkpoint()) {
            return Ok(false);
        }
        if let Some(name) = pending.take() {
            let parent = stack.last().map_or(parent, |frame| &frame.directory);
            let meta = statat(parent, &name, AtFlags::SYMLINK_NOFOLLOW)?;
            if FileType::from_raw_mode(meta.st_mode) == FileType::Directory {
                let child = File::from(openat(
                    parent,
                    &name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
                // Check the opened handle, not the preceding lstat: a mount or
                // replacement may have appeared between those two syscalls.
                if device(&child)? != root_device {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                let entries = rustix::fs::Dir::read_from(&child)?;
                stack.push(Frame {
                    directory: child,
                    entries,
                    name,
                });
            } else {
                if progress.checkpoint(|| job.checkpoint()) {
                    return Ok(false);
                }
                unlinkat(parent, &name, AtFlags::empty())?;
                progress.progress();
            }
        } else if let Some(frame) = stack.last_mut() {
            if let Some(entry) = frame.entries.next() {
                let entry = entry?;
                let name = entry.file_name().to_bytes();
                if name != b"." && name != b".." {
                    pending = Some(std::ffi::OsStr::from_bytes(name).to_os_string());
                }
            } else {
                let frame = stack.pop().expect("frame was present");
                let parent = stack.last().map_or(parent, |frame| &frame.directory);
                if progress.checkpoint(|| job.checkpoint()) {
                    return Ok(false);
                }
                unlinkat(parent, &frame.name, AtFlags::REMOVEDIR)?;
                progress.progress();
            }
        } else {
            return Ok(true);
        }
    }
}
fn invalidate_sizes(root: &File) -> std::io::Result<()> {
    for cache in ["directorysizes", ".trashsize"] {
        match rustix::fs::unlinkat(root, cache, rustix::fs::AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn purge(path: &Path, job: &JobHandle, progress: &ProgressWatchdog) -> std::io::Result<OpReport> {
    let table = crate::packaging::read_host_file(
        std::path::Path::new("/proc/self/mountinfo"),
        4 * 1024 * 1024,
    )?;
    purge_with_table(path, job, &table, progress)
}
fn purge_with_table(
    path: &Path,
    job: &JobHandle,
    table: &str,
    progress: &ProgressWatchdog,
) -> std::io::Result<OpReport> {
    use std::os::unix::ffi::OsStrExt;
    if !safe_path(path, &points(table)) {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    let (root, files, info) = open_trash(path, unsafe { libc::geteuid() })?;
    let root_device = device(&root)?;
    same_device(&files, root_device)?;
    same_device(&info, root_device)?;
    let mut report = OpReport::default();
    let shown = VPath::local(path).map_err(|_| std::io::ErrorKind::InvalidInput)?;
    for entry in rustix::fs::Dir::read_from(&files)? {
        if progress.checkpoint(|| job.checkpoint()) {
            report.cancelled = true;
            break;
        }
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name = std::ffi::OsStr::from_bytes(name);
        // Caches are disposable, including if cancellation leaves a partial directory.
        invalidate_sizes(&root)?;
        let result = remove_entry(&files, name, job, progress, root_device).and_then(|removed| {
            if !removed {
                return Ok(false);
            }
            let mut record = name.to_os_string();
            record.push(".trashinfo");
            match rustix::fs::unlinkat(&info, &record, rustix::fs::AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(true),
                Err(e) => Err(e.into()),
            }
        });
        if matches!(result, Ok(false)) {
            report.cancelled = true;
            break;
        }
        if result.is_ok() {
            invalidate_sizes(&root)?;
            progress.progress();
            report.done.push(shown.clone());
            job.emit(ProgressEvent::ItemDone(shown.clone()));
        } else {
            report
                .failed
                .push((shown.clone(), Error::Unsupported("trashPurgeFailed")));
        }
    }
    Ok(report)
}
/// The worker retains the mount lease after timeout until its last syscall returns.
/// Cancellation and timeout stop subsequent entries; a kernel syscall cannot be interrupted safely.
pub fn empty<G: Send + Sync + 'static>(
    dirs: Vec<PathBuf>,
    job: &JobHandle,
    guard: Arc<G>,
) -> OpReport {
    empty_with(
        dirs,
        job,
        guard,
        pool(),
        Duration::from_secs(30),
        Arc::new(purge),
    )
}
fn empty_with<
    G: Send + Sync + 'static,
    F: Fn(&Path, &JobHandle, &ProgressWatchdog) -> std::io::Result<OpReport> + Send + Sync + 'static,
>(
    dirs: Vec<PathBuf>,
    job: &JobHandle,
    guard: Arc<G>,
    workers: &Pool,
    timeout: Duration,
    purge: Arc<F>,
) -> OpReport {
    let mut report = OpReport::default();
    for path in dirs {
        if job.checkpoint() {
            report.cancelled = true;
            break;
        }
        let worker = job.clone();
        let lease = guard.clone();
        let target = path.clone();
        let purge = purge.clone();
        let result = workers.run_keyed_watchdog(
            path.to_string_lossy().into_owned(),
            timeout,
            || job.is_cancelled(),
            move |progress| {
                let _lease = lease;
                purge(&target, &worker, &progress)
            },
        );
        match result {
            Some(Ok(part)) => {
                report.done.extend(part.done);
                report.failed.extend(part.failed);
                report.cancelled |= part.cancelled;
            }
            failure => {
                if job.is_cancelled() {
                    report.cancelled = true;
                    break;
                }
                report.failed.push((
                    VPath::local(&path).expect("absolute trash directory"),
                    Error::Unsupported("trashPurgeFailed"),
                ));
                if failure.is_none() {
                    job.cancel();
                    break;
                }
            }
        }
    }
    report
}
#[cfg(test)]
mod tests {
    use super::*;
    fn trash_fixture(data: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let root = data.join("Trash");
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::create_dir(root.join("info")).unwrap();
        for path in [&root, &root.join("files"), &root.join("info")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        root
    }
    #[test]
    fn a_trash_writable_by_group_is_left_alone_but_reported() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("share");
        let root = trash_fixture(&data);
        std::fs::write(root.join("files/item"), b"trash").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
        let table = format!("1 0 8:1 / {} rw - ext4 /dev/a rw", temp.path().display());
        let found = discover_report(&table, &data).unwrap();
        assert!(found.found.is_empty());
        assert_eq!(
            found.skipped,
            vec![SkippedTrash {
                path: root.clone(),
                reason: SkipReason::OtherWritable,
                mode: 0o775
            }]
        );
        assert!(discover(&table, &data).unwrap().is_empty());
        assert!(root.join("files/item").exists());
    }
    #[test]
    fn an_own_trash_without_access_is_reported_and_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("share");
        let root = trash_fixture(&data);
        std::fs::write(root.join("files/item"), b"trash").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let table = format!("1 0 8:1 / {} rw - ext4 /dev/a rw", temp.path().display());
        let found = discover_report(&table, &data).unwrap();
        let sub = root.join("files");
        // Restore access before any assertion so the temp dir can be removed.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(found.found.is_empty());
        assert_eq!(
            found.skipped,
            vec![SkippedTrash {
                path: root.clone(),
                reason: SkipReason::NoAccess,
                mode: 0
            }]
        );
        assert!(sub.join("item").exists());
        // Only the unreadable root is refused; a missing trash stays silent.
        let none = discover_report(&table, &temp.path().join("other")).unwrap();
        assert!(none.found.is_empty() && none.skipped.is_empty());
    }
    #[test]
    fn an_unreadable_files_folder_reports_its_own_mode() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("share");
        let root = trash_fixture(&data);
        std::fs::set_permissions(root.join("files"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        let table = format!("1 0 8:1 / {} rw - ext4 /dev/a rw", temp.path().display());
        let found = discover_report(&table, &data).unwrap();
        std::fs::set_permissions(root.join("files"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(found.found.is_empty());
        assert_eq!(found.skipped.len(), 1);
        assert_eq!(found.skipped[0].reason, SkipReason::NoAccess);
        assert_eq!(found.skipped[0].mode, 0);
    }
    #[test]
    fn a_symlinked_data_parent_is_resolved_before_mount_classification() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real-home");
        let data = real.join("share");
        let root = trash_fixture(&data);
        std::fs::write(root.join("files/item"), b"trash").unwrap();
        let alias = temp.path().join("home");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        // The lexical alias lies on an excluded mount, but its target is local.
        let table = format!(
            "1 0 0:1 / / rw - nfs host:/ rw\n2 1 8:1 / {} rw - ext4 /dev/a rw",
            real.display()
        );
        assert!(!safe_path(&alias.join("share/Trash"), &points(&table)));
        let found = discover(&table, &alias.join("share")).unwrap();
        assert_eq!(found, vec![root.clone()]);
        let report = purge_with_table(
            &found[0],
            &JobHandle::silent(),
            &table,
            &ProgressWatchdog::default(),
        )
        .unwrap();
        assert_eq!(report.done.len(), 1);
        assert!(report.failed.is_empty());
        assert!(!root.join("files/item").exists());
        // Canonicalization must stop before Trash: its own symlink is untrusted.
        std::fs::rename(&root, data.join("actual-trash")).unwrap();
        std::os::unix::fs::symlink(data.join("actual-trash"), &root).unwrap();
        assert!(discover(&table, &alias.join("share")).unwrap().is_empty());
    }
    #[test]
    fn iterative_removal_handles_more_than_256_levels() {
        let temp = tempfile::tempdir().unwrap();
        let root = trash_fixture(temp.path());
        let mut bottom = root.join("files/tree");
        for _ in 0..300 {
            bottom.push("d");
        }
        std::fs::create_dir_all(&bottom).unwrap();
        std::fs::write(bottom.join("payload"), b"trash").unwrap();
        let report = purge_with_table(
            &root,
            &JobHandle::silent(),
            "1 0 8:1 / / rw - ext4 /dev/a rw",
            &ProgressWatchdog::default(),
        )
        .unwrap();
        assert_eq!(report.done.len(), 1);
        assert!(report.failed.is_empty());
        assert!(!root.join("files/tree").exists());
    }
    #[test]
    fn a_new_device_inside_a_trashed_tree_is_not_descended_into() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let mounted = temp.path().join("tree/new-mount");
        std::fs::create_dir_all(&mounted).unwrap();
        std::fs::write(mounted.join("keep"), b"keep").unwrap();
        let mounted_inode = std::fs::metadata(&mounted).unwrap().ino();
        let root = open_dir(temp.path()).unwrap();
        let root_device = device(&root).unwrap();
        // Simulate a new mount in the metadata of the actual opened directory.
        // No privileged mount operation is needed to exercise the device boundary.
        let result = remove_entry_with_device(
            &root,
            std::ffi::OsStr::new("tree"),
            &JobHandle::silent(),
            &ProgressWatchdog::default(),
            root_device,
            |file| {
                let metadata = file.metadata()?;
                Ok(if metadata.ino() == mounted_inode {
                    root_device + 1
                } else {
                    metadata.dev()
                })
            },
        );
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(mounted.join("keep")).unwrap(), b"keep");
        assert!(same_device(&root, root_device).is_ok());
        assert!(same_device(&root, root_device + 1).is_err());
    }
    #[test]
    fn healthy_progress_can_take_longer_than_the_old_total_budget() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..24 {
            std::fs::write(temp.path().join(i.to_string()), b"data").unwrap();
        }
        let started = std::time::Instant::now();
        let job = JobHandle::silent();
        let report = empty_with(
            vec![temp.path().to_owned()],
            &job,
            Arc::new(()),
            &Pool::new(1),
            Duration::from_millis(500),
            Arc::new(|path: &Path, _: &JobHandle, progress: &ProgressWatchdog| {
                let mut report = OpReport::default();
                for i in 0..24 {
                    std::thread::sleep(Duration::from_millis(100));
                    let file = path.join(i.to_string());
                    std::fs::remove_file(&file)?;
                    progress.progress();
                    report.done.push(VPath::local(file).unwrap());
                }
                Ok(report)
            }),
        );
        assert!(started.elapsed() > Duration::from_secs(2));
        assert_eq!(report.done.len(), 24);
        assert!(report.failed.is_empty());
        assert!(!job.is_cancelled());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn trash_on_local_block_and_overlay_filesystems_is_included() {
        for fs in ["bcachefs", "jfs", "overlay", "fuseblk"] {
            let table = format!("1 0 8:1 / / rw - {fs} /dev/a rw");
            assert!(candidates(&table, Some(Path::new("/home/data")), 1000)
                .contains(&PathBuf::from("/home/data/Trash")));
        }
        for fs in ["autofs", "nfs", "fuse.sshfs", "fuse"] {
            assert!(!trash_local_type(fs));
        }
    }
    #[test]
    fn timed_out_purge_cancels_and_retains_its_guard_until_the_worker_exits() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let guard = Arc::new(Guard(released.clone()));
        let (send, recv) = std::sync::mpsc::channel::<()>();
        let recv = std::sync::Mutex::new(recv);
        let job = JobHandle::silent();
        let report = empty_with(
            vec!["/home/test/.local/share/Trash".into()],
            &job,
            guard,
            &Pool::new(1),
            Duration::from_millis(20),
            Arc::new(move |_: &Path, _: &JobHandle, _: &ProgressWatchdog| {
                recv.lock().unwrap().recv().unwrap();
                Ok(OpReport::default())
            }),
        );
        assert_eq!(report.failed.len(), 1);
        assert!(job.is_cancelled());
        assert!(!released.load(Ordering::SeqCst));
        send.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !released.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn candidates_skip_network_autofs_and_nested_mounts_without_probing() {
        let table = "1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 0:2 / /net rw - nfs host:/ rw\n3 1 0:3 / /auto rw - autofs auto rw\n4 1 0:4 / /data/Trash/files/dead rw - nfs host:/ rw";
        let paths = candidates(table, Some(Path::new("/data")), 1000);
        assert!(!paths
            .iter()
            .any(|p| p.starts_with("/net") || p.starts_with("/auto") || p.starts_with("/data")));
        assert!(paths.contains(&PathBuf::from("/.Trash-1000")));
    }
    #[test]
    fn purge_cancels_and_unlinks_symlinks_without_following_them() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Trash");
        for p in [&root, &root.join("files"), &root.join("info")] {
            std::fs::create_dir(p).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let victim = temp.path().join("keep");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, root.join("files/link")).unwrap();
        std::fs::write(root.join("info/link.trashinfo"), b"info").unwrap();
        std::fs::write(root.join("directorysizes"), b"stale").unwrap();
        std::fs::write(root.join(".trashsize"), b"stale").unwrap();
        let table = "1 0 8:1 / / rw - ext4 /dev/a rw";
        assert!(discover(table, temp.path()).unwrap().contains(&root));
        for component in [&root, &root.join("files"), &root.join("info")] {
            std::fs::set_permissions(component, std::fs::Permissions::from_mode(0o775)).unwrap();
            assert!(open_trash(&root, unsafe { libc::geteuid() }).is_err());
            assert!(!discover(table, temp.path()).unwrap().contains(&root));
            std::fs::set_permissions(component, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let nested = root.join("files/directory/subdir");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("payload"), b"payload").unwrap();
        std::os::unix::fs::symlink(&victim, nested.join("link")).unwrap();
        let cancelled = JobHandle::silent();
        cancelled.cancel();
        assert!(empty(vec![root.clone()], &cancelled, Arc::new(())).cancelled);
        assert!(root.join("files/link").symlink_metadata().is_ok());
        let report = purge_with_table(
            &root,
            &JobHandle::silent(),
            "1 0 8:1 / / rw - ext4 /dev/a rw",
            &ProgressWatchdog::default(),
        )
        .unwrap();
        assert_eq!(report.done.len(), 2);
        assert!(report.failed.is_empty());
        assert!(victim.exists());
        assert!(std::fs::symlink_metadata(root.join("files/link")).is_err());
        assert!(!root.join("files/directory").exists());
        assert!(!root.join("info/link.trashinfo").exists());
        assert!(!root.join("directorysizes").exists());
        assert!(!root.join(".trashsize").exists());
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::remove_dir(root.join("files")).unwrap();
        std::os::unix::fs::symlink(outside, root.join("files")).unwrap();
        assert!(open_trash(&root, unsafe { libc::geteuid() }).is_err());
    }
}
