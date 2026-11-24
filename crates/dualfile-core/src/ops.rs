use crate::error::{Error, Result};
use crate::names::{keep_both_name, validate_name};
use crate::resume::SourceIdentity;
use crate::vfs::{Entry, EntryKind, ReplaceMode, Scheme, VPath, Vfs, WriteFinish, WriteMode};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Debug)]
pub enum ProgressEvent {
    ScanProgress {
        entries: u64,
        bytes: u64,
    },
    BytesCopied {
        path: VPath,
        bytes: u64,
        total_bytes: u64,
    },
    ItemDone(VPath),
    ItemFailed(VPath),
    ItemSkipped(VPath),
}
pub trait ProgressSink: Send + Sync {
    fn event(&self, event: ProgressEvent);
}
struct Silent;
impl ProgressSink for Silent {
    fn event(&self, _: ProgressEvent) {}
}
pub(crate) trait JobControl: Send + Sync {
    fn cancel(&self);
    fn is_cancelled(&self) -> bool;
    fn pause(&self);
    fn resume(&self);
    fn is_paused(&self) -> bool;
    fn checkpoint(&self) -> bool;
}
#[derive(Default)]
struct LocalControl {
    cancelled: AtomicBool,
    paused: Mutex<bool>,
    changed: Condvar,
}
impl JobControl for LocalControl {
    fn cancel(&self) {
        let _guard = self.paused.lock().unwrap();
        self.cancelled.store(true, Ordering::SeqCst);
        self.changed.notify_all();
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn pause(&self) {
        *self.paused.lock().unwrap() = true;
    }
    fn resume(&self) {
        *self.paused.lock().unwrap() = false;
        self.changed.notify_all();
    }
    fn is_paused(&self) -> bool {
        *self.paused.lock().unwrap()
    }
    fn checkpoint(&self) -> bool {
        let mut paused = self.paused.lock().unwrap();
        while *paused && !self.is_cancelled() {
            paused = self.changed.wait(paused).unwrap();
        }
        self.is_cancelled()
    }
}
#[derive(Clone)]
pub struct JobHandle {
    control: Arc<dyn JobControl>,
    sink: Arc<dyn ProgressSink>,
    verify: VerifyMode,
    error_resolver: Option<Arc<dyn ErrorResolver>>,
}
impl JobHandle {
    pub fn new(sink: Arc<dyn ProgressSink>) -> Self {
        Self::controlled(sink, Arc::new(LocalControl::default()))
    }
    pub(crate) fn controlled(sink: Arc<dyn ProgressSink>, control: Arc<dyn JobControl>) -> Self {
        Self {
            control,
            sink,
            verify: VerifyMode::Off,
            error_resolver: None,
        }
    }
    pub fn with_error_resolver(&self, resolver: Option<Arc<dyn ErrorResolver>>) -> Self {
        let mut job = self.clone();
        job.error_resolver = resolver;
        job
    }
    pub fn silent() -> Self {
        Self::new(Arc::new(Silent))
    }
    pub fn cancel(&self) {
        self.control.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.control.is_cancelled()
    }
    pub fn pause(&self) {
        self.control.pause();
    }
    pub fn resume(&self) {
        self.control.resume();
    }
    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }
    /// Wait at a cooperative boundary; returns true when cancelled.
    pub fn checkpoint(&self) -> bool {
        self.control.checkpoint()
    }
    /// Absolute counters for read-only scans using this job's existing progress sink.
    pub fn scan_progress(&self, entries: u64, bytes: u64) {
        self.emit(ProgressEvent::ScanProgress { entries, bytes });
    }
    pub(crate) fn emit(&self, event: ProgressEvent) {
        self.sink.event(event);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifyMode {
    Off,
    Sha256,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyPolicy {
    Never,
    #[default]
    Auto,
    Always,
}

pub fn should_verify(dest: &VPath, policy: VerifyPolicy) -> bool {
    match policy {
        VerifyPolicy::Never => false,
        VerifyPolicy::Always => true,
        VerifyPolicy::Auto if dest.scheme() != Scheme::Local => true,
        VerifyPolicy::Auto => removable(dest.path()),
    }
}

#[cfg(target_os = "linux")]
fn removable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    // Resolve the nearest existing ancestor for a not-yet-created destination.
    let mut existing = None;
    for ancestor in path.ancestors() {
        match std::fs::metadata(ancestor) {
            Ok(meta) => {
                existing = Some(meta);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    let Some(meta) = existing else {
        return false;
    };
    let dev = meta.dev();
    let device = format!(
        "/sys/dev/block/{}:{}",
        rustix::fs::major(dev),
        rustix::fs::minor(dev)
    );
    let Ok(device) = std::fs::canonicalize(device) else {
        return false;
    };
    for ancestor in device.ancestors() {
        if let Ok(flag) = std::fs::read_to_string(ancestor.join("removable")) {
            return flag.trim() == "1";
        }
    }
    false
}
#[cfg(not(target_os = "linux"))]
fn removable(_: &std::path::Path) -> bool {
    false
}

/// Copy with an explicit checksum policy; existing `copy` callers use Off.
pub fn copy_verified(
    sv: &dyn Vfs,
    sources: &[VPath],
    dv: &dyn Vfs,
    dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    verify: VerifyMode,
) -> OpReport {
    let mut job = job.clone();
    job.verify = verify;
    let mut report = copy(sv, sources, dv, dir, resolver, &job);
    report.verified = verify == VerifyMode::Sha256
        && !report.done.is_empty()
        && report.failed.is_empty()
        && report.skipped.is_empty()
        && !report.cancelled;
    report
}

/// A verified move uses copy/verify/delete, including on the same device.
pub fn move_verified(
    sv: &dyn Vfs,
    sources: &[VPath],
    dv: &dyn Vfs,
    dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    verify: VerifyMode,
) -> OpReport {
    let mut job = job.clone();
    job.verify = verify;
    let mut report = move_with_strategy(
        sv,
        sources,
        dv,
        dir,
        resolver,
        &job,
        if verify == VerifyMode::Off {
            MoveStrategy::Auto
        } else {
            MoveStrategy::ForceCopy
        },
    );
    report.verified = verify == VerifyMode::Sha256
        && !report.done.is_empty()
        && report.failed.is_empty()
        && report.skipped.is_empty()
        && !report.cancelled;
    report
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution {
    Overwrite,
    Skip,
    KeepBoth,
    Cancel,
    OverwriteAll,
    SkipAll,
    KeepBothAll,
}
#[derive(Clone, Debug)]
pub struct Conflict {
    pub source: Entry,
    pub destination: Entry,
    pub source_path: VPath,
    pub destination_path: VPath,
}
pub trait ConflictResolver: Send + Sync {
    fn resolve(&self, conflict: &Conflict) -> Resolution;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorDecision {
    Skip,
    Retry,
    Abort,
}

pub trait ErrorResolver: Send + Sync {
    fn resolve(&self, path: &VPath, error: &Error) -> ErrorDecision;
}

// Every retry requires a fresh decision; there is no unbounded automatic retry loop.
fn retry_io<T>(job: &JobHandle, path: &VPath, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    loop {
        if job.checkpoint() {
            return Err(Error::Cancelled);
        }
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error @ Error::Exists(_)) => return Err(error),
            Err(error) => match job.error_resolver.as_ref().map(|r| r.resolve(path, &error)) {
                Some(ErrorDecision::Retry) => continue,
                Some(ErrorDecision::Abort) => {
                    job.cancel();
                    return Err(Error::Cancelled);
                }
                _ => return Err(error),
            },
        }
    }
}

#[derive(Default)]
pub struct OpReport {
    /// Actual newly created destinations, including conflict-renamed targets.
    pub created: Vec<(VPath, VPath)>,
    /// A destination was successfully overwritten anywhere in this operation.
    pub replaced: bool,
    pub verified: bool,
    pub done: Vec<VPath>,
    pub failed: Vec<(VPath, Error)>,
    pub skipped: Vec<VPath>,
    pub warnings: Vec<(VPath, String)>,
    pub cancelled: bool,
}
fn failed(report: &mut OpReport, path: &VPath, error: Error, job: &JobHandle) {
    job.emit(ProgressEvent::ItemFailed(path.clone()));
    report.failed.push((path.clone(), error));
}

fn join(path: &VPath, name: &str) -> Result<VPath> {
    path.join_entry(name)
}

const READ_ONLY_MESSAGE: &str = "Archive sind schreibgeschützt";

/// Marks every item as failed because a read-only back end (an archive) is involved.
fn reject_read_only(items: &[VPath], job: &JobHandle) -> OpReport {
    let mut report = OpReport::default();
    for item in items {
        failed(
            &mut report,
            item,
            Error::Unsupported(READ_ONLY_MESSAGE),
            job,
        );
    }
    report
}

/// A link read from a non-local source (an archive) may only be created when its target is
/// a plain relative path without `..`: it can then never point outside the copied tree.
fn archive_link_is_safe(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('/')
        && !target.contains('\0')
        && !target.contains('\\')
        && target.split('/').all(|part| part != "..")
        && !(target.len() >= 2 && target.as_bytes()[1] == b':')
}

#[cfg(unix)]
fn identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let resolved = std::fs::canonicalize(path).ok()?;
    let metadata = std::fs::metadata(resolved).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

fn source_destination_error(src: &VPath, dst_dir: &VPath) -> Option<Error> {
    if src.scheme().is_remote()
        && src.scheme() == dst_dir.scheme()
        && src.conn_id() == dst_dir.conn_id()
    {
        // Same server connection: the paths are compared as text (the server's own links
        // cannot be resolved from here).
        if src.path().parent() == Some(dst_dir.path()) || src.path() == dst_dir.path() {
            return Some(Error::SameFile);
        }
        if dst_dir.path().starts_with(src.path()) {
            return Some(Error::DestinationInsideSource);
        }
        return None;
    }
    if src.scheme() != Scheme::Local || dst_dir.scheme() != Scheme::Local {
        return None; // paths inside an archive or on a server never name local files
    }
    #[cfg(unix)]
    {
        let source_parent = src.path().parent()?;
        let source_identity = identity(source_parent)?;
        let destination_identity = identity(dst_dir.path())?;
        if source_identity == destination_identity {
            return Some(Error::SameFile);
        }
        let resolved_source = std::fs::canonicalize(src.path()).ok()?;
        let resolved_destination = std::fs::canonicalize(dst_dir.path()).ok()?;
        if resolved_destination == resolved_source {
            return Some(Error::SameFile);
        }
        if resolved_destination.starts_with(resolved_source) {
            return Some(Error::DestinationInsideSource);
        }
    }
    #[cfg(not(unix))]
    let _ = (src, dst_dir);
    None
}

/// Where the source entry is read from: through the `Vfs` trait, or (local Linux sources)
/// relative to an already opened parent directory handle, so a path swap after opening
/// cannot lead out of the source tree.
#[derive(Clone, Copy)]
enum Origin<'a> {
    Vfs(std::marker::PhantomData<&'a ()>),
    #[cfg(target_os = "linux")]
    Handle {
        dir: rustix::fd::BorrowedFd<'a>,
        name: &'a std::path::Path,
    },
}

struct SourceChild {
    name: String,
    os_name: std::ffi::OsString,
    kind: Option<EntryKind>,
    error: Option<String>,
}

#[cfg(target_os = "linux")]
fn errno_error(op: &'static str, path: &VPath, error: rustix::io::Errno) -> Error {
    crate::error::from_io(op, path.path(), std::io::Error::from(error))
}

/// Opens one source directory relative to its parent handle (root: absolute path, `CWD`
/// is ignored for absolute names). `NOFOLLOW` rejects a swap to a symlink.
#[cfg(target_os = "linux")]
fn open_directory_at(
    dir: rustix::fd::BorrowedFd<'_>,
    name: &std::path::Path,
    shown: &VPath,
) -> Result<rustix::fd::OwnedFd> {
    use rustix::fs::{openat, Mode, OFlags};
    openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| errno_error("Quellordner sicher öffnen", shown, error))
}

/// What of the source is read once to be carried over to the copy.
#[derive(Clone, Copy)]
struct SrcMeta {
    kind: EntryKind,
    size: u64,
    mode: Option<u32>,
    modified: Option<std::time::SystemTime>,
}

fn origin_stat(origin: Origin<'_>, sv: &dyn Vfs, src: &VPath) -> Result<SrcMeta> {
    match origin {
        Origin::Vfs(_) => sv.stat(src).map(|entry| SrcMeta {
            kind: entry.kind,
            size: entry.size,
            mode: entry.mode,
            modified: entry.modified,
        }),
        #[cfg(target_os = "linux")]
        Origin::Handle { dir, name } => {
            use rustix::fs::{statat, AtFlags, FileType};
            let stat = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| errno_error("Datei-Information lesen", src, error))?;
            let (kind, size) = match FileType::from_raw_mode(stat.st_mode as _) {
                FileType::Directory => (EntryKind::Dir, 0),
                FileType::Symlink => (EntryKind::Symlink, 0),
                _ => (EntryKind::File, stat.st_size as u64),
            };
            let modified = u64::try_from(stat.st_mtime)
                .ok()
                .map(|secs| {
                    std::time::UNIX_EPOCH
                        + std::time::Duration::new(secs, stat.st_mtime_nsec as u32)
                })
                .or_else(|| {
                    // Before 1970: seconds negative, nanoseconds still count forward.
                    let before = std::time::UNIX_EPOCH.checked_sub(
                        std::time::Duration::from_secs(stat.st_mtime.unsigned_abs()),
                    )?;
                    Some(before + std::time::Duration::from_nanos(stat.st_mtime_nsec as u64))
                });
            Ok(SrcMeta {
                kind,
                size,
                mode: Some(stat.st_mode as u32),
                modified,
            })
        }
    }
}

fn origin_read_link(origin: Origin<'_>, sv: &dyn Vfs, src: &VPath) -> Result<String> {
    match origin {
        Origin::Vfs(_) => sv.read_link(src),
        #[cfg(target_os = "linux")]
        Origin::Handle { dir, name } => rustix::fs::readlinkat(dir, name, Vec::new())
            .map(|target| target.to_string_lossy().into_owned())
            .map_err(|error| errno_error("Linkziel lesen", src, error)),
    }
}

fn origin_open_read(origin: Origin<'_>, sv: &dyn Vfs, src: &VPath) -> Result<Box<dyn Read + Send>> {
    match origin {
        Origin::Vfs(_) => sv.open_read(src),
        #[cfg(target_os = "linux")]
        Origin::Handle { dir, name } => {
            use rustix::fs::{fstat, openat, FileType, Mode, OFlags};
            // NONBLOCK: a file swapped for a FIFO must not hang the job.
            let fd = openat(
                dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| errno_error("Datei öffnen", src, error))?;
            let stat = fstat(&fd).map_err(|error| errno_error("Datei öffnen", src, error))?;
            match FileType::from_raw_mode(stat.st_mode as _) {
                FileType::RegularFile => Ok(Box::new(std::fs::File::from(fd))),
                FileType::Directory => Err(Error::IsADirectory(src.path().to_path_buf())),
                _ => Err(Error::InvalidPath(
                    "Der Quelleneintrag wurde während des Kopierens ausgetauscht".into(),
                )),
            }
        }
    }
}

/// Like `origin_open_read`, starting at `offset` (continuing a partial copy).
fn origin_open_read_from(
    origin: Origin<'_>,
    sv: &dyn Vfs,
    src: &VPath,
    offset: u64,
) -> Result<Box<dyn Read + Send>> {
    if offset == 0 {
        return origin_open_read(origin, sv, src);
    }
    match origin {
        Origin::Vfs(_) => sv.open_read_from(src, offset),
        #[cfg(target_os = "linux")]
        Origin::Handle { .. } => {
            let mut reader = origin_open_read(origin, sv, src)?;
            let skipped = std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())
                .map_err(|e| crate::error::from_io("Datei lesen", src.path(), e))?;
            if skipped != offset {
                return Err(Error::SizeMismatch(src.path().to_path_buf()));
            }
            Ok(reader)
        }
    }
}

#[cfg(target_os = "linux")]
fn list_handle(handle: &rustix::fd::OwnedFd, shown: &VPath) -> Result<Vec<SourceChild>> {
    use rustix::fs::FileType;
    use std::os::unix::ffi::OsStrExt;
    let to_error =
        |error: std::io::Error| crate::error::from_io("Ordner auflisten", shown.path(), error);
    let dir = rustix::fs::Dir::read_from(handle)
        .map_err(|error| to_error(std::io::Error::from(error)))?;
    let mut children = Vec::new();
    for item in dir {
        let item = item.map_err(|error| to_error(std::io::Error::from(error)))?;
        let raw = item.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        let os_name = std::ffi::OsStr::from_bytes(raw).to_os_string();
        let kind = match item.file_type() {
            FileType::Directory => Some(EntryKind::Dir),
            FileType::Symlink => Some(EntryKind::Symlink),
            FileType::Unknown => None,
            _ => Some(EntryKind::File),
        };
        let name = os_name.to_string_lossy().into_owned();
        let error =
            (os_name.to_str().is_none()).then(|| "Der Name ist kein gültiges Unicode".to_string());
        children.push(SourceChild {
            name,
            os_name,
            kind,
            error,
        });
    }
    Ok(children)
}

fn decide(
    src_vfs: &dyn Vfs,
    dst_vfs: &dyn Vfs,
    src: &VPath,
    dst: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    report: &mut OpReport,
    job: &JobHandle,
) -> std::result::Result<Option<(VPath, bool)>, (VPath, Error)> {
    let dest = match dst_vfs.stat(dst) {
        Ok(entry) => entry,
        Err(Error::NotFound(_)) => return Ok(Some((dst.clone(), false))),
        Err(e) => {
            return Err((dst.clone(), e));
        }
    };
    let source = match src_vfs.stat(src) {
        Ok(entry) => entry,
        Err(e) => {
            return Err((src.clone(), e));
        }
    };
    if source.kind == EntryKind::Dir && dest.kind == EntryKind::Dir {
        return Ok(Some((dst.clone(), false)));
    }
    let source_kind = source.kind;
    let dest_kind = dest.kind;
    let resolution = resolver
        .map(|r| {
            r.resolve(&Conflict {
                source,
                destination: dest,
                source_path: src.clone(),
                destination_path: dst.clone(),
            })
        })
        .unwrap_or(Resolution::Skip);
    Ok(match resolution {
        Resolution::Skip | Resolution::SkipAll => {
            report.skipped.push(src.clone());
            job.emit(ProgressEvent::ItemSkipped(src.clone()));
            None
        }
        Resolution::Cancel => {
            report.cancelled = true;
            // Stop the siblings too, and keep a move from treating the rest as complete.
            job.cancel();
            None
        }
        Resolution::KeepBoth | Resolution::KeepBothAll => {
            let name = match keep_both_name(
                dst.path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
                |n| match dst.sibling(n) {
                    // Only a clear "not there" frees a name; any other answer counts as taken.
                    Ok(candidate) => !matches!(dst_vfs.stat(&candidate), Err(Error::NotFound(_))),
                    Err(_) => true,
                },
            ) {
                Ok(v) => v,
                Err(e) => {
                    return Err((dst.clone(), e));
                }
            };
            Some((dst.sibling(&name).map_err(|e| (dst.clone(), e))?, false))
        }
        Resolution::Overwrite | Resolution::OverwriteAll if source_kind != dest_kind => {
            return Err((dst.clone(), Error::Exists(dst.path().to_path_buf())));
        }
        Resolution::Overwrite | Resolution::OverwriteAll => Some((dst.clone(), true)),
    })
}

pub fn copy(
    src_vfs: &dyn Vfs,
    sources: &[VPath],
    dst_vfs: &dyn Vfs,
    dst_dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
) -> OpReport {
    if dst_vfs.is_read_only() {
        return reject_read_only(sources, job);
    }
    let mut report = OpReport::default();
    let remembered = RememberAll::new(resolver);
    let resolver = resolver.map(|_| &remembered as &dyn ConflictResolver);
    for src in sources {
        if job.checkpoint() {
            report.cancelled = true;
            break;
        }
        let source = match retry_io(job, src, || src_vfs.stat(src)) {
            Ok(v) => v,
            Err(e) => {
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
                continue;
            }
        };
        if let Some(error) = source_destination_error(src, dst_dir) {
            failed(&mut report, src, error, job);
            continue;
        }
        let dst = match join(dst_dir, &source.name) {
            Ok(v) => v,
            Err(e) => {
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
                continue;
            }
        };
        copy_one(src_vfs, src, dst_vfs, &dst, resolver, job, &mut report);
    }
    report
}

/// Copies every source next to itself under a free name (`name_copy`, `name_copy2`, ...),
/// without asking. The name is chosen by looking at the folder; should another process
/// create it in between, the copy meets a conflict that is answered with "skip" (no resolver),
/// so an existing file is never overwritten. Folders are copied the same way as in `copy`.
pub fn duplicate(vfs: &dyn Vfs, sources: &[VPath], job: &JobHandle) -> OpReport {
    if vfs.is_read_only() {
        return reject_read_only(sources, job);
    }
    let mut report = OpReport::default();
    for src in sources {
        if job.checkpoint() {
            report.cancelled = true;
            break;
        }
        let source = match retry_io(job, src, || vfs.stat(src)) {
            Ok(v) => v,
            Err(e) => {
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
                continue;
            }
        };
        // Only a clear "not there" frees a name and an existing entry takes it. Any other answer
        // (lost connection, no permission) ends the search with that error instead of counting up
        // forever; a cancelled job ends it as well.
        let mut probe_error: Option<Error> = None;
        let mut stopped = false;
        let dst = crate::names::copy_name(&source.name, source.kind == EntryKind::Dir, |n| {
            if stopped || probe_error.is_some() {
                return false;
            }
            if job.checkpoint() {
                stopped = true;
                return false;
            }
            match src.sibling(n) {
                Ok(candidate) => match vfs.stat(&candidate) {
                    Ok(_) => true,
                    Err(Error::NotFound(_)) => false,
                    Err(e) => {
                        probe_error = Some(e);
                        false
                    }
                },
                Err(_) => true,
            }
        })
        .and_then(|name| src.sibling(&name));
        if stopped {
            report.cancelled = true;
            break;
        }
        if let Some(e) = probe_error {
            failed(&mut report, src, e, job);
            continue;
        }
        match dst {
            Ok(dst) => copy_one(vfs, src, vfs, &dst, None, job, &mut report),
            Err(e) => failed(&mut report, src, e, job),
        }
    }
    report
}

struct RememberAll<'a> {
    inner: Option<&'a dyn ConflictResolver>,
    remembered: std::sync::Mutex<Option<Resolution>>,
}
impl<'a> RememberAll<'a> {
    fn new(inner: Option<&'a dyn ConflictResolver>) -> Self {
        Self {
            inner,
            remembered: std::sync::Mutex::new(None),
        }
    }
}
impl ConflictResolver for RememberAll<'_> {
    fn resolve(&self, conflict: &Conflict) -> Resolution {
        let mut remembered = self.remembered.lock().unwrap();
        if let Some(choice) = *remembered {
            return choice;
        }
        let choice = self
            .inner
            .map(|inner| inner.resolve(conflict))
            .unwrap_or(Resolution::Skip);
        *remembered = match choice {
            Resolution::OverwriteAll => Some(Resolution::Overwrite),
            Resolution::SkipAll => Some(Resolution::Skip),
            Resolution::KeepBothAll => Some(Resolution::KeepBoth),
            _ => None,
        };
        choice
    }
}
fn copy_one(
    sv: &dyn Vfs,
    src: &VPath,
    dv: &dyn Vfs,
    dest: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    report: &mut OpReport,
) {
    #[cfg(target_os = "linux")]
    let origin = if sv.is_local() {
        Origin::Handle {
            dir: rustix::fs::CWD,
            name: src.path(),
        }
    } else {
        Origin::Vfs(std::marker::PhantomData)
    };
    #[cfg(not(target_os = "linux"))]
    let origin = Origin::Vfs(std::marker::PhantomData);
    copy_one_attempt(sv, origin, src, dv, dest, resolver, job, report, 0);
}

/// Carries modification time and permissions of the source over to the finished copy.
/// Only the lower 0o777 bits are taken: setuid/setgid/sticky on a fresh copy would hand
/// the copier's new file special powers the user did not ask for (like plain `cp`).
/// Failures never fail the copy: they become a warning for that item. A Vfs without
/// support (`Unsupported`) is expected for non-local back ends and stays silent, so a
/// large copy does not flood the report with one identical warning per file.
fn preserve_metadata(
    dv: &dyn Vfs,
    target: &VPath,
    mode: Option<u32>,
    modified: Option<std::time::SystemTime>,
    report: &mut OpReport,
) {
    if mode.is_none() && modified.is_none() {
        return;
    }
    match dv.set_metadata(target, mode.map(|m| m & 0o777), modified) {
        Ok(()) | Err(Error::Unsupported(_)) => {}
        Err(e) => report.warnings.push((
            target.clone(),
            format!("Rechte oder Änderungszeit konnten nicht übernommen werden: {e}"),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_one_attempt(
    sv: &dyn Vfs,
    origin: Origin<'_>,
    src: &VPath,
    dv: &dyn Vfs,
    dest: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    report: &mut OpReport,
    retries: u8,
) {
    loop {
        let Err((path, error)) =
            copy_one_inner(sv, origin, src, dv, dest, resolver, job, report, retries)
        else {
            return;
        };
        if job.is_cancelled() {
            report.cancelled = true;
            return;
        }
        match job
            .error_resolver
            .as_ref()
            .map(|r| r.resolve(&path, &error))
        {
            Some(ErrorDecision::Retry) => continue,
            Some(ErrorDecision::Abort) => {
                job.cancel();
                report.cancelled = true;
                return;
            }
            _ => {
                failed(report, &path, error, job);
                return;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_one_inner(
    sv: &dyn Vfs,
    origin: Origin<'_>,
    src: &VPath,
    dv: &dyn Vfs,
    dest: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    report: &mut OpReport,
    retries: u8,
) -> std::result::Result<(), (VPath, Error)> {
    if job.checkpoint() {
        report.cancelled = true;
        return Ok(());
    }
    let SrcMeta {
        kind: entry_kind,
        size: entry_size,
        mode: source_mode,
        modified: source_modified,
    } = match origin_stat(origin, sv, src) {
        Ok(e) => e,
        Err(e) => {
            return Err((src.clone(), e));
        }
    };
    let (target, replace) = match decide(sv, dv, src, dest, resolver, report, job)? {
        Some(v) => v,
        None => return Ok(()),
    };
    let existing = dv.stat(&target).ok();
    if entry_kind == EntryKind::Dir {
        // The handle stays open until this directory is finished; children are opened
        // relative to it.
        #[cfg(target_os = "linux")]
        let handle = match origin {
            Origin::Handle { dir, name } => match open_directory_at(dir, name, src) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    return Err((src.clone(), error));
                }
            },
            Origin::Vfs(_) => None,
        };
        let created_dir = existing.is_none();
        if created_dir {
            if let Err(e) = dv.mkdir(&target) {
                return Err((target.clone(), e));
            }
        }
        let list_result: Result<Vec<SourceChild>> = {
            #[cfg(target_os = "linux")]
            let from_handle = handle.as_ref().map(|handle| list_handle(handle, src));
            #[cfg(not(target_os = "linux"))]
            let from_handle: Option<Result<Vec<SourceChild>>> = None;
            match from_handle {
                Some(result) => result,
                None => sv.list(src).map(|entries| {
                    entries
                        .into_iter()
                        .map(|child| SourceChild {
                            os_name: child.name.clone().into(),
                            name: child.name,
                            kind: Some(child.kind),
                            error: child.error,
                        })
                        .collect()
                }),
            }
        };
        let mut children = match list_result {
            Ok(v) => v,
            Err(e) => {
                return Err((src.clone(), e));
            }
        };
        children.sort_by(|left, right| left.name.cmp(&right.name));
        for child in children {
            if let Some(message) = child.error {
                let error = Error::InvalidPath(message);
                let Some(errors) = job.error_resolver.as_ref() else {
                    failed(report, src, error, job);
                    continue;
                };
                let path = join(src, &child.name).map_err(|e| (src.clone(), e))?;
                match errors.resolve(&path, &error) {
                    ErrorDecision::Skip => {
                        failed(report, &path, error, job);
                        continue;
                    }
                    ErrorDecision::Abort => {
                        job.cancel();
                        report.cancelled = true;
                        return Ok(());
                    }
                    // Re-read this child's metadata below; a failed listing must not
                    // permanently hide a file whose permissions have since been fixed.
                    ErrorDecision::Retry => {}
                }
            }
            #[cfg(target_os = "linux")]
            let child_origin = match handle.as_ref() {
                Some(handle) => Origin::Handle {
                    dir: std::os::fd::AsFd::as_fd(handle),
                    name: std::path::Path::new(&child.os_name),
                },
                None => Origin::Vfs(std::marker::PhantomData),
            };
            #[cfg(not(target_os = "linux"))]
            let child_origin = Origin::Vfs(std::marker::PhantomData);
            match (join(src, &child.name), join(&target, &child.name)) {
                (Ok(sp), Ok(dp)) => match origin_stat(child_origin, sv, &sp) {
                    Ok(SrcMeta { kind: current, .. })
                        if child.kind.is_some_and(|kind| kind != current) =>
                    {
                        failed(
                            report,
                            &sp,
                            Error::InvalidPath(
                                "Der Quelleneintrag wurde während des Kopierens ausgetauscht"
                                    .into(),
                            ),
                            job,
                        );
                    }
                    Ok(_) => {
                        copy_one_attempt(sv, child_origin, &sp, dv, &dp, resolver, job, report, 0)
                    }
                    Err(_) => {
                        copy_one_attempt(sv, child_origin, &sp, dv, &dp, resolver, job, report, 0)
                    }
                },
                (Err(e), _) | (_, Err(e)) => failed(report, src, e, job),
            }
            if job.checkpoint() {
                report.cancelled = true;
                return Ok(());
            }
        }
        // Only after the content: creating children changes the folder time, and a
        // read-only folder would block them. A merged, already existing destination
        // folder keeps its own time and mode.
        if created_dir {
            preserve_metadata(dv, &target, source_mode, source_modified, report);
        }
    } else if entry_kind == EntryKind::Symlink {
        let link = match origin_read_link(origin, sv, src) {
            Ok(v) => v,
            Err(e) => {
                return Err((src.clone(), e));
            }
        };
        if !sv.is_local() && !archive_link_is_safe(&link) {
            // Never create a link from an archive that could point out of the copied tree.
            report.skipped.push(src.clone());
            report.warnings.push((
                src.clone(),
                "Verknüpfung aus dem Archiv übersprungen: das Ziel ist nicht sicher relativ".into(),
            ));
            job.emit(ProgressEvent::ItemSkipped(src.clone()));
            return Ok(());
        }
        if replace {
            return Err((target.clone(), Error::Exists(target.path().to_path_buf())));
        }
        if let Err(e) = dv.create_symlink(&link, &target) {
            return Err((target.clone(), e));
        }
    } else {
        let mode = if replace {
            WriteMode::Replace
        } else {
            WriteMode::NoReplace
        };
        // A server target keeps a partial file that a later copy of the same unchanged source
        // continues; everything else (local, and replacing) is written the plain way.
        let resumed = if !replace && dv.remote_endpoint().is_some() {
            let identity = SourceIdentity::new(src, entry_size, source_modified);
            match dv.create_write_resumable(&target, identity) {
                Ok(resumable) => Some(resumable),
                Err(Error::Unsupported(_)) => None,
                Err(e) => {
                    return Err((target.clone(), e));
                }
            }
        } else {
            None
        };
        let (mut output, start): (Box<dyn WriteFinish>, u64) = match resumed {
            Some(resumable) => (resumable.writer, resumable.existing_len),
            None => match dv.create_write(&target, mode) {
                Ok(v) => (v, 0),
                Err(e) => {
                    return Err((target.clone(), e));
                }
            },
        };
        let hashing = job.verify == VerifyMode::Sha256;
        let mut digest = Sha256::new();
        let mut input =
            match origin_open_read_from(origin, sv, src, if hashing { 0 } else { start }) {
                Ok(v) => v,
                Err(e) => {
                    return Err((src.clone(), e));
                }
            };
        // A non-local source (a server or plugin) could advertise one size and then stream an
        // arbitrarily long body, filling the destination through the staging file before the
        // post-copy size check runs. Cap the reader at the advertised length plus one byte: the
        // staging file can never exceed the advertised size, and the `copied != entry_size` check
        // below still rejects both over- and under-delivery.
        if !sv.is_local() {
            let remaining = if hashing {
                entry_size
            } else {
                entry_size.saturating_sub(start)
            };
            input = Box::new(input.take(remaining.saturating_add(1)));
        }
        let mut buf = vec![0u8; 1024 * 1024];
        let mut copied = if hashing { 0 } else { start };
        loop {
            if job.checkpoint() {
                report.cancelled = true;
                return Ok(());
            }
            match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if hashing {
                        digest.update(&buf[..n]);
                    }
                    let skip = start.saturating_sub(copied).min(n as u64) as usize;
                    if let Err(e) = output.write_all(&buf[skip..n]) {
                        return Err((
                            target.clone(),
                            crate::error::from_io("Datei schreiben", target.path(), e),
                        ));
                    }
                    copied += n as u64;
                    job.emit(ProgressEvent::BytesCopied {
                        path: src.clone(),
                        bytes: copied,
                        total_bytes: if hashing {
                            entry_size.saturating_mul(2)
                        } else {
                            entry_size
                        },
                    });
                }
                Err(e) => {
                    return Err((
                        src.clone(),
                        crate::error::from_io("Datei lesen", src.path(), e),
                    ));
                }
            }
        }
        let verify = !(sv.is_local() && dv.is_local());
        // A source that is not a plain local file must deliver exactly the size it reported
        // (a move deletes the source afterwards); checked before the copy becomes visible.
        if !sv.is_local() && copied != entry_size {
            return Err((src.clone(), Error::SizeMismatch(src.path().to_path_buf())));
        }
        if let Err(e) = output.commit() {
            drop(output);
            if matches!(e, Error::Exists(_)) && retries < 5 {
                copy_one_attempt(
                    sv,
                    origin,
                    src,
                    dv,
                    dest,
                    resolver,
                    job,
                    report,
                    retries + 1,
                );
            } else {
                return Err((target.clone(), e));
            }
            return Ok(());
        }
        if verify && !hashing {
            match dv.stat(&target) {
                Ok(written) if written.size == copied => {}
                Ok(_) => {
                    return Err((
                        target.clone(),
                        Error::SizeMismatch(target.path().to_path_buf()),
                    ));
                }
                Err(e) => {
                    return Err((target.clone(), e));
                }
            }
        }
        if hashing {
            let result = (|| -> Result<()> {
                let mut reader = dv.open_read(&target)?;
                let mut actual = Sha256::new();
                let mut checked = 0;
                loop {
                    if job.checkpoint() {
                        return Err(Error::Cancelled);
                    }
                    let n = reader
                        .read(&mut buf)
                        .map_err(|e| crate::error::from_io("Prüfsumme lesen", target.path(), e))?;
                    if n == 0 {
                        break;
                    }
                    actual.update(&buf[..n]);
                    checked += n as u64;
                    job.emit(ProgressEvent::BytesCopied {
                        path: src.clone(),
                        bytes: copied.saturating_add(checked),
                        total_bytes: copied.saturating_mul(2),
                    });
                }
                if digest.finalize() != actual.finalize() {
                    return Err(Error::VerifyFailed);
                }
                Ok(())
            })();
            if let Err(e) = result {
                if matches!(e, Error::Cancelled) {
                    report.cancelled = true;
                } else {
                    return Err((target.clone(), e));
                }
                return Ok(());
            }
        }
        preserve_metadata(dv, &target, source_mode, source_modified, report);
    }
    report.replaced |= replace;
    if existing.is_none() {
        report.created.push((src.clone(), target));
    }
    report.done.push(src.clone());
    job.emit(ProgressEvent::ItemDone(src.clone()));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveStrategy {
    Auto,
    ForceCopy,
}
pub fn move_(
    src_vfs: &dyn Vfs,
    sources: &[VPath],
    dst_vfs: &dyn Vfs,
    dst_dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
) -> OpReport {
    move_with_strategy(
        src_vfs,
        sources,
        dst_vfs,
        dst_dir,
        resolver,
        job,
        MoveStrategy::Auto,
    )
}
pub fn move_with_strategy(
    sv: &dyn Vfs,
    sources: &[VPath],
    dv: &dyn Vfs,
    dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
    strategy: MoveStrategy,
) -> OpReport {
    if sv.is_read_only() || dv.is_read_only() {
        return reject_read_only(sources, job);
    }
    let mut report = OpReport::default();
    for src in sources {
        if job.checkpoint() {
            report.cancelled = true;
            break;
        }
        let ent = match retry_io(job, src, || sv.stat(src)) {
            Ok(e) => e,
            Err(e) => {
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
                continue;
            }
        };
        let dst = match join(dir, &ent.name) {
            Ok(v) => v,
            Err(e) => {
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
                continue;
            }
        };
        if strategy == MoveStrategy::Auto && std::ptr::eq(sv, dv) {
            match retry_io(job, src, || dv.rename(src, &dst, ReplaceMode::NoReplace)) {
                Ok(()) => {
                    report.created.push((src.clone(), dst));
                    report.done.push(src.clone());
                    continue;
                }
                Err(Error::Exists(_)) => {}
                Err(e) => {
                    failed(&mut report, src, e, job);
                    report.cancelled |= job.is_cancelled();
                    continue;
                }
            }
        }
        let copied = copy(sv, std::slice::from_ref(src), dv, dir, resolver, job);
        let complete_copy = copied.failed.is_empty();
        let skipped_copy = !copied.skipped.is_empty();
        report.replaced |= copied.replaced;
        let done_start = report.done.len();
        let retained_paths = copied
            .failed
            .iter()
            .map(|(path, _)| path.clone())
            .chain(copied.skipped.iter().cloned())
            .collect::<Vec<_>>();
        report.failed.extend(copied.failed);
        report.skipped.extend(copied.skipped);
        report.warnings.extend(copied.warnings);
        report.cancelled |= copied.cancelled;
        // A partial directory copy is safe to keep, but is not enough evidence to
        // remove any part of its source tree.
        if copied.cancelled
            || (job.verify == VerifyMode::Sha256 && (!complete_copy || skipped_copy))
        {
            continue;
        }
        // Skipped paths need the same ancestor protection as failed copies.
        if !complete_copy || skipped_copy {
            remove_successful_parts(sv, &copied.done, &retained_paths, job, &mut report);
        } else if !copied.done.is_empty() {
            let mut removed = Vec::new();
            if let Err(e) = remove_tree(sv, src, job, &mut report, &mut removed) {
                report.done.extend(removed);
                failed(&mut report, src, e, job);
                report.cancelled |= job.is_cancelled();
            } else {
                report.done.push(src.clone());
            }
        }
        report
            .created
            .extend(copied.created.into_iter().filter(|(source, _)| {
                report.done[done_start..]
                    .iter()
                    .any(|done| source.path().starts_with(done.path()))
            }));
    }
    report
}
fn remove_successful_parts(
    v: &dyn Vfs,
    done: &[VPath],
    bad_paths: &[VPath],
    job: &JobHandle,
    report: &mut OpReport,
) {
    for path in done {
        if job.checkpoint() {
            report.cancelled = true;
            return;
        }
        let kind = match v.stat(path) {
            Ok(entry) => entry.kind,
            Err(_) => continue,
        };
        if kind == EntryKind::Dir
            && bad_paths
                .iter()
                .any(|bad| bad.path().starts_with(path.path()))
        {
            continue;
        }
        if let Err(error) = retry_io(job, path, || v.remove_permanent(path)) {
            failed(report, path, error, job);
            report.cancelled |= job.is_cancelled();
        } else {
            report.done.push(path.clone());
        }
    }
}
fn remove_tree(
    v: &dyn Vfs,
    p: &VPath,
    j: &JobHandle,
    r: &mut OpReport,
    removed: &mut Vec<VPath>,
) -> Result<()> {
    if j.checkpoint() {
        r.cancelled = true;
        return Err(Error::Cancelled);
    }
    let e = retry_io(j, p, || v.stat(p))?;
    if e.kind == EntryKind::Dir {
        // A local directory is removed with the standard library's hardened remove_dir_all,
        // which walks the tree through openat/unlinkat and never follows a directory symlink
        // swapped in mid-walk (CVE-2022-21658). The manual path walk below follows such a swap
        // via list(), so it is kept only for remote backends, where that local race cannot
        // happen. The trade-off is no per-entry cancellation/progress during a local removal,
        // acceptable for the final source cleanup of a move.
        if p.scheme() == Scheme::Local {
            retry_io(j, p, || {
                std::fs::remove_dir_all(p.path())
                    .map_err(|e| crate::error::from_io("Ordner löschen", p.path(), e))
            })?;
            removed.push(p.clone());
            return Ok(());
        }
        for c in retry_io(j, p, || v.list(p))? {
            let cp = join(p, &c.name)?;
            remove_tree(v, &cp, j, r, removed)?;
        }
    }
    retry_io(j, p, || v.remove_permanent(p))?;
    removed.push(p.clone());
    Ok(())
}

pub fn trash_items(v: &dyn Vfs, items: &[VPath], job: &JobHandle) -> OpReport {
    trash_items_checked(v, items, job, &mut |_| None)
}

/// Revalidate each candidate after pause/queue boundaries, immediately before trash.
/// A reason skips the candidate; all successful items still form one undo operation.
pub fn trash_items_checked(
    v: &dyn Vfs,
    items: &[VPath],
    job: &JobHandle,
    check: &mut dyn FnMut(&VPath) -> Option<String>,
) -> OpReport {
    if v.is_read_only() {
        return reject_read_only(items, job);
    }
    let mut r = OpReport::default();
    for p in items {
        if job.checkpoint() {
            r.cancelled = true;
            break;
        }
        if let Some(reason) = check(p) {
            r.skipped.push(p.clone());
            r.warnings.push((p.clone(), reason));
            continue;
        }
        match v.trash(p) {
            Ok(()) => r.done.push(p.clone()),
            Err(e) => failed(&mut r, p, e, job),
        }
    }
    r
}
pub fn restore_from_trash(_items: &[VPath]) -> Result<OpReport> {
    if _items.iter().any(|path| path.scheme() != Scheme::Local) {
        return Err(Error::Unsupported(
            "Wiederherstellen aus dem Papierkorb (nur lokal)",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let requested: Vec<_> = _items
            .iter()
            .map(|path| path.path().to_path_buf())
            .collect();
        let selected = trash::os_limited::list()
            .map_err(|error| Error::Io {
                op: "Papierkorb auflisten",
                path: std::path::PathBuf::from("/"),
                source: std::io::Error::other(error),
            })?
            .into_iter()
            .filter(|item| requested.contains(&item.original_path()))
            .collect::<Vec<_>>();
        let mut report = OpReport::default();
        let restored = selected
            .iter()
            .map(|item| item.original_path())
            .collect::<Vec<_>>();
        trash::os_limited::restore_all(selected).map_err(|error| Error::Io {
            op: "Papierkorb-Einträge wiederherstellen",
            path: std::path::PathBuf::from("/"),
            source: std::io::Error::other(error),
        })?;
        report.done = restored
            .into_iter()
            .filter_map(|path| VPath::local(path).ok())
            .collect();
        Ok(report)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = _items;
        Err(Error::Unsupported(
            "Wiederherstellen aus dem Papierkorb (nur Linux)",
        ))
    }
}

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A fresh confirmation token (same scheme as the delete plan); used by sync plans.
pub(crate) fn new_token() -> u128 {
    ((std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos())
        << 32)
        ^ TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed) as u128
}

/// Deepest directory level a delete plan descends into (roots are level 0).
pub const MAX_DELETE_DEPTH: usize = 256;
/// Most entries a delete plan collects before it gives up.
pub const MAX_DELETE_ENTRIES: usize = 2_000_000;

/// What an entry looked like when it was planned. Compared again right before removal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileId {
    kind: EntryKind,
    /// Server entries have no inode: size and modification time stand in.
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
    /// Device and inode (lstat); only on Unix.
    #[cfg(unix)]
    inode: (u64, u64),
    /// Without inodes the size stands in (files only).
    #[cfg(not(unix))]
    size: u64,
}
fn file_id(path: &std::path::Path) -> std::io::Result<FileId> {
    let meta = std::fs::symlink_metadata(path)?;
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileId {
            kind,
            stamp: None,
            inode: (meta.dev(), meta.ino()),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(FileId {
            kind,
            stamp: None,
            size: if kind == EntryKind::File {
                meta.len()
            } else {
                0
            },
        })
    }
}

/// The identity of `path` on `v` right now: inode data for the local file system, kind plus
/// size and modification time for everything else (never a local lookup of a server path).
fn current_id(v: &dyn Vfs, path: &VPath, entry: Option<&Entry>) -> Option<FileId> {
    if v.is_local() {
        return file_id(path.path()).ok();
    }
    let fetched;
    let entry = match entry {
        Some(entry) => entry,
        None => {
            fetched = v.stat(path).ok()?;
            &fetched
        }
    };
    Some(FileId {
        kind: entry.kind,
        stamp: Some((entry.size, entry.modified)),
        #[cfg(unix)]
        inode: (0, 0),
        #[cfg(not(unix))]
        size: entry.size,
    })
}

/// True if `path` lies on the back end `v` serves (same connection for servers).
fn belongs_to(v: &dyn Vfs, path: &VPath) -> bool {
    match v.remote_endpoint() {
        Some((scheme, id)) => path.scheme() == scheme && path.conn_id() == Some(id),
        None => !path.scheme().is_remote(),
    }
}

#[derive(Clone, Debug)]
struct PlannedEntry {
    path: VPath,
    id: FileId,
    /// The folder the entry sits in, so a folder swapped for a link is noticed before
    /// anything below it is touched through that link.
    parent: Option<FileId>,
}

#[derive(Clone, Debug)]
pub struct DeletePlan {
    pub token: u128,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
    pub first_names: Vec<String>,
    /// The server connection the plan was made on (`None` for local plans). A plan only runs
    /// on the back end it was made for.
    endpoint: Option<(Scheme, String)>,
    /// True if the walk was cut short (limit, unreadable folder, cancel). Such a plan
    /// is never executed.
    pub incomplete: bool,
    entries: Vec<PlannedEntry>,
}
pub fn plan_delete(v: &dyn Vfs, items: &[VPath]) -> DeletePlan {
    plan_delete_cancellable(v, items, &JobHandle::silent())
}
pub fn plan_delete_cancellable(v: &dyn Vfs, items: &[VPath], job: &JobHandle) -> DeletePlan {
    plan_delete_limited(v, items, job, MAX_DELETE_DEPTH, MAX_DELETE_ENTRIES)
}
fn plan_delete_limited(
    v: &dyn Vfs,
    items: &[VPath],
    job: &JobHandle,
    max_depth: usize,
    max_entries: usize,
) -> DeletePlan {
    let mut p = DeletePlan {
        token: ((std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos())
            << 32)
            ^ TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed) as u128,
        files: 0,
        dirs: 0,
        links: 0,
        bytes: 0,
        first_names: Vec::new(),
        endpoint: v
            .remote_endpoint()
            .map(|(scheme, id)| (scheme, id.to_owned())),
        incomplete: false,
        entries: Vec::new(),
    };
    // Own stack instead of recursion. Parents are recorded before their children, so the
    // reversed list removes children first.
    let mut stack: Vec<(VPath, usize)> = items.iter().rev().map(|i| (i.clone(), 0)).collect();
    while let Some((path, depth)) = stack.pop() {
        if job.checkpoint() || p.entries.len() >= max_entries {
            p.incomplete = true;
            break;
        }
        let e = match v.stat(&path) {
            Ok(e) => e,
            Err(_) => {
                p.incomplete = true;
                continue;
            }
        };
        if !belongs_to(v, &path) {
            p.incomplete = true;
            continue;
        }
        let id = match current_id(v, &path, Some(&e)) {
            Some(id) if id.kind == e.kind => id,
            _ => {
                p.incomplete = true;
                continue;
            }
        };
        // Servers have no inode of the folder to compare; the parent check is local only.
        let parent = if v.is_local() {
            path.path().parent().and_then(|d| file_id(d).ok())
        } else {
            None
        };
        if p.first_names.len() < 5 {
            p.first_names.push(e.name.clone());
        }
        p.entries.push(PlannedEntry {
            path: path.clone(),
            id,
            parent,
        });
        match e.kind {
            EntryKind::File => {
                p.files += 1;
                p.bytes += e.size
            }
            EntryKind::Symlink => p.links += 1,
            EntryKind::Dir => {
                p.dirs += 1;
                if depth >= max_depth {
                    p.incomplete = true;
                    continue;
                }
                match v.list(&path) {
                    Ok(children) => {
                        for c in children.into_iter().rev() {
                            match join(&path, &c.name) {
                                Ok(child) => stack.push((child, depth + 1)),
                                Err(_) => p.incomplete = true,
                            }
                        }
                    }
                    Err(_) => p.incomplete = true,
                }
            }
        }
    }
    p
}
pub fn delete_permanent(
    v: &dyn Vfs,
    plan: &DeletePlan,
    confirmed_token: u128,
    job: &JobHandle,
) -> OpReport {
    let mut r = OpReport::default();
    if v.is_read_only() {
        let paths: Vec<VPath> = plan
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        return reject_read_only(&paths, job);
    }
    let first = || {
        plan.entries
            .first()
            .map(|x| x.path.clone())
            .unwrap_or_else(|| VPath::local("/").unwrap())
    };
    if plan.token != confirmed_token {
        r.failed.push((
            first(),
            Error::InvalidPath("Bestätigungstoken stimmt nicht mit dem Löschplan überein".into()),
        ));
        return r;
    }
    // The plan belongs to the connection it was made on; a foreign connection (or the local
    // file system) never executes it.
    let plan_endpoint = plan
        .endpoint
        .as_ref()
        .map(|(scheme, id)| (*scheme, id.as_str()));
    if plan_endpoint != v.remote_endpoint() || !plan.entries.iter().all(|e| belongs_to(v, &e.path))
    {
        r.failed.push((
            first(),
            Error::InvalidPath("Der Löschplan gehört zu einer anderen Verbindung".into()),
        ));
        return r;
    }
    if plan.incomplete {
        r.failed.push((first(), Error::IncompletePlan));
        return r;
    }
    for entry in plan.entries.iter().rev() {
        if job.checkpoint() {
            r.cancelled = true;
            break;
        }
        let p = &entry.path;
        // Only what was confirmed may go: same type, same inode, same parent folder.
        // Folders are removed with rmdir semantics, so content that appeared after the
        // plan makes the folder fail as "not empty" and stays; nothing is ever deleted
        // recursively here. Remaining window: between this lstat and the unlink another
        // process can still swap the entry (path-based Vfs API, no handle on the parent).
        let unchanged = current_id(v, p, None).is_some_and(|now| now == entry.id)
            && match entry.parent {
                Some(parent) => p
                    .path()
                    .parent()
                    .and_then(|d| file_id(d).ok())
                    .is_some_and(|now| now == parent),
                None => true,
            };
        if !unchanged {
            failed(
                &mut r,
                p,
                Error::ChangedSincePlan(p.path().to_path_buf()),
                job,
            );
            continue;
        }
        match v.remove_permanent(p) {
            Ok(()) => r.done.push(p.clone()),
            Err(e) => failed(&mut r, p, e, job),
        }
    }
    r
}
pub struct RenameResult {
    pub old: VPath,
    pub new: VPath,
}
pub fn rename_entry(v: &dyn Vfs, path: &VPath, new_name: &str) -> Result<RenameResult> {
    if v.is_read_only() {
        return Err(Error::Unsupported(READ_ONLY_MESSAGE));
    }
    validate_name(new_name)?;
    let new = path.sibling(new_name)?;
    v.rename(path, &new, ReplaceMode::NoReplace)?;
    Ok(RenameResult {
        old: path.clone(),
        new,
    })
}
pub fn make_dir(v: &dyn Vfs, parent: &VPath, name: &str) -> Result<VPath> {
    if v.is_read_only() {
        return Err(Error::Unsupported(READ_ONLY_MESSAGE));
    }
    validate_name(name)?;
    let path = parent.join(name)?;
    v.mkdir(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_tree_removal_unlinks_a_symlink_without_deleting_its_target() {
        // Removing a source tree must unlink a directory symlink inside it, never follow it and
        // delete the files it points at outside the tree (CVE-2022-21658 class, finding F3).
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), b"important").unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::os::unix::fs::symlink(&victim, src.join("link")).unwrap();
        std::fs::write(src.join("own.txt"), b"x").unwrap();
        let mut removed = Vec::new();
        let mut report = OpReport::default();
        remove_tree(
            &LocalVfs,
            &VPath::local(&src).unwrap(),
            &JobHandle::silent(),
            &mut report,
            &mut removed,
        )
        .unwrap();
        assert!(!src.exists(), "the source tree is gone");
        assert!(
            victim.join("keep.txt").exists(),
            "the symlink target must survive"
        );
        assert_eq!(
            std::fs::read(victim.join("keep.txt")).unwrap(),
            b"important"
        );
    }
    use crate::vfs::LocalVfs;
    use crate::vfs::{Caps, WriteFinish};
    use std::fs;
    use std::sync::Mutex;

    #[test]
    fn checked_trash_skips_before_vfs_call_and_records_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep");
        fs::write(&path, b"safe").unwrap();
        let item = VPath::local(&path).unwrap();
        let report = trash_items_checked(
            &LocalVfs,
            std::slice::from_ref(&item),
            &JobHandle::silent(),
            &mut |_| Some("changed".into()),
        );
        assert!(path.exists());
        assert!(report.done.is_empty());
        assert_eq!(report.skipped, vec![item]);
        assert_eq!(report.warnings[0].1, "changed");
    }

    #[test]
    fn checked_trash_cancellation_precedes_validation() {
        let job = JobHandle::silent();
        job.cancel();
        let item = VPath::local("/unused").unwrap();
        let report = trash_items_checked(&LocalVfs, &[item], &job, &mut |_| {
            panic!("cancelled job must not validate or trash")
        });
        assert!(report.cancelled);
    }

    struct Choose(Resolution);
    impl ConflictResolver for Choose {
        fn resolve(&self, _: &Conflict) -> Resolution {
            self.0
        }
    }

    struct ErrorChoices {
        choices: Mutex<std::collections::VecDeque<ErrorDecision>>,
        calls: AtomicU64,
    }
    impl ErrorResolver for ErrorChoices {
        fn resolve(&self, _: &VPath, _: &Error) -> ErrorDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.choices
                .lock()
                .unwrap()
                .pop_front()
                .expect("no silent retry")
        }
    }
    fn error_choices(choices: &[ErrorDecision]) -> Arc<ErrorChoices> {
        Arc::new(ErrorChoices {
            choices: Mutex::new(choices.iter().copied().collect()),
            calls: AtomicU64::new(0),
        })
    }

    #[test]
    fn error_skip_is_per_file_and_keeps_successful_siblings() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("folder")).unwrap();
        let bad = src.path().join("folder/a");
        fs::write(&bad, b"bad").unwrap();
        fs::write(src.path().join("folder/b"), b"good").unwrap();
        let errors = error_choices(&[ErrorDecision::Skip]);
        let job = JobHandle::silent().with_error_resolver(Some(errors.clone()));
        let report = copy(
            &DenyOneRead {
                denied: bad,
                list_error: false,
            },
            &[VPath::local(src.path().join("folder")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
        );
        assert_eq!(report.failed.len(), 1);
        assert!(!report.cancelled);
        assert_eq!(errors.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(dst.path().join("folder/b")).unwrap(), b"good");
        assert!(!dst.path().join("folder/a").exists());
    }

    #[test]
    fn error_abort_stops_the_job_and_cleans_partial_output() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let a = src.path().join("a");
        let b = src.path().join("b");
        fs::write(&a, b"bad").unwrap();
        fs::write(&b, b"good").unwrap();
        let errors = error_choices(&[ErrorDecision::Abort]);
        let job = JobHandle::silent().with_error_resolver(Some(errors));
        let report = copy(
            &DenyOneRead {
                denied: a.clone(),
                list_error: false,
            },
            &[VPath::local(a).unwrap(), VPath::local(b).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
        );
        assert!(report.cancelled);
        assert!(job.is_cancelled());
        assert_eq!(fs::read_dir(dst.path()).unwrap().count(), 0);
    }

    #[test]
    fn error_retry_always_reasks_even_beyond_three_attempts() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let a = src.path().join("a");
        fs::write(&a, b"bad").unwrap();
        let errors = error_choices(&[
            ErrorDecision::Retry,
            ErrorDecision::Retry,
            ErrorDecision::Retry,
            ErrorDecision::Retry,
            ErrorDecision::Skip,
        ]);
        let job = JobHandle::silent().with_error_resolver(Some(errors.clone()));
        let report = copy(
            &DenyOneRead {
                denied: a.clone(),
                list_error: false,
            },
            &[VPath::local(a).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
        );
        assert_eq!(errors.calls.load(Ordering::SeqCst), 5);
        assert_eq!(report.failed.len(), 1);
        assert!(report.done.is_empty());
        assert_eq!(fs::read_dir(dst.path()).unwrap().count(), 0);
    }

    #[test]
    fn error_abort_on_last_move_rename_is_reported_as_cancelled() {
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("a");
        fs::write(&file, b"keep source").unwrap();
        let errors = error_choices(&[ErrorDecision::Abort]);
        let job = JobHandle::silent().with_error_resolver(Some(errors.clone()));
        let vfs = LocalVfs;
        let report = move_(
            &vfs,
            &[VPath::local(&file).unwrap()],
            &vfs,
            &VPath::local(src.path().join("missing")).unwrap(),
            None,
            &job,
        );
        assert!(report.cancelled);
        assert_eq!(errors.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(file).unwrap(), b"keep source");
    }

    #[test]
    fn error_retry_can_recover_a_missing_source() {
        struct Repair;
        impl ErrorResolver for Repair {
            fn resolve(&self, path: &VPath, _: &Error) -> ErrorDecision {
                fs::write(path.path(), b"recovered").unwrap();
                ErrorDecision::Retry
            }
        }
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let job = JobHandle::silent().with_error_resolver(Some(Arc::new(Repair)));
        let report = copy_verified(
            &LocalVfs,
            &[VPath::local(src.path().join("a")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
            VerifyMode::Sha256,
        );
        assert!(report.failed.is_empty());
        assert!(report.verified);
        assert_eq!(fs::read(dst.path().join("a")).unwrap(), b"recovered");
    }

    #[test]
    fn no_error_resolver_preserves_continue_on_failure() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("b"), b"good").unwrap();
        let report = copy(
            &LocalVfs,
            &[
                VPath::local(src.path().join("missing")).unwrap(),
                VPath::local(src.path().join("b")).unwrap(),
            ],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::silent().with_error_resolver(None),
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.done.len(), 1);
        assert!(!report.verified);
    }

    #[test]
    fn listing_item_errors_offer_skip_retry_and_abort() {
        for decision in [
            ErrorDecision::Skip,
            ErrorDecision::Retry,
            ErrorDecision::Abort,
        ] {
            let src = tempfile::tempdir().unwrap();
            let dst = tempfile::tempdir().unwrap();
            let folder = src.path().join("folder");
            fs::create_dir(&folder).unwrap();
            let bad = folder.join("a");
            fs::write(&bad, b"bad").unwrap();
            fs::write(folder.join("b"), b"good").unwrap();
            let errors = error_choices(&[decision, ErrorDecision::Skip]);
            let job = JobHandle::silent().with_error_resolver(Some(errors.clone()));
            let report = copy(
                &DenyOneRead {
                    denied: bad,
                    list_error: true,
                },
                &[VPath::local(folder).unwrap()],
                &LocalVfs,
                &VPath::local(dst.path()).unwrap(),
                None,
                &job,
            );
            assert_eq!(report.cancelled, decision == ErrorDecision::Abort);
            assert_eq!(
                errors.calls.load(Ordering::SeqCst),
                if decision == ErrorDecision::Retry {
                    2
                } else {
                    1
                }
            );
            assert_eq!(
                dst.path().join("folder/b").exists(),
                decision != ErrorDecision::Abort
            );
            assert!(!dst.path().join("folder/a").exists());
        }
    }

    struct DenyOneRead {
        denied: std::path::PathBuf,
        list_error: bool,
    }
    impl Vfs for DenyOneRead {
        fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
            let mut entries = LocalVfs.list(p)?;
            if self.list_error {
                for entry in &mut entries {
                    if p.path().join(&entry.name) == self.denied {
                        entry.error = Some("Zugriff verweigert".into());
                    }
                }
            }
            Ok(entries)
        }
        fn stat(&self, p: &VPath) -> Result<Entry> {
            LocalVfs.stat(p)
        }
        fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
            if p.path() == self.denied {
                Err(Error::PermissionDenied(p.path().to_path_buf()))
            } else {
                LocalVfs.open_read(p)
            }
        }
        fn create_write(&self, p: &VPath, m: WriteMode) -> Result<Box<dyn WriteFinish>> {
            LocalVfs.create_write(p, m)
        }
        fn mkdir(&self, p: &VPath) -> Result<()> {
            LocalVfs.mkdir(p)
        }
        fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
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

    #[test]
    fn forced_copy_move_keeps_failed_part_and_moves_successful_sibling() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let folder = src.path().join("bundle");
        fs::create_dir(&folder).unwrap();
        let bad = folder.join("a-denied");
        let good = folder.join("b-good");
        fs::write(&bad, b"secret").unwrap();
        fs::write(&good, b"move me").unwrap();
        let vfs = DenyOneRead {
            denied: bad.clone(),
            list_error: false,
        };
        let report = move_with_strategy(
            &vfs,
            &[VPath::local(&folder).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::silent(),
            MoveStrategy::ForceCopy,
        );
        assert!(!report.failed.is_empty());
        assert!(bad.exists());
        assert!(!good.exists());
        assert_eq!(
            fs::read(dst.path().join("bundle/b-good")).unwrap(),
            b"move me"
        );
    }

    #[derive(Default)]
    struct Record(Mutex<Vec<ProgressEvent>>);
    impl ProgressSink for Record {
        fn event(&self, event: ProgressEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    struct CancelOnBytes(Mutex<Option<JobHandle>>);
    impl ProgressSink for CancelOnBytes {
        fn event(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::BytesCopied { .. }) {
                if let Some(job) = self.0.lock().unwrap().as_ref() {
                    job.cancel();
                }
            }
        }
    }

    struct RaceCreate {
        target: std::path::PathBuf,
        fired: std::sync::atomic::AtomicBool,
    }
    impl ProgressSink for RaceCreate {
        fn event(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::BytesCopied { .. })
                && !self.fired.swap(true, Ordering::SeqCst)
            {
                fs::write(&self.target, b"racer").unwrap();
            }
        }
    }

    struct SwapDirectory {
        source_dir: std::path::PathBuf,
        moved_dir: std::path::PathBuf,
        outside: std::path::PathBuf,
        fired: std::sync::atomic::AtomicBool,
    }
    impl ProgressSink for SwapDirectory {
        fn event(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::BytesCopied { .. })
                && !self.fired.swap(true, Ordering::SeqCst)
            {
                fs::rename(&self.source_dir, &self.moved_dir).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(&self.outside, &self.source_dir).unwrap();
            }
        }
    }

    #[test]
    fn resolver_is_reasked_when_destination_appears_before_commit() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let target = dst.path().join("data");
        fs::write(src.path().join("data"), b"payload").unwrap();
        let sink = Arc::new(RaceCreate {
            target: target.clone(),
            fired: std::sync::atomic::AtomicBool::new(false),
        });
        let resolver = Choose(Resolution::Skip);
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path().join("data")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            Some(&resolver),
            &JobHandle::new(sink),
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read(target).unwrap(), b"racer");
    }

    #[test]
    fn unreadable_file_is_collected_while_other_files_copy() {
        #[cfg(unix)]
        {
            if rustix::process::geteuid().is_root() {
                return;
            }
            use std::os::unix::fs::PermissionsExt;
            let src = tempfile::tempdir().unwrap();
            let dst = tempfile::tempdir().unwrap();
            let blocked = src.path().join("blocked");
            fs::write(&blocked, b"private").unwrap();
            fs::set_permissions(&blocked, fs::Permissions::from_mode(0o0)).unwrap();
            fs::write(src.path().join("readable"), b"ok").unwrap();
            let report = copy(
                &LocalVfs,
                &[
                    VPath::local(blocked).unwrap(),
                    VPath::local(src.path().join("readable")).unwrap(),
                ],
                &LocalVfs,
                &VPath::local(dst.path()).unwrap(),
                None,
                &JobHandle::silent(),
            );
            assert!(!report.failed.is_empty());
            assert_eq!(fs::read(dst.path().join("readable")).unwrap(), b"ok");
        }
    }

    #[cfg(unix)]
    #[test]
    fn directory_replaced_by_external_link_is_not_traversed() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("branch")).unwrap();
        fs::write(
            src.path().join("branch/trigger"),
            vec![5u8; 2 * 1024 * 1024],
        )
        .unwrap();
        fs::create_dir(src.path().join("branch/victim")).unwrap();
        fs::write(src.path().join("branch/victim/original"), b"inside").unwrap();
        fs::write(outside.path().join("secret"), b"outside").unwrap();
        let sink = Arc::new(SwapDirectory {
            source_dir: src.path().join("branch/victim"),
            moved_dir: src.path().join("branch/saved-victim"),
            outside: outside.path().to_path_buf(),
            fired: std::sync::atomic::AtomicBool::new(false),
        });
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path()).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::new(sink),
        );
        assert!(!report.failed.is_empty());
        assert!(!dst.path().join("secret").exists());
    }

    #[cfg(target_os = "linux")]
    struct XdgGuard {
        previous_xdg: Option<std::ffi::OsString>,
        previous_home: Option<std::ffi::OsString>,
    }
    #[cfg(target_os = "linux")]
    impl XdgGuard {
        fn set(data_home: &std::path::Path, home: &std::path::Path) -> Self {
            let guard = Self {
                previous_xdg: std::env::var_os("XDG_DATA_HOME"),
                previous_home: std::env::var_os("HOME"),
            };
            std::env::set_var("XDG_DATA_HOME", data_home);
            std::env::set_var("HOME", home);
            guard
        }
    }
    #[cfg(target_os = "linux")]
    impl Drop for XdgGuard {
        fn drop(&mut self) {
            for (key, value) in [
                ("XDG_DATA_HOME", self.previous_xdg.take()),
                ("HOME", self.previous_home.take()),
            ] {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn restore_from_trash_restores_content_and_never_overwrites() {
        let _lock = crate::vfs::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data-home");
        let home = dir.path().join("home");
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&home).unwrap();
        let _env = XdgGuard::set(&data_home, &home);

        let first = dir.path().join("restore-me.txt");
        let second = dir.path().join("collide.txt");
        fs::write(&first, b"first content").unwrap();
        fs::write(&second, b"trashed content").unwrap();
        let first_path = VPath::local(&first).unwrap();
        let second_path = VPath::local(&second).unwrap();
        let trashed = trash_items(
            &LocalVfs,
            &[first_path.clone(), second_path.clone()],
            &JobHandle::silent(),
        );
        assert!(trashed.failed.is_empty(), "{:?}", trashed.failed);
        assert!(!first.exists() && !second.exists());

        let report = restore_from_trash(std::slice::from_ref(&first_path)).unwrap();
        assert_eq!(report.done, vec![first_path]);
        assert_eq!(fs::read(&first).unwrap(), b"first content");

        // A new file at the original location must survive the restore attempt.
        fs::write(&second, b"newer content").unwrap();
        let result = restore_from_trash(std::slice::from_ref(&second_path));
        assert!(result.is_err());
        assert_eq!(fs::read(&second).unwrap(), b"newer content");
        // The trashed copy is still available, nothing was lost.
        assert_eq!(
            fs::read_dir(data_home.join("Trash/files")).unwrap().count(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ancestor_swapped_after_open_is_still_read_through_handle() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let branch = src.path().join("branch");
        fs::create_dir(&branch).unwrap();
        fs::write(branch.join("a-trigger"), vec![1u8; 2 * 1024 * 1024]).unwrap();
        fs::write(branch.join("b-file"), b"inside").unwrap();
        fs::create_dir(branch.join("c-dir")).unwrap();
        fs::write(branch.join("c-dir/deep"), b"inside-deep").unwrap();
        fs::write(outside.path().join("b-file"), b"outside").unwrap();
        fs::create_dir(outside.path().join("c-dir")).unwrap();
        fs::write(outside.path().join("c-dir/deep"), b"outside-deep").unwrap();
        // Fires inside the first file of `branch`, i.e. after `branch` was opened.
        let sink = Arc::new(SwapDirectory {
            source_dir: branch.clone(),
            moved_dir: src.path().join("saved-branch"),
            outside: outside.path().to_path_buf(),
            fired: std::sync::atomic::AtomicBool::new(false),
        });
        let report = copy(
            &LocalVfs,
            &[VPath::local(&branch).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::new(sink),
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(fs::symlink_metadata(&branch)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(dst.path().join("branch/b-file")).unwrap(),
            b"inside"
        );
        assert_eq!(
            fs::read(dst.path().join("branch/c-dir/deep")).unwrap(),
            b"inside-deep"
        );
    }

    struct SwapFile {
        file: std::path::PathBuf,
        outside_file: std::path::PathBuf,
        fired: std::sync::atomic::AtomicBool,
    }
    impl ProgressSink for SwapFile {
        fn event(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::BytesCopied { .. })
                && !self.fired.swap(true, Ordering::SeqCst)
            {
                fs::remove_file(&self.file).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(&self.outside_file, &self.file).unwrap();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_replaced_by_external_link_after_open_is_not_copied() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = src.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a-trigger"), vec![1u8; 2 * 1024 * 1024]).unwrap();
        fs::write(folder.join("b-file"), b"inside").unwrap();
        fs::write(outside.path().join("secret"), b"outside").unwrap();
        let sink = Arc::new(SwapFile {
            file: folder.join("b-file"),
            outside_file: outside.path().join("secret"),
            fired: std::sync::atomic::AtomicBool::new(false),
        });
        let report = copy(
            &LocalVfs,
            &[VPath::local(&folder).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::new(sink),
        );
        assert!(!report.failed.is_empty());
        let copied = dst.path().join("folder/b-file");
        assert!(fs::read(&copied).map_or(true, |content| content != b"outside"));
    }

    #[cfg(unix)]
    #[test]
    fn copied_relative_link_target_is_unchanged() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("tree")).unwrap();
        std::os::unix::fs::symlink("../dualfile-no-such-target", src.path().join("tree/link"))
            .unwrap();
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path().join("tree/link")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        assert_eq!(
            fs::read_link(dst.path().join("link")).unwrap(),
            std::path::PathBuf::from("../dualfile-no-such-target")
        );
    }

    #[test]
    fn missing_resolver_never_overwrites_existing_file() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("f"), b"source").unwrap();
        fs::write(dst.path().join("f"), b"keep").unwrap();
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path().join("f")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read(dst.path().join("f")).unwrap(), b"keep");
    }

    #[test]
    fn invalid_delete_token_does_not_remove_any_item() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        fs::write(&path, b"stay").unwrap();
        let vpath = VPath::local(&path).unwrap();
        let plan = plan_delete(&LocalVfs, std::slice::from_ref(&vpath));
        let report = delete_permanent(
            &LocalVfs,
            &plan,
            plan.token.wrapping_add(1),
            &JobHandle::silent(),
        );
        assert_eq!(report.done.len(), 0);
        assert!(!report.failed.is_empty());
        assert_eq!(fs::read(path).unwrap(), b"stay");
    }

    #[test]
    fn cancelling_during_copy_leaves_no_part_file() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("big"), vec![1u8; 4 * 1024 * 1024]).unwrap();
        let sink = Arc::new(CancelOnBytes(Mutex::new(None)));
        let job = JobHandle::new(sink.clone());
        *sink.0.lock().unwrap() = Some(job.clone());
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path().join("big")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
        );
        assert!(report.cancelled);
        assert!(!dst.path().join("big").exists());
        assert!(fs::read_dir(dst.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("dualfile-part")));
    }

    #[test]
    fn delete_stops_after_cancellation_and_collects_missing_entry_error() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a");
        let second = dir.path().join("b");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        let a = VPath::local(&first).unwrap();
        let b = VPath::local(&second).unwrap();
        let plan = plan_delete(&LocalVfs, &[a.clone(), b.clone()]);
        let job = JobHandle::silent();
        job.cancel();
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &job);
        assert!(report.cancelled);
        assert!(first.exists() && second.exists());
        fs::remove_file(&first).unwrap();
        let partial = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(!partial.failed.is_empty());
        assert!(!second.exists());
    }

    #[test]
    fn fifty_megabyte_copy_emits_monotonic_block_progress() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let content = vec![7u8; 50 * 1024 * 1024];
        fs::write(src.path().join("large"), &content).unwrap();
        let record = Arc::new(Record::default());
        let job = JobHandle::new(record.clone());
        let report = copy(
            &LocalVfs,
            &[VPath::local(src.path().join("large")).unwrap()],
            &LocalVfs,
            &VPath::local(dst.path()).unwrap(),
            None,
            &job,
        );
        assert!(report.failed.is_empty());
        assert_eq!(
            fs::metadata(dst.path().join("large")).unwrap().len(),
            content.len() as u64
        );
        let totals: Vec<_> = record
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::BytesCopied { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .collect();
        assert!(totals.len() >= 50);
        assert!(totals.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn copy_rejects_destination_inside_source() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("source")).unwrap();
        fs::create_dir(root.path().join("source/inside")).unwrap();
        let src = VPath::local(root.path().join("source")).unwrap();
        let dst = VPath::local(root.path().join("source/inside")).unwrap();
        let result = copy(
            &LocalVfs,
            &[src],
            &LocalVfs,
            &dst,
            None,
            &JobHandle::silent(),
        );
        assert!(matches!(result.failed[0].1, Error::DestinationInsideSource));
    }

    #[test]
    fn duplicate_creates_free_copy_names_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        fs::write(dir.join("foto.jpg"), "orig").unwrap();
        fs::write(dir.join("foto_copy.jpg"), "vorhanden").unwrap();
        fs::write(dir.join("a.tar.gz"), "tar").unwrap();
        fs::create_dir_all(dir.join("ordner/sub")).unwrap();
        fs::write(dir.join("ordner/sub/x.txt"), "x").unwrap();
        let sources: Vec<VPath> = ["foto.jpg", "a.tar.gz", "ordner"]
            .iter()
            .map(|n| VPath::local(dir.join(n)).unwrap())
            .collect();
        let report = duplicate(&LocalVfs, &sources, &JobHandle::silent());
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(
            fs::read_to_string(dir.join("foto_copy.jpg")).unwrap(),
            "vorhanden"
        );
        assert_eq!(
            fs::read_to_string(dir.join("foto_copy2.jpg")).unwrap(),
            "orig"
        );
        assert_eq!(
            fs::read_to_string(dir.join("a_copy.tar.gz")).unwrap(),
            "tar"
        );
        assert_eq!(
            fs::read_to_string(dir.join("ordner_copy/sub/x.txt")).unwrap(),
            "x"
        );
        // Created covers the copied items (folder contents included), roots are among them.
        for name in ["foto_copy2.jpg", "a_copy.tar.gz", "ordner_copy"] {
            assert!(
                report
                    .created
                    .iter()
                    .any(|(_, t)| t.path() == dir.join(name)),
                "{name}"
            );
        }
        // A second round counts further and leaves the first copies alone.
        let again = duplicate(&LocalVfs, &sources, &JobHandle::silent());
        assert!(again.failed.is_empty());
        assert!(dir.join("foto_copy3.jpg").exists());
        assert!(dir.join("a_copy2.tar.gz").exists());
        assert!(dir.join("ordner_copy2/sub/x.txt").exists());
        assert_eq!(fs::read_to_string(dir.join("foto.jpg")).unwrap(), "orig");
    }

    #[test]
    fn duplicate_stops_with_the_error_when_the_name_probe_fails() {
        // Source stat works, every later probe fails (connection lost): no endless counting.
        struct LostAfterSource {
            calls: std::sync::atomic::AtomicUsize,
        }
        impl Vfs for LostAfterSource {
            fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
                LocalVfs.list(p)
            }
            fn stat(&self, p: &VPath) -> Result<Entry> {
                if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    LocalVfs.stat(p)
                } else {
                    Err(Error::Disconnected)
                }
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
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "x").unwrap();
        let vfs = LostAfterSource { calls: 0.into() };
        let src = VPath::local(root.path().join("a.txt")).unwrap();
        let report = duplicate(&vfs, &[src], &JobHandle::silent());
        assert_eq!(report.failed.len(), 1);
        assert!(matches!(report.failed[0].1, Error::Disconnected));
        // One source stat plus exactly one failing probe, not a counting loop.
        assert_eq!(vfs.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(!root.path().join("a_copy.txt").exists());
    }
    #[test]
    fn duplicate_of_a_missing_source_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let src = VPath::local(root.path().join("weg.txt")).unwrap();
        let report = duplicate(&LocalVfs, &[src], &JobHandle::silent());
        assert_eq!(report.failed.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn copy_detects_same_directory_through_symlink() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("real")).unwrap();
        std::os::unix::fs::symlink("real", root.path().join("alias")).unwrap();
        let src = VPath::local(root.path().join("real")).unwrap();
        let dst = VPath::local(root.path().join("alias")).unwrap();
        let report = copy(
            &LocalVfs,
            &[src],
            &LocalVfs,
            &dst,
            None,
            &JobHandle::silent(),
        );
        assert!(matches!(report.failed[0].1, Error::SameFile));
    }

    #[cfg(unix)]
    #[test]
    fn copy_keeps_parent_cycle_as_link() {
        let src_root = tempfile::tempdir().unwrap();
        let dst_root = tempfile::tempdir().unwrap();
        fs::create_dir(src_root.path().join("tree")).unwrap();
        std::os::unix::fs::symlink("..", src_root.path().join("tree/up")).unwrap();
        let report = copy(
            &LocalVfs,
            &[VPath::local(src_root.path().join("tree")).unwrap()],
            &LocalVfs,
            &VPath::local(dst_root.path()).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        assert_eq!(
            fs::read_link(dst_root.path().join("tree/up")).unwrap(),
            std::path::PathBuf::from("..")
        );
    }

    #[test]
    fn conflicts_handle_all_resolutions() {
        for (resolution, expected) in [
            (Resolution::Skip, "old"),
            (Resolution::KeepBoth, "new"),
            (Resolution::SkipAll, "old"),
            (Resolution::KeepBothAll, "new"),
        ] {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::write(a.path().join("f"), b"new").unwrap();
            fs::write(b.path().join("f"), b"old").unwrap();
            let pick = Choose(resolution);
            let result = copy(
                &LocalVfs,
                &[VPath::local(a.path().join("f")).unwrap()],
                &LocalVfs,
                &VPath::local(b.path()).unwrap(),
                Some(&pick),
                &JobHandle::silent(),
            );
            assert!(result.failed.is_empty());
            let actual = if expected == "new" {
                fs::read(b.path().join("f (2)")).unwrap()
            } else {
                fs::read(b.path().join("f")).unwrap()
            };
            assert_eq!(actual, expected.as_bytes());
        }
        for resolution in [
            Resolution::Cancel,
            Resolution::Overwrite,
            Resolution::OverwriteAll,
        ] {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::write(a.path().join("f"), b"new").unwrap();
            fs::write(b.path().join("f"), b"old").unwrap();
            let pick = Choose(resolution);
            let result = copy(
                &LocalVfs,
                &[VPath::local(a.path().join("f")).unwrap()],
                &LocalVfs,
                &VPath::local(b.path()).unwrap(),
                Some(&pick),
                &JobHandle::silent(),
            );
            if resolution == Resolution::Cancel {
                assert!(result.cancelled);
                assert_eq!(fs::read(b.path().join("f")).unwrap(), b"old");
            } else {
                assert_eq!(fs::read(b.path().join("f")).unwrap(), b"new");
            }
        }
    }

    #[test]
    fn directory_conflict_merges_children() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::create_dir(a.path().join("folder")).unwrap();
        fs::create_dir(b.path().join("folder")).unwrap();
        fs::write(a.path().join("folder/a"), b"a").unwrap();
        fs::write(b.path().join("folder/b"), b"b").unwrap();
        let report = copy(
            &LocalVfs,
            &[VPath::local(a.path().join("folder")).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        assert_eq!(fs::read(b.path().join("folder/a")).unwrap(), b"a");
        assert_eq!(fs::read(b.path().join("folder/b")).unwrap(), b"b");
    }

    #[test]
    fn forced_copy_move_preserves_source_on_conflict_skip() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("f"), b"source").unwrap();
        fs::write(b.path().join("f"), b"destination").unwrap();
        let choose = Choose(Resolution::Skip);
        let report = move_with_strategy(
            &LocalVfs,
            &[VPath::local(a.path().join("f")).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            Some(&choose),
            &JobHandle::silent(),
            MoveStrategy::ForceCopy,
        );
        assert!(a.path().join("f").exists());
        assert_eq!(fs::read(b.path().join("f")).unwrap(), b"destination");
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn directory_move_keeps_skipped_child_and_moves_successful_sibling() {
        for strategy in [MoveStrategy::Auto, MoveStrategy::ForceCopy] {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::create_dir_all(a.path().join("folder/nested")).unwrap();
            fs::create_dir_all(b.path().join("folder/nested")).unwrap();
            fs::write(a.path().join("folder/nested/skip"), b"source").unwrap();
            fs::write(b.path().join("folder/nested/skip"), b"destination").unwrap();
            fs::write(a.path().join("folder/move"), b"copied").unwrap();
            let skipped = VPath::local(a.path().join("folder/nested/skip")).unwrap();
            let job = JobHandle::silent();
            assert_ne!(job.verify, VerifyMode::Sha256);
            let report = move_with_strategy(
                &LocalVfs,
                &[VPath::local(a.path().join("folder")).unwrap()],
                &LocalVfs,
                &VPath::local(b.path()).unwrap(),
                Some(&Choose(Resolution::Skip)),
                &job,
                strategy,
            );
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            assert_eq!(report.skipped, vec![skipped]);
            assert_eq!(
                fs::read(a.path().join("folder/nested/skip")).unwrap(),
                b"source"
            );
            assert_eq!(
                fs::read(b.path().join("folder/nested/skip")).unwrap(),
                b"destination"
            );
            assert_eq!(fs::read(b.path().join("folder/move")).unwrap(), b"copied");
            assert!(!a.path().join("folder/move").exists());
        }
    }

    #[test]
    fn directory_move_cancel_keeps_entire_source_tree() {
        for strategy in [MoveStrategy::Auto, MoveStrategy::ForceCopy] {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::create_dir_all(a.path().join("folder/nested")).unwrap();
            fs::create_dir_all(b.path().join("folder/nested")).unwrap();
            fs::write(a.path().join("folder/nested/clash"), b"source").unwrap();
            fs::write(b.path().join("folder/nested/clash"), b"destination").unwrap();
            fs::write(a.path().join("folder/other"), b"other").unwrap();
            let job = JobHandle::silent();
            assert_ne!(job.verify, VerifyMode::Sha256);
            let report = move_with_strategy(
                &LocalVfs,
                &[VPath::local(a.path().join("folder")).unwrap()],
                &LocalVfs,
                &VPath::local(b.path()).unwrap(),
                Some(&Choose(Resolution::Cancel)),
                &job,
                strategy,
            );
            assert!(report.cancelled);
            assert_eq!(
                fs::read(a.path().join("folder/nested/clash")).unwrap(),
                b"source"
            );
            assert_eq!(fs::read(a.path().join("folder/other")).unwrap(), b"other");
            assert_eq!(
                fs::read(b.path().join("folder/nested/clash")).unwrap(),
                b"destination"
            );
        }
    }

    #[test]
    fn same_device_move_keeps_inode() {
        use std::os::unix::fs::MetadataExt;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let source = a.path().join("item");
        fs::write(&source, b"same inode").unwrap();
        let before = fs::metadata(&source).unwrap().ino();
        let report = move_(
            &LocalVfs,
            &[VPath::local(&source).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        assert_eq!(fs::metadata(b.path().join("item")).unwrap().ino(), before);
        assert!(!source.exists());
    }

    #[test]
    fn forced_copy_move_copies_then_removes_source() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let source = a.path().join("item");
        fs::write(&source, b"copied").unwrap();
        let report = move_with_strategy(
            &LocalVfs,
            &[VPath::local(&source).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            None,
            &JobHandle::silent(),
            MoveStrategy::ForceCopy,
        );
        assert!(report.failed.is_empty());
        assert!(!source.exists());
        assert_eq!(fs::read(b.path().join("item")).unwrap(), b"copied");
    }

    #[test]
    fn file_directory_conflict_never_overwrites() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("same"), b"file").unwrap();
        fs::create_dir(b.path().join("same")).unwrap();
        let overwrite = Choose(Resolution::Overwrite);
        let report = copy(
            &LocalVfs,
            &[VPath::local(a.path().join("same")).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            Some(&overwrite),
            &JobHandle::silent(),
        );
        assert!(!report.failed.is_empty());
        assert!(b.path().join("same").is_dir());
    }

    #[test]
    fn move_conflict_skip_keeps_both_sources_and_destination() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("f"), b"source").unwrap();
        fs::write(b.path().join("f"), b"dest").unwrap();
        let pick = Choose(Resolution::Skip);
        let report = move_(
            &LocalVfs,
            &[VPath::local(a.path().join("f")).unwrap()],
            &LocalVfs,
            &VPath::local(b.path()).unwrap(),
            Some(&pick),
            &JobHandle::silent(),
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read(a.path().join("f")).unwrap(), b"source");
        assert_eq!(fs::read(b.path().join("f")).unwrap(), b"dest");
    }

    #[test]
    fn delete_plan_counts_link_without_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("safe"), b"1234").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let link = VPath::local(dir.path().join("link")).unwrap();
        let plan = plan_delete(&LocalVfs, std::slice::from_ref(&link));
        assert_eq!(plan.links, 1);
        assert_eq!(plan.dirs, 0);
        assert_eq!(plan.files, 0);
        let removed = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(removed.failed.is_empty());
        assert!(outside.path().join("safe").exists());
    }

    #[test]
    fn trash_unsupported_is_reported_without_deleting_source() {
        struct NoTrash(LocalVfs);
        impl Vfs for NoTrash {
            fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
                self.0.list(p)
            }
            fn stat(&self, p: &VPath) -> Result<Entry> {
                self.0.stat(p)
            }
            fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
                self.0.open_read(p)
            }
            fn create_write(
                &self,
                p: &VPath,
                m: WriteMode,
            ) -> Result<Box<dyn crate::vfs::WriteFinish>> {
                self.0.create_write(p, m)
            }
            fn mkdir(&self, p: &VPath) -> Result<()> {
                self.0.mkdir(p)
            }
            fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
                self.0.rename(a, b, m)
            }
            fn read_link(&self, p: &VPath) -> Result<String> {
                self.0.read_link(p)
            }
            fn create_symlink(&self, t: &str, p: &VPath) -> Result<()> {
                self.0.create_symlink(t, p)
            }
            fn trash(&self, _: &VPath) -> Result<()> {
                Err(Error::Unsupported("Papierkorb"))
            }
            fn remove_permanent(&self, p: &VPath) -> Result<()> {
                self.0.remove_permanent(p)
            }
            fn capabilities(&self) -> crate::vfs::Caps {
                self.0.capabilities()
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stay");
        fs::write(&path, b"safe").unwrap();
        let item = VPath::local(&path).unwrap();
        let report = trash_items(&NoTrash(LocalVfs), &[item], &JobHandle::silent());
        assert!(matches!(report.failed[0].1, Error::Unsupported(_)));
        assert!(path.exists());
    }

    #[test]
    fn recursive_copy_keeps_empty_directories_and_links() {
        let src_root = tempfile::tempdir().unwrap();
        let dst_root = tempfile::tempdir().unwrap();
        fs::create_dir(src_root.path().join("tree")).unwrap();
        fs::create_dir(src_root.path().join("tree/empty")).unwrap();
        fs::write(src_root.path().join("tree/file"), b"payload").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../file", src_root.path().join("tree/link")).unwrap();
        let src = VPath::local(src_root.path().join("tree")).unwrap();
        let dst = VPath::local(dst_root.path()).unwrap();
        let report = copy(
            &LocalVfs,
            &[src],
            &LocalVfs,
            &dst,
            None,
            &JobHandle::silent(),
        );
        assert!(
            report.failed.is_empty(),
            "{:?}",
            report
                .failed
                .iter()
                .map(|e| e.1.to_string())
                .collect::<Vec<_>>()
        );
        assert!(dst_root.path().join("tree/empty").is_dir());
        assert_eq!(
            fs::read(dst_root.path().join("tree/file")).unwrap(),
            b"payload"
        );
        #[cfg(unix)]
        {
            assert_eq!(
                fs::read_link(dst_root.path().join("tree/link"))
                    .unwrap()
                    .to_string_lossy(),
                "../file"
            );
        }
    }

    #[test]
    fn conflicts_default_to_skip_and_keep_both_is_distinct() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("x"), b"new").unwrap();
        fs::write(b.path().join("x"), b"old").unwrap();
        let source = VPath::local(a.path().join("x")).unwrap();
        let dest = VPath::local(b.path()).unwrap();
        let report = copy(
            &LocalVfs,
            std::slice::from_ref(&source),
            &LocalVfs,
            &dest,
            None,
            &JobHandle::silent(),
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read(b.path().join("x")).unwrap(), b"old");
        let keep = Choose(Resolution::KeepBoth);
        let report = copy(
            &LocalVfs,
            &[source],
            &LocalVfs,
            &dest,
            Some(&keep),
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        assert_eq!(fs::read(b.path().join("x (2)")).unwrap(), b"new");
    }

    #[test]
    fn delete_requires_plan_token_and_removes_tree_bottom_up() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("f"), b"abc").unwrap();
        let root = VPath::local(&folder).unwrap();
        let plan = plan_delete(&LocalVfs, std::slice::from_ref(&root));
        assert_eq!((plan.files, plan.dirs, plan.bytes), (1, 1, 3));
        let wrong = delete_permanent(
            &LocalVfs,
            &plan,
            plan.token.wrapping_add(1),
            &JobHandle::silent(),
        );
        assert_eq!(wrong.done.len(), 0);
        assert!(folder.exists());
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(report.failed.is_empty());
        assert!(!folder.exists());
    }

    fn failed_with_changed(report: &OpReport, path: &std::path::Path) -> bool {
        report
            .failed
            .iter()
            .any(|(p, e)| p.path() == path && matches!(e, Error::ChangedSincePlan(_)))
    }

    #[test]
    fn delete_unchanged_tree_is_removed_completely() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/f"), b"1").unwrap();
        fs::write(root.join("g"), b"22").unwrap();
        std::os::unix::fs::symlink("g", root.join("l")).unwrap();
        let plan = plan_delete(&LocalVfs, &[VPath::local(&root).unwrap()]);
        assert!(!plan.incomplete);
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(report.failed.is_empty());
        assert_eq!(report.done.len(), 6);
        assert!(!root.exists());
    }

    #[test]
    fn delete_does_not_touch_folder_replaced_by_link_after_planning() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"data").unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/keep"), b"data").unwrap();
        let plan = plan_delete(&LocalVfs, &[VPath::local(&root).unwrap()]);
        fs::remove_dir_all(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("sub")).unwrap();
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(failed_with_changed(&report, &root.join("sub")));
        assert!(failed_with_changed(&report, &root.join("sub/keep")));
        assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"data");
        assert!(fs::symlink_metadata(root.join("sub"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(root.exists());
    }

    #[test]
    fn delete_does_not_remove_file_replaced_by_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        fs::write(&f, b"old").unwrap();
        let plan = plan_delete(&LocalVfs, &[VPath::local(&f).unwrap()]);
        // A second file keeps the old inode alive, so the new one cannot reuse it.
        let parked = dir.path().join("parked");
        fs::rename(&f, &parked).unwrap();
        fs::write(&f, b"new").unwrap();
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(failed_with_changed(&report, &f));
        assert_eq!(fs::read(&f).unwrap(), b"new");
        assert!(report.done.is_empty());
    }

    #[test]
    fn delete_keeps_folder_with_content_added_after_planning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("planned"), b"1").unwrap();
        let plan = plan_delete(&LocalVfs, &[VPath::local(&root).unwrap()]);
        fs::write(root.join("added"), b"new").unwrap();
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(!root.join("planned").exists());
        assert_eq!(fs::read(root.join("added")).unwrap(), b"new");
        assert!(root.exists());
        assert!(report
            .failed
            .iter()
            .any(|(p, e)| p.path() == root && matches!(e, Error::DirectoryNotEmpty(_))));
    }

    #[test]
    fn delete_plan_depth_limit_marks_incomplete_and_delete_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("d0");
        fs::create_dir_all(root.join("d1/d2/d3")).unwrap();
        fs::write(root.join("d1/d2/d3/f"), b"x").unwrap();
        let items = [VPath::local(&root).unwrap()];
        let ok = plan_delete_limited(&LocalVfs, &items, &JobHandle::silent(), 4, 100);
        assert!(!ok.incomplete);
        let plan = plan_delete_limited(&LocalVfs, &items, &JobHandle::silent(), 3, 100);
        assert!(plan.incomplete);
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(report.done.is_empty());
        assert!(report
            .failed
            .iter()
            .any(|(_, e)| matches!(e, Error::IncompletePlan)));
        assert!(root.join("d1/d2/d3/f").exists());
    }

    #[test]
    fn delete_plan_entry_limit_marks_incomplete_and_delete_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        for n in 0..10 {
            fs::write(root.join(format!("f{n}")), b"x").unwrap();
        }
        let items = [VPath::local(&root).unwrap()];
        let plan = plan_delete_limited(&LocalVfs, &items, &JobHandle::silent(), 10, 5);
        assert!(plan.incomplete);
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(report.done.is_empty());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 10);
        let full = plan_delete_limited(&LocalVfs, &items, &JobHandle::silent(), 10, 11);
        assert!(!full.incomplete);
    }

    #[test]
    fn cancelling_delete_planning_gives_unexecutable_plan() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        fs::write(&f, b"x").unwrap();
        let job = JobHandle::silent();
        job.cancel();
        let plan = plan_delete_cancellable(&LocalVfs, &[VPath::local(&f).unwrap()], &job);
        assert!(plan.incomplete);
        let report = delete_permanent(&LocalVfs, &plan, plan.token, &JobHandle::silent());
        assert!(report.done.is_empty());
        assert!(f.exists());
    }

    #[test]
    fn rename_and_create_directory_validate_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("old");
        fs::write(&file, b"x").unwrap();
        let old = VPath::local(&file).unwrap();
        let renamed = rename_entry(&LocalVfs, &old, "new").unwrap();
        assert_eq!(renamed.old, old);
        assert_eq!(renamed.new.path(), dir.path().join("new"));
        assert!(rename_entry(&LocalVfs, &renamed.new, "../bad").is_err());
        let parent = VPath::local(dir.path()).unwrap();
        let made = make_dir(&LocalVfs, &parent, "folder").unwrap();
        assert!(made.path().is_dir());
        assert!(matches!(
            make_dir(&LocalVfs, &parent, "folder"),
            Err(Error::Exists(_))
        ));
    }

    #[cfg(unix)]
    mod preserve {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        fn y2001() -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(978_307_200)
        }
        fn set_time(p: &std::path::Path, t: SystemTime) {
            fs::File::open(p).unwrap().set_modified(t).unwrap();
        }
        fn set_mode(p: &std::path::Path, mode: u32) {
            fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
        }
        fn mode(p: &std::path::Path) -> u32 {
            fs::metadata(p).unwrap().permissions().mode() & 0o777
        }
        fn mtime(p: &std::path::Path) -> SystemTime {
            fs::metadata(p).unwrap().modified().unwrap()
        }
        fn run(src: &std::path::Path, dst: &std::path::Path) -> OpReport {
            copy(
                &LocalVfs,
                &[VPath::local(src).unwrap()],
                &LocalVfs,
                &VPath::local(dst).unwrap(),
                None,
                &JobHandle::silent(),
            )
        }

        /// Local Vfs whose `set_metadata` always fails with the given error.
        struct FailMeta(fn(&VPath) -> Error);
        impl Vfs for FailMeta {
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
            fn set_metadata(&self, p: &VPath, _: Option<u32>, _: Option<SystemTime>) -> Result<()> {
                Err((self.0)(p))
            }
        }

        #[test]
        fn copy_keeps_time_and_mode_of_files_and_subdirectories() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let tree = a.path().join("tree");
            fs::create_dir_all(tree.join("sub")).unwrap();
            fs::write(tree.join("f640"), b"x").unwrap();
            fs::write(tree.join("sub/f755"), b"y").unwrap();
            set_mode(&tree.join("f640"), 0o640);
            set_mode(&tree.join("sub/f755"), 0o755);
            set_mode(&tree.join("sub"), 0o750);
            for rel in ["f640", "sub/f755", "sub"] {
                set_time(&tree.join(rel), y2001());
            }
            set_time(&tree, y2001());
            let report = run(&tree, b.path());
            assert!(report.failed.is_empty() && report.warnings.is_empty());
            let out = b.path().join("tree");
            for rel in ["f640", "sub/f755", "sub", ""] {
                assert_eq!(mtime(&out.join(rel)), y2001(), "time of {rel:?}");
            }
            assert_eq!(mode(&out.join("f640")), 0o640);
            assert_eq!(mode(&out.join("sub/f755")), 0o755);
            assert_eq!(mode(&out.join("sub")), 0o750);
        }

        #[test]
        fn read_only_directory_with_content_is_copied_and_keeps_mode_and_time() {
            if rustix::process::geteuid().is_root() {
                return; // root ignores directory permissions; the test would prove nothing
            }
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let ro = a.path().join("ro");
            fs::create_dir_all(ro.join("inner")).unwrap();
            fs::write(ro.join("inner/file"), b"data").unwrap();
            set_time(&ro.join("inner"), y2001());
            set_mode(&ro.join("inner"), 0o555);
            set_time(&ro, y2001());
            set_mode(&ro, 0o555);
            let report = run(&ro, b.path());
            let out = b.path().join("ro");
            let result = (
                report.failed.len(),
                report.warnings.len(),
                mode(&out),
                mode(&out.join("inner")),
                mtime(&out),
                mtime(&out.join("inner")),
                fs::read(out.join("inner/file")).ok(),
            );
            // Give permissions back so tempfile can delete everything.
            for p in [&ro, &ro.join("inner"), &out, &out.join("inner")] {
                set_mode(p, 0o755);
            }
            assert_eq!(
                result,
                (0, 0, 0o555, 0o555, y2001(), y2001(), Some(b"data".to_vec()))
            );
        }

        #[test]
        fn directory_time_survives_copying_its_children() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let dir = a.path().join("d");
            fs::create_dir(&dir).unwrap();
            for n in 0..5 {
                fs::write(dir.join(format!("f{n}")), b"x").unwrap();
            }
            set_time(&dir, y2001());
            let report = run(&dir, b.path());
            assert!(report.failed.is_empty());
            assert_eq!(mtime(&b.path().join("d")), y2001());
        }

        #[test]
        fn overwrite_and_keep_both_keep_time_and_mode() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let file = a.path().join("x");
            fs::write(&file, b"new").unwrap();
            set_mode(&file, 0o640);
            set_time(&file, y2001());
            fs::write(b.path().join("x"), b"old").unwrap();
            for (resolution, name) in [
                (Resolution::Overwrite, "x"),
                (Resolution::KeepBoth, "x (2)"),
            ] {
                let report = copy(
                    &LocalVfs,
                    &[VPath::local(&file).unwrap()],
                    &LocalVfs,
                    &VPath::local(b.path()).unwrap(),
                    Some(&Choose(resolution)),
                    &JobHandle::silent(),
                );
                assert!(report.failed.is_empty() && report.warnings.is_empty());
                let out = b.path().join(name);
                assert_eq!(fs::read(&out).unwrap(), b"new", "{name}");
                assert_eq!(mtime(&out), y2001(), "time of {name}");
                assert_eq!(mode(&out), 0o640, "mode of {name}");
            }
        }

        #[test]
        fn failing_set_metadata_is_a_warning_not_a_failure() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::create_dir(a.path().join("d")).unwrap();
            fs::write(a.path().join("d/f"), b"x").unwrap();
            let vfs = FailMeta(|p| Error::PermissionDenied(p.path().to_path_buf()));
            let report = copy(
                &vfs,
                &[VPath::local(a.path().join("d")).unwrap()],
                &vfs,
                &VPath::local(b.path()).unwrap(),
                None,
                &JobHandle::silent(),
            );
            assert!(report.failed.is_empty());
            assert_eq!(report.warnings.len(), 2, "one per file and folder");
            assert_eq!(fs::read(b.path().join("d/f")).unwrap(), b"x");
            assert!(report
                .done
                .contains(&VPath::local(a.path().join("d")).unwrap()));
        }

        #[test]
        fn unsupported_set_metadata_stays_silent() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            fs::write(a.path().join("f"), b"x").unwrap();
            let vfs = FailMeta(|_| Error::Unsupported("test"));
            let report = copy(
                &vfs,
                &[VPath::local(a.path().join("f")).unwrap()],
                &vfs,
                &VPath::local(b.path()).unwrap(),
                None,
                &JobHandle::silent(),
            );
            assert!(report.failed.is_empty() && report.warnings.is_empty());
            assert!(b.path().join("f").exists());
        }

        #[test]
        fn set_metadata_leaves_symlink_and_its_target_untouched() {
            let a = tempfile::tempdir().unwrap();
            let target = a.path().join("target");
            fs::write(&target, b"x").unwrap();
            set_mode(&target, 0o600);
            set_time(&target, y2001());
            let link = a.path().join("link");
            std::os::unix::fs::symlink("target", &link).unwrap();
            let other = y2001() + Duration::from_secs(86_400);
            LocalVfs
                .set_metadata(&VPath::local(&link).unwrap(), Some(0o777), Some(other))
                .unwrap();
            assert_eq!(mode(&target), 0o600);
            assert_eq!(mtime(&target), y2001());
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_ne!(
                fs::symlink_metadata(&link).unwrap().modified().unwrap(),
                other
            );
        }

        #[test]
        fn forced_copy_move_keeps_time_and_mode() {
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let file = a.path().join("m");
            fs::write(&file, b"x").unwrap();
            set_mode(&file, 0o640);
            set_time(&file, y2001());
            let report = move_with_strategy(
                &LocalVfs,
                &[VPath::local(&file).unwrap()],
                &LocalVfs,
                &VPath::local(b.path()).unwrap(),
                None,
                &JobHandle::silent(),
                MoveStrategy::ForceCopy,
            );
            assert!(report.failed.is_empty() && report.warnings.is_empty());
            assert!(!file.exists());
            assert_eq!(mtime(&b.path().join("m")), y2001());
            assert_eq!(mode(&b.path().join("m")), 0o640);
        }
    }
    struct CorruptWrite {
        inner: Box<dyn WriteFinish>,
        target: VPath,
    }
    impl Write for CorruptWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    impl WriteFinish for CorruptWrite {
        fn commit(&mut self) -> Result<()> {
            self.inner.commit()?;
            let mut data = fs::read(self.target.path()).unwrap();
            if let Some(byte) = data.first_mut() {
                *byte ^= 1;
            }
            fs::write(self.target.path(), data).unwrap();
            Ok(())
        }
    }
    struct CorruptVfs;
    impl Vfs for CorruptVfs {
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
            Ok(Box::new(CorruptWrite {
                inner: LocalVfs.create_write(p, m)?,
                target: p.clone(),
            }))
        }
        fn mkdir(&self, p: &VPath) -> Result<()> {
            LocalVfs.mkdir(p)
        }
        fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
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
    /// A reader that records how many bytes were actually pulled from it.
    struct CountingRead {
        inner: std::io::Cursor<Vec<u8>>,
        read: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    impl Read for CountingRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read
                .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            Ok(n)
        }
    }

    /// A non-local source that reports one size but streams a longer body — a lying server. The
    /// `read` counter records how many bytes the copy loop pulled, so a test can assert the bound.
    struct LyingSizeVfs {
        reported: u64,
        actual: Vec<u8>,
        read: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    impl Vfs for LyingSizeVfs {
        fn list(&self, _p: &VPath) -> Result<Vec<Entry>> {
            unimplemented!()
        }
        fn stat(&self, p: &VPath) -> Result<Entry> {
            Ok(Entry {
                uid: None,
                gid: None,
                name: p
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                kind: EntryKind::File,
                size: self.reported,
                modified: None,
                mode: None,
                hidden: false,
                link_target: None,
                broken_link: false,
                error: None,
            })
        }
        fn open_read(&self, _p: &VPath) -> Result<Box<dyn Read + Send>> {
            Ok(Box::new(CountingRead {
                inner: std::io::Cursor::new(self.actual.clone()),
                read: self.read.clone(),
            }))
        }
        fn create_write(&self, _p: &VPath, _m: WriteMode) -> Result<Box<dyn WriteFinish>> {
            unimplemented!()
        }
        fn mkdir(&self, _p: &VPath) -> Result<()> {
            unimplemented!()
        }
        fn rename(&self, _a: &VPath, _b: &VPath, _m: ReplaceMode) -> Result<()> {
            unimplemented!()
        }
        fn read_link(&self, _p: &VPath) -> Result<String> {
            unimplemented!()
        }
        fn create_symlink(&self, _t: &str, _p: &VPath) -> Result<()> {
            unimplemented!()
        }
        fn trash(&self, _p: &VPath) -> Result<()> {
            unimplemented!()
        }
        fn remove_permanent(&self, _p: &VPath) -> Result<()> {
            unimplemented!()
        }
        fn capabilities(&self) -> Caps {
            Caps::default()
        }
    }

    #[test]
    fn a_remote_source_longer_than_it_reports_is_rejected_and_the_staging_file_is_bounded() {
        let dstdir = tempfile::tempdir().unwrap();
        let src = VPath::local("/egal/data").unwrap(); // the fake ignores the path
        let dst = VPath::local(dstdir.path()).unwrap();
        let reported = 4u64;
        let read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sv = LyingSizeVfs {
            reported,
            // More than it reports; the `take` must stop the reader at reported+1 regardless of how
            // much the server would send (kept small here so the test stays light under load).
            actual: vec![b'x'; 64 * 1024],
            read: read.clone(),
        };
        let job = JobHandle::new(Arc::new(VerifyEvents::default()));
        let report = copy_verified(
            &sv,
            std::slice::from_ref(&src),
            &LocalVfs,
            &dst,
            None,
            &job,
            VerifyMode::Off,
        );
        assert_eq!(report.failed.len(), 1);
        assert!(matches!(report.failed[0].1, Error::SizeMismatch(_)));
        // The copy was refused, so no visible destination file remains.
        assert!(!dstdir.path().join("data").exists());
        // The point of the fix: the loop pulled at most reported+1 bytes, not the whole 10 MiB, so
        // a lying server cannot fill the staging file. (Without the `take`, this would be 10 MiB.)
        assert!(
            read.load(std::sync::atomic::Ordering::Relaxed) <= reported + 1,
            "read {} bytes, expected at most {}",
            read.load(std::sync::atomic::Ordering::Relaxed),
            reported + 1
        );
    }

    #[derive(Default)]
    struct VerifyEvents(Mutex<Vec<ProgressEvent>>);
    impl ProgressSink for VerifyEvents {
        fn event(&self, e: ProgressEvent) {
            self.0.lock().unwrap().push(e);
        }
    }

    #[test]
    fn sha256_detects_corruption_keeps_target_and_move_source() {
        for moving in [false, true] {
            let srcdir = tempfile::tempdir().unwrap();
            let dstdir = tempfile::tempdir().unwrap();
            let src = VPath::local(srcdir.path().join("data")).unwrap();
            let dst = VPath::local(dstdir.path()).unwrap();
            fs::write(src.path(), b"checksum data").unwrap();
            let events = Arc::new(VerifyEvents::default());
            let job = JobHandle::new(events.clone());
            let op = if moving { move_verified } else { copy_verified };
            let report = op(
                &LocalVfs,
                std::slice::from_ref(&src),
                &CorruptVfs,
                &dst,
                None,
                &job,
                VerifyMode::Sha256,
            );
            assert_eq!(report.failed.len(), 1);
            assert!(matches!(report.failed[0].1, Error::VerifyFailed));
            assert!(report.done.is_empty());
            assert_eq!(fs::read(src.path()).unwrap(), b"checksum data");
            assert_ne!(fs::read(dst.path().join("data")).unwrap(), b"checksum data");
            assert!(events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, ProgressEvent::ItemFailed(_))));
            assert!(events.0.lock().unwrap().iter().any(|e| matches!(
                e,
                ProgressEvent::BytesCopied {
                    bytes: 26,
                    total_bytes: 26,
                    ..
                }
            )));
        }
    }

    #[test]
    fn sha256_success_and_off_compatibility() {
        for verify in [VerifyMode::Off, VerifyMode::Sha256] {
            let srcdir = tempfile::tempdir().unwrap();
            let dstdir = tempfile::tempdir().unwrap();
            let src = VPath::local(srcdir.path().join("data")).unwrap();
            let dst = VPath::local(dstdir.path()).unwrap();
            fs::write(src.path(), vec![42; 2_100_000]).unwrap();
            let report = move_verified(
                &LocalVfs,
                std::slice::from_ref(&src),
                &LocalVfs,
                &dst,
                None,
                &JobHandle::silent(),
                verify,
            );
            assert!(report.failed.is_empty());
            assert!(!src.path().exists());
            assert_eq!(
                fs::read(dst.path().join("data")).unwrap(),
                vec![42; 2_100_000]
            );
        }
    }

    #[test]
    fn verify_policy_local_unknown_remote_never_always() {
        // /proc has no removable backing block device; independent of the test host's disks.
        let local = VPath::local("/proc/self").unwrap();
        let remote = crate::vfs::parse_ipc_path("sftp:test!/out").unwrap();
        assert!(!should_verify(&local, VerifyPolicy::Auto));
        assert!(should_verify(&remote, VerifyPolicy::Auto));
        for dest in [&local, &remote] {
            assert!(!should_verify(dest, VerifyPolicy::Never));
            assert!(should_verify(dest, VerifyPolicy::Always));
        }
    }

    #[test]
    fn standalone_pause_resume_and_cancel() {
        use std::sync::mpsc;
        use std::time::Duration;
        for cancel in [false, true] {
            let job = JobHandle::silent();
            job.pause();
            assert!(job.is_paused());
            let worker = job.clone();
            let (ready, ready_rx) = mpsc::channel();
            let (done, done_rx) = mpsc::channel();
            let t = std::thread::spawn(move || {
                ready.send(()).unwrap();
                done.send(worker.checkpoint()).unwrap();
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(done_rx.try_recv().is_err());
            if cancel {
                job.cancel();
            } else {
                job.resume();
            }
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                cancel
            );
            t.join().unwrap();
        }
    }
    #[test]
    fn cancellation_during_verification_keeps_move_source_and_target() {
        struct CancelVerification(Mutex<Option<JobHandle>>);
        impl ProgressSink for CancelVerification {
            fn event(&self, event: ProgressEvent) {
                if let ProgressEvent::BytesCopied {
                    bytes, total_bytes, ..
                } = event
                {
                    if bytes > total_bytes / 2 {
                        self.0.lock().unwrap().as_ref().unwrap().cancel();
                    }
                }
            }
        }
        let srcdir = tempfile::tempdir().unwrap();
        let dstdir = tempfile::tempdir().unwrap();
        let src = VPath::local(srcdir.path().join("data")).unwrap();
        let dst = VPath::local(dstdir.path()).unwrap();
        fs::write(src.path(), vec![7; 2 * 1024 * 1024]).unwrap();
        let sink = Arc::new(CancelVerification(Mutex::new(None)));
        let job = JobHandle::new(sink.clone());
        *sink.0.lock().unwrap() = Some(job.clone());
        let report = move_verified(
            &LocalVfs,
            std::slice::from_ref(&src),
            &LocalVfs,
            &dst,
            None,
            &job,
            VerifyMode::Sha256,
        );
        sink.0.lock().unwrap().take();
        assert!(report.cancelled);
        assert!(report.done.is_empty());
        assert!(src.path().exists());
        assert_eq!(
            fs::read(src.path()).unwrap(),
            fs::read(dst.path().join("data")).unwrap()
        );
    }

    #[test]
    fn verified_move_skip_does_not_prevent_later_source_from_moving() {
        let srcdir = tempfile::tempdir().unwrap();
        let dstdir = tempfile::tempdir().unwrap();
        let a = VPath::local(srcdir.path().join("a")).unwrap();
        let b = VPath::local(srcdir.path().join("b")).unwrap();
        let dst = VPath::local(dstdir.path()).unwrap();
        fs::write(a.path(), b"a").unwrap();
        fs::write(b.path(), b"b").unwrap();
        fs::write(dst.path().join("a"), b"existing").unwrap();
        let r = move_verified(
            &LocalVfs,
            &[a.clone(), b.clone()],
            &LocalVfs,
            &dst,
            None,
            &JobHandle::silent(),
            VerifyMode::Sha256,
        );
        assert_eq!(r.skipped, std::slice::from_ref(&a));
        assert!(r.failed.is_empty());
        assert_eq!(r.done, std::slice::from_ref(&b));
        assert!(a.path().exists());
        assert!(!b.path().exists());
        assert_eq!(fs::read(dst.path().join("a")).unwrap(), b"existing");
        assert_eq!(fs::read(dst.path().join("b")).unwrap(), b"b");
    }
}
