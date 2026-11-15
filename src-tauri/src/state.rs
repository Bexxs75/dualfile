//! Application state and the logic behind every command. No Tauri types in here: the
//! commands module only forwards, so everything is testable with a channel as event sink.

use crate::compare::{CompareStore, SyncStore};
use crate::conflict::{BridgeResolver, ConflictBridge, JobIdCell};
use crate::connections::{ConnectionState, RemoteParts};
use crate::convert::{
    delete_plan_dto, entry_dto, file_info_dto, io_error, job_status_dto, path_text, token_text,
    vpath_text, ConflictAnswerDto, CoreEventDto, DeletePlanDto, DeletePlanInput, DirectionDto,
    FileInfoDto, JobStatusDto, JobTitleDto, ListingDto, LoadedSettingsDto, PackFormatDto,
    RenameDto, RenameItemDto, SearchResultDto, SettingsDto, UiError,
};
use crate::plans::PlanStore;
use crate::plugins::{PluginParts, PluginState};
use crate::search::SearchStore;
use crate::validate::{
    check_name, parse_path, parse_paths_with, reject_archive_writes, reject_remote, reject_root,
    reject_roots, validate_settings, MAX_ITEMS, MAX_WATCHED, MSG_NO_REMOTE_TRASH,
};
use crate::watch::{WatchService, DEBOUNCE};
use dualfile_core::archive::{self, ArchiveLimits, ArchiveVfs, Budget};
use dualfile_core::connections::PROFILE_PATH_PREFIX;
use dualfile_core::jobs::{JobEvent, JobManager, JobSink, JobState, JobStatus};
use dualfile_core::lister;
use dualfile_core::multirename;
use dualfile_core::ops::{self, JobHandle, OpReport};
use dualfile_core::settings;
use dualfile_core::vfs::{EntryKind, LocalVfs, Scheme, VPath, Vfs};
use dualfile_core::VfsRouter;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Receives every event for the UI. Production: Tauri `emit("core-event", …)`.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEventDto);
}

/// Progress events arrive per copied block; the UI gets at most one per interval and job.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Throttle {
    last: HashMap<u64, Instant>,
}

impl Throttle {
    pub fn allow(&mut self, job: u64, now: Instant, interval: Duration) -> bool {
        match self.last.get(&job) {
            Some(last) if now.saturating_duration_since(*last) < interval => false,
            _ => {
                self.last.insert(job, now);
                true
            }
        }
    }

    pub fn forget(&mut self, job: u64) {
        self.last.remove(&job);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

type JobWork = Box<dyn FnOnce(&JobHandle) -> OpReport + Send>;

struct JobMeta {
    name: String,
    title: Option<JobTitleDto>,
    /// Folders whose content a finished job may have changed (as the UI knows them: a local
    /// path or `ftp:<id>!<path>`).
    dirs: Vec<String>,
    undo: bool,
}

struct PendingJob {
    meta: JobMeta,
    cell: Arc<JobIdCell>,
}

/// The job sink of the core: turns core events into UI events.
struct Shared {
    sink: Arc<dyn EventSink>,
    conflicts: Arc<ConflictBridge>,
    errors: Arc<crate::item_error::ErrorBridge>,
    metas: Mutex<HashMap<u64, JobMeta>>,
    /// Name and folders of the job being submitted; picked up by its `Queued` event.
    pending: Mutex<Option<PendingJob>>,
    manager: OnceLock<Weak<JobManager>>,
    throttle: Mutex<Throttle>,
}

impl Shared {
    fn name_of(&self, id: u64) -> String {
        lock(&self.metas)
            .get(&id)
            .map(|meta| meta.name.clone())
            .unwrap_or_default()
    }

    fn status_dto(&self, id: u64, status: &JobStatus) -> JobStatusDto {
        let mut dto = job_status_dto(id, &self.name_of(id), status);
        dto.title = lock(&self.metas)
            .get(&id)
            .and_then(|meta| meta.title.clone());
        if lock(&self.metas).get(&id).is_some_and(|meta| meta.undo) {
            if let Some(report) = status.report.as_ref() {
                dto.failed = report
                    .failed
                    .iter()
                    .map(|(_, error)| crate::convert::PathMessage {
                        kind: None,
                        path: String::new(),
                        message: match error {
                            dualfile_core::Error::Unsupported(code) => (*code).to_owned(),
                            _ => "io".into(),
                        },
                    })
                    .collect();
            }
        }
        dto
    }
    fn emit_status(&self, id: u64, status: &JobStatus) {
        self.sink.emit(CoreEventDto::Job {
            status: self.status_dto(id, status),
        });
    }

    fn emit_current(&self, id: u64) {
        let manager = self.manager.get().and_then(Weak::upgrade);
        if let Some(status) = manager.and_then(|m| m.status(id)) {
            self.emit_status(id, &status);
        }
    }
}

impl JobSink for Shared {
    fn event(&self, event: JobEvent) {
        match event {
            JobEvent::Queued(id) => {
                if let Some(pending) = lock(&self.pending).take() {
                    pending.cell.set(id);
                    lock(&self.metas).insert(id, pending.meta);
                }
                self.emit_current(id);
            }
            JobEvent::Started(id) => self.emit_current(id),
            JobEvent::Progress(id, status) => {
                // Zustandswechsel (Pause, Warten) dürfen nie von der Drosselung verschluckt werden:
                // ein pausierter Worker sendet danach keine Ereignisse mehr.
                if bypasses_throttle(status.state)
                    || lock(&self.throttle).allow(id, Instant::now(), PROGRESS_INTERVAL)
                {
                    self.emit_status(id, &status);
                }
            }
            JobEvent::Finished(id, _) => {
                self.emit_current(id);
                let dirs = lock(&self.metas)
                    .get(&id)
                    .map(|meta| meta.dirs.clone())
                    .unwrap_or_default();
                for dir in dirs {
                    self.sink.emit(CoreEventDto::DirChanged { path: dir });
                }
                self.conflicts.forget(id);
                self.errors.forget(id);
                lock(&self.throttle).forget(id);
            }
        }
    }
}

/// Nur laufende Fortschrittsmeldungen werden gedrosselt; jeder andere Zustand geht sofort raus.
fn bypasses_throttle(state: JobState) -> bool {
    state != JobState::Running
}

/// `Kopieren: name (+2)`, like the mock API.
pub fn describe(verb: &str, paths: &[VPath]) -> String {
    let first = first_name(paths);
    match paths.len() {
        0 | 1 => format!("{verb}: {first}"),
        more => format!("{verb}: {first} (+{})", more - 1),
    }
}

fn first_name(paths: &[VPath]) -> String {
    paths
        .first()
        .and_then(|path| path.path().file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Language-neutral counterpart of [`describe`] (`kind` is the key the UI translates).
pub fn title_of(kind: &str, paths: &[VPath]) -> JobTitleDto {
    JobTitleDto::new(kind, first_name(paths), paths.len().saturating_sub(1))
}

/// Server locations have no watcher; the UI reloads them by hand (and gets a `dirChanged` for
/// what its own jobs changed).
fn is_remote_text(raw: &str) -> bool {
    [
        "ftp:",
        "sftp:",
        "smb:",
        "webdav:",
        "plugin:",
        PROFILE_PATH_PREFIX,
    ]
    .iter()
    .any(|prefix| raw.starts_with(prefix))
}

/// What to watch for a location: the folder itself, or for a location inside an archive the
/// folder of the outermost archive file (a change of the archive file is reported as a change
/// of that folder).
fn watch_target(path: &VPath) -> PathBuf {
    let mut outermost = path;
    while let Some(outer) = outermost.outer() {
        outermost = outer;
    }
    if path.scheme() == Scheme::Archive {
        outermost
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| outermost.path().to_path_buf())
    } else {
        outermost.path().to_path_buf()
    }
}

fn parent_dirs(paths: &[VPath]) -> Vec<String> {
    let set: BTreeSet<String> = paths
        .iter()
        .filter_map(VPath::parent)
        .map(|parent| vpath_text(&parent))
        .collect();
    set.into_iter().collect()
}

/// True if both locations lie in the same place: the same folder tree, the same archive or
/// the same server connection.
fn same_place(a: &VPath, b: &VPath) -> bool {
    a.scheme() == b.scheme() && a.conn_id() == b.conn_id() && a.outer() == b.outer()
}

/// `xdg-open <path>` as an argument list (no shell). The path is absolute, so it can never
/// be taken for an option.
pub fn xdg_open_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    dualfile_core::appenv::apply_clean_env(&mut command);
    dualfile_core::appenv::apply_external_current_dir(&mut command);
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Largest remote file opened by copying it locally first. Bigger files should be fetched with
/// a deliberate copy job rather than silently streamed into temp.
const MAX_OPEN_EXTERNAL_BYTES: u64 = 512 << 20;

/// Total size of retained temp copies of opened remote files before the oldest are removed.
/// Bounds disk use when many files are opened in one session (the runtime dir is often a small
/// tmpfs). At least the newest copy is always kept.
const OPEN_TEMP_BUDGET: u64 = 2 << 30;

/// Base directory for the private temp copies of opened remote files: the per-user runtime dir
/// when the session provides one (the OS clears it at logout), otherwise the system temp dir.
fn open_temp_base() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::temp_dir(),
    }
}

/// Local file name for an opened remote file: the last path component with any directory
/// separators stripped, so the system opener keeps the original extension and nothing can
/// escape the temp directory. Falls back to a neutral name.
fn remote_open_name(path: &dualfile_core::vfs::VPath) -> String {
    let name = path
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_matches(|c| c == '/' || c == '\\'))
        .filter(|n| !n.is_empty() && n != &"." && n != &"..")
        .unwrap_or("datei");
    // The local copy is handed to xdg-open: a `.desktop` entry would be *launched* (its Exec=
    // line run) instead of shown. Neutralise that extension so the copy opens as plain text.
    if name.to_ascii_lowercase().ends_with(".desktop") {
        format!("{name}.txt")
    } else {
        name.to_string()
    }
}

/// Copies a remote file into a fresh private temp directory (0700) as a 0600 file and returns
/// the directory handle; the caller keeps it alive and dropping it removes the copy. `max` is
/// enforced both against the reported size and while streaming, so a server that under-reports
/// the size cannot make us write an unbounded file.
fn fetch_remote_to_temp(
    vfs: &dyn dualfile_core::vfs::Vfs,
    path: &dualfile_core::vfs::VPath,
    max: u64,
) -> Result<(tempfile::TempDir, u64), UiError> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    if vfs.stat(path)?.size > max {
        return Err(UiError::new(
            "unsupported",
            "Die Datei ist zu groß zum direkten Öffnen; bitte erst herunterladen",
        ));
    }
    let base = open_temp_base();
    let dir = tempfile::Builder::new()
        .prefix("dualfile-open-")
        .tempdir_in(&base)
        .map_err(|error| io_error("Das Temp-Verzeichnis", &base, &error))?;
    let local = dir.path().join(remote_open_name(path));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&local)
        .map_err(|error| io_error("Die lokale Kopie", &local, &error))?;
    let mut reader = vfs.open_read(path)?;
    // Stream with a hard ceiling in case `stat` under-reported the size.
    let copied = std::io::copy(&mut reader.by_ref().take(max + 1), &mut file)
        .map_err(|error| io_error("Die lokale Kopie", &local, &error))?;
    if copied > max {
        return Err(UiError::new(
            "unsupported",
            "Die Datei ist größer als gemeldet; Öffnen abgebrochen",
        ));
    }
    file.sync_all().ok();
    Ok((dir, copied))
}

/// Decompression budget of one search inside an archive.
const SEARCH_BYTES: u64 = 8 << 30;
const SEARCH_TIME: Duration = Duration::from_secs(120);

mod compare_impl;
mod connection_impl;
mod plugin_impl;
mod search_impl;
mod similar_impl;
mod space_impl;
mod undo_impl;

pub struct AppState {
    undo: Arc<dualfile_core::undo::UndoJournal>,
    undo_dirs: Arc<Mutex<HashMap<u64, Vec<String>>>>,
    home: PathBuf,
    config_dir: PathBuf,
    jobs: Arc<JobManager>,
    shared: Arc<Shared>,
    plans: PlanStore,
    usage: crate::space::ScanStore<dualfile_core::usage::UsageResult>,
    dupes: crate::space::ScanStore<dualfile_core::dupes::DupeResult>,
    similar: crate::space::ScanStore<dualfile_core::similar::SimilarResult>,
    thumbnails: crate::similar_space::ThumbnailGate,
    compares: Arc<CompareStore>,
    /// Results of the search over many files (see `state/search_impl.rs`).
    searches: Arc<SearchStore>,
    syncs: SyncStore,
    submit_lock: Mutex<()>,
    watcher: Option<WatchService>,
    router: Arc<VfsRouter>,
    /// Server connections: profiles, secrets, trust (see `state/connection_impl.rs`).
    connections: ConnectionState,
    /// Plugins: runtime and the ones switched on (see `state/plugin_impl.rs`).
    plugins: PluginState,
    /// Cancel flag of the running lister search; one search at a time (a new one cancels it).
    search: Mutex<Option<Arc<AtomicBool>>>,
    /// Temp copies of remote files opened externally, with each copy's size. Kept (the external
    /// program may still have the file open) until the app exits or the total exceeds
    /// `OPEN_TEMP_BUDGET`, when the oldest are removed. Cleared in `shutdown`. See `open_external`.
    open_temp: Mutex<Vec<(tempfile::TempDir, u64)>>,
}

impl AppState {
    #[cfg(any(test, not(unix)))]
    pub fn new(sink: Arc<dyn EventSink>, home: PathBuf, config_dir: PathBuf) -> Self {
        Self::with_remote(sink, home, config_dir, RemoteParts::production())
    }

    /// Like `new`, with the secret store and the server connectors given (tests plug in fake
    /// servers and a fake keychain). Nothing connects by itself, here or later: a server is
    /// only contacted by `connection_connect`.
    pub fn with_remote(
        sink: Arc<dyn EventSink>,
        home: PathBuf,
        config_dir: PathBuf,
        remote: RemoteParts,
    ) -> Self {
        Self::with_parts(sink, home, config_dir, remote, PluginParts::production())
    }

    /// Like `with_remote`, with the plugin folder and runtime settings given (tests use their
    /// own folder). Plugins confirmed earlier are registered, but no plugin process starts
    /// here: that happens at first use.
    pub fn with_parts(
        sink: Arc<dyn EventSink>,
        home: PathBuf,
        config_dir: PathBuf,
        remote: RemoteParts,
        plugins: PluginParts,
    ) -> Self {
        let conflicts = Arc::new(ConflictBridge::default());
        let shared = Arc::new(Shared {
            sink: sink.clone(),
            conflicts,
            errors: Arc::default(),
            metas: Mutex::default(),
            pending: Mutex::default(),
            manager: OnceLock::new(),
            throttle: Mutex::default(),
        });
        let jobs = Arc::new(JobManager::new(2, shared.clone()));
        let _ = shared.manager.set(Arc::downgrade(&jobs));
        let router = Arc::new(VfsRouter::new());
        let state = Self {
            undo: Arc::default(),
            undo_dirs: Arc::default(),
            home,
            config_dir,
            jobs,
            shared,
            plans: PlanStore::default(),
            usage: Default::default(),
            dupes: Default::default(),
            similar: Default::default(),
            thumbnails: Default::default(),
            compares: Arc::default(),
            searches: Arc::default(),
            syncs: SyncStore::default(),
            connections: ConnectionState::new(remote, router.clone()),
            router,
            submit_lock: Mutex::new(()),
            search: Mutex::new(None),
            open_temp: Mutex::new(Vec::new()),
            // Without a watcher (e.g. no inotify) everything else keeps working.
            watcher: WatchService::new(sink, DEBOUNCE).ok(),
            plugins: PluginState::new(plugins),
        };
        state.restore_plugins();
        state
    }

    /// A location from the UI: a local path, an archive entry, a server location of an open
    /// connection (`ftp:<id>!<path>`), or the persistent form `conn:<profile id>!<path>`,
    /// which only resolves while that profile is connected (`disconnected` otherwise; no
    /// connection is ever opened here).
    fn path(&self, raw: &str) -> Result<VPath, UiError> {
        if raw.starts_with(PROFILE_PATH_PREFIX) {
            return self.resolve_profile_path(raw);
        }
        parse_path(raw, &self.home)
    }

    pub fn space_free(&self, raw: &str) -> Option<dualfile_core::vfs::SpaceFree> {
        let path = self.path(raw).ok()?;
        if !matches!(
            path.scheme(),
            dualfile_core::vfs::Scheme::Local | dualfile_core::vfs::Scheme::Sftp
        ) {
            return None;
        }
        self.vfs(&path).ok()?.space_free(&path)
    }

    fn paths(&self, raw: &[String]) -> Result<Vec<VPath>, UiError> {
        parse_paths_with(raw, |text| self.path(text))
    }

    /// The `Vfs` that serves `path`: the local file system or the (cached) archive it lies in.
    fn vfs(&self, path: &VPath) -> Result<Arc<dyn Vfs>, UiError> {
        self.apply_plugin_failures();
        Ok(self.router.vfs_for(path)?)
    }

    /// Like `vfs`, for a job (copy, pack source): an archive starts the job with a fresh
    /// cost budget (the cached instance is shared and would otherwise add up over jobs).
    fn vfs_for_job(&self, path: &VPath) -> Result<Arc<dyn Vfs>, UiError> {
        let vfs = self.vfs(path)?;
        if let Some(archive) = self.archive_of(path)? {
            archive.reset_job_budget();
        }
        Ok(vfs)
    }

    /// The archive instance behind an archive location (`None` for local paths).
    fn archive_of(&self, path: &VPath) -> Result<Option<Arc<ArchiveVfs>>, UiError> {
        let Some(file) = path.outer() else {
            return Ok(None);
        };
        let budget = ArchiveVfs::new_index_budget(&ArchiveLimits::default());
        Ok(Some(self.router.archive_for(file, &budget)?))
    }

    /// Reading for the viewer: the shared cost budget is renewed once half of it is spent,
    /// so sequential reading keeps its decoder position but never runs into the job limit.
    fn vfs_for_read(&self, path: &VPath) -> Result<Arc<dyn Vfs>, UiError> {
        let vfs = self.vfs(path)?;
        if let Some(archive) = self.archive_of(path)? {
            if archive.cost().0 > ArchiveLimits::default().job_bytes / 2 {
                archive.reset_job_budget();
            }
        }
        Ok(vfs)
    }

    /// All sources must lie in the same place (the same folder tree or the same archive):
    /// one operation reads through one `Vfs`.
    fn source_vfs(&self, sources: &[VPath]) -> Result<Arc<dyn Vfs>, UiError> {
        let first = sources
            .first()
            .ok_or_else(|| UiError::new("invalidInput", "Keine Einträge angegeben"))?;
        if sources.iter().any(|source| !same_place(source, first)) {
            return Err(UiError::new(
                "invalidInput",
                "Die Einträge liegen nicht im selben Ordner, Archiv oder auf demselben Server",
            ));
        }
        self.vfs_for_job(first)
    }

    fn announce_dir(&self, dir: &str) {
        self.shared.sink.emit(CoreEventDto::DirChanged {
            path: dir.to_owned(),
        });
    }

    fn submit_job(
        &self,
        name: String,
        dirs: Vec<String>,
        build: impl FnOnce(BridgeResolver) -> JobWork,
    ) -> u64 {
        self.submit_job_with_lane(name, None, dirs, None, build)
    }

    fn submit_job_titled(
        &self,
        name: String,
        title: JobTitleDto,
        dirs: Vec<String>,
        build: impl FnOnce(BridgeResolver) -> JobWork,
    ) -> u64 {
        self.submit_job_with_lane(name, Some(title), dirs, None, build)
    }

    fn submit_job_with_lane(
        &self,
        name: String,
        title: Option<JobTitleDto>,
        dirs: Vec<String>,
        lane: Option<String>,
        build: impl FnOnce(BridgeResolver) -> JobWork,
    ) -> u64 {
        // One submission at a time: the `Queued` event picks up exactly this job's metadata.
        let _guard = lock(&self.submit_lock);
        let cell = Arc::new(JobIdCell::default());
        *lock(&self.shared.pending) = Some(PendingJob {
            meta: JobMeta {
                undo: name == "Rückgängig",
                name: name.clone(),
                title,
                dirs,
            },
            cell: cell.clone(),
        });
        let work = build(BridgeResolver {
            job: cell,
            bridge: self.shared.conflicts.clone(),
            sink: self.shared.sink.clone(),
        });
        self.jobs.submit_with_lane(name, lane, work)
    }

    // ---------------------------------------------------------------- Commands

    pub fn list(&self, path: &str) -> Result<ListingDto, UiError> {
        let path = self.path(path)?;
        let entries = self.vfs(&path)?.list(&path)?;
        Ok(ListingDto {
            path: vpath_text(&path),
            entries: entries.iter().map(entry_dto).collect(),
        })
    }

    pub fn file_info(&self, path: &str) -> Result<FileInfoDto, UiError> {
        let path = self.path(path)?;
        let vfs = self.vfs_for_read(&path)?;
        Ok(file_info_dto(&lister::file_info(&*vfs, &path)?))
    }

    /// Reads EXIF/ID3 tags for the given files, for the multi-rename placeholders. Only local
    /// files are read (metadata parsing needs a real path); remote, archive or unreadable paths
    /// yield empty tags, and the batch never fails as a whole.
    pub fn media_tags(&self, paths: Vec<String>) -> Vec<dualfile_core::mediatags::MediaTags> {
        paths
            .into_iter()
            .map(|raw| match self.path(&raw) {
                Ok(path) if path.scheme() == Scheme::Local => {
                    dualfile_core::mediatags::read_tags(path.path())
                }
                _ => dualfile_core::mediatags::MediaTags::default(),
            })
            .collect()
    }

    /// The core cuts `len` to `lister::MAX_BLOCK`; the UI's idea of sizes is never trusted.
    pub fn read_block(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>, UiError> {
        let path = self.path(path)?;
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        let vfs = self.vfs_for_read(&path)?;
        Ok(lister::read_block(&*vfs, &path, offset, len)?)
    }

    pub fn find_in_file(
        &self,
        path: &str,
        needle: &[u8],
        start: u64,
        direction: DirectionDto,
        ignore_case: bool,
    ) -> Result<SearchResultDto, UiError> {
        let path = self.path(path)?;
        let vfs = self.vfs(&path)?;
        if let Some(archive) = self.archive_of(&path)? {
            // One search, one budget: decompressed bytes and time are counted for all of it.
            archive.set_job_budget(Budget::new(SEARCH_BYTES, Some(SEARCH_TIME)));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        // A newer search replaces (and so cancels) the one still running.
        if let Some(previous) = lock(&self.search).replace(Arc::clone(&cancel)) {
            previous.store(true, Ordering::Relaxed);
        }
        let result = lister::find_in_file(
            &*vfs,
            &path,
            needle,
            start,
            direction.into(),
            ignore_case,
            &cancel,
        );
        let mut current = lock(&self.search);
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel)) {
            *current = None;
        }
        Ok(result?.into())
    }

    /// Stops the running search (it ends with the error kind `cancelled`); no search is fine.
    pub fn cancel_search(&self) {
        if let Some(flag) = lock(&self.search).as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn home_dir(&self) -> String {
        path_text(&self.home)
    }

    /// The destination of an operation must be an existing folder. Local folders keep their
    /// specific messages; on a server the back end answers.
    fn require_dir(&self, destination: &VPath, vfs: &dyn Vfs) -> Result<(), UiError> {
        if destination.scheme() == Scheme::Local {
            return match std::fs::metadata(destination.path()) {
                Ok(meta) if meta.is_dir() => Ok(()),
                Ok(_) => Err(UiError::new(
                    "notDirectory",
                    format!("Der Pfad ist kein Ordner: {}", destination.path().display()),
                )),
                Err(error) => Err(io_error("Der Zielordner", destination.path(), &error)),
            };
        }
        if vfs.stat(destination)?.kind == EntryKind::Dir {
            Ok(())
        } else {
            Err(UiError::new(
                "notDirectory",
                format!("Der Pfad ist kein Ordner: {}", destination.path().display()),
            ))
        }
    }

    fn transfer(
        &self,
        sources: &[String],
        destination: &str,
        moving: bool,
    ) -> Result<u64, UiError> {
        let sources = self.paths(sources)?;
        reject_roots(&sources)?;
        let destination = self.path(destination)?;
        // Into an archive and out of an archive by moving would write or delete there.
        reject_archive_writes(std::slice::from_ref(&destination))?;
        if moving {
            reject_archive_writes(&sources)?;
            self.require_writable(&sources)?;
        }
        self.require_writable(std::slice::from_ref(&destination))?;
        // Source and destination each get their own back end (local, archive or server).
        let source_vfs = self.source_vfs(&sources)?;
        let dest_vfs = self.vfs(&destination)?;
        self.require_dir(&destination, &*dest_vfs)?;
        let mut dirs = vec![vpath_text(&destination)];
        if moving {
            dirs.extend(parent_dirs(&sources));
        }
        let to_server = destination.scheme().is_remote();
        let (verb, kind) = match (moving, sources[0].scheme()) {
            (true, _) => ("Verschieben", "move"),
            (false, Scheme::Archive) => ("Entpacken", "extract"),
            (false, Scheme::Ftp | Scheme::Sftp | Scheme::Smb | Scheme::Webdav | Scheme::Plugin)
                if to_server =>
            {
                ("Kopieren", "copy")
            }
            (false, Scheme::Ftp | Scheme::Sftp | Scheme::Smb | Scheme::Webdav | Scheme::Plugin) => {
                ("Herunterladen", "download")
            }
            (false, Scheme::Local) if to_server => ("Hochladen", "upload"),
            (false, Scheme::Local) => ("Kopieren", "copy"),
        };
        let name = describe(verb, &sources);
        let title = title_of(kind, &sources);
        let recorder = self.undo_recorder(dirs.clone());
        let local = sources.iter().all(|p| p.scheme() == Scheme::Local)
            && destination.scheme() == Scheme::Local;
        let lane = crate::lanes::destination_lane(&destination);
        let errors = self.shared.errors.clone();
        let policy = settings::load(&self.config_dir).0.verify_policy;
        let verify = crate::verification::mode(&destination, policy);
        Ok(
            self.submit_job_with_lane(name, Some(title), dirs, lane, move |resolver| {
                Box::new(move |job| {
                    let job = job.with_error_resolver(Some(Arc::new(
                        crate::item_error::BridgeErrorResolver {
                            job: resolver.job.clone(),
                            bridge: errors,
                            sink: resolver.sink.clone(),
                        },
                    )));
                    let report = if moving {
                        ops::move_verified(
                            &*source_vfs,
                            &sources,
                            &*dest_vfs,
                            &destination,
                            Some(&resolver),
                            &job,
                            verify,
                        )
                    } else {
                        ops::copy_verified(
                            &*source_vfs,
                            &sources,
                            &*dest_vfs,
                            &destination,
                            Some(&resolver),
                            &job,
                            verify,
                        )
                    };
                    if local {
                        let pairs = undo_impl::roots(&report.created);
                        recorder.record(if moving {
                            recorder.journal.record_move(&pairs, report.replaced)
                        } else {
                            recorder.journal.record_copy_created(
                                &pairs.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(),
                                report.replaced,
                            )
                        });
                    }
                    report
                })
            }),
        )
    }

    /// Packs the sources into a new archive `name` inside the local folder `destination`, as
    /// a job. An existing file of that name is never replaced; the answer is the error kind
    /// `exists` (the UI offers a new name). Sources may also come from one archive.
    pub fn pack_archive(
        &self,
        sources: &[String],
        destination: &str,
        name: &str,
        format: PackFormatDto,
    ) -> Result<u64, UiError> {
        let sources = self.paths(sources)?;
        reject_roots(&sources)?;
        let destination = self.path(destination)?;
        reject_archive_writes(std::slice::from_ref(&destination))?;
        // Packing reads from anywhere but writes the new archive in a local folder only.
        if destination.scheme().is_remote() {
            return Err(UiError::new(
                "unsupported",
                "Archive werden nur in lokale Ordner gepackt, nicht auf einen Server",
            ));
        }
        check_name(name)?;
        match std::fs::metadata(destination.path()) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(UiError::new(
                    "notDirectory",
                    format!("Der Pfad ist kein Ordner: {}", destination.path().display()),
                ))
            }
            Err(error) => return Err(io_error("Der Zielordner", destination.path(), &error)),
        }
        let target = destination.join(name)?;
        if std::fs::symlink_metadata(target.path()).is_ok() {
            return Err(UiError::new(
                "exists",
                format!(
                    "Die Datei oder der Ordner ist bereits vorhanden: {}",
                    target.path().display()
                ),
            ));
        }
        let source_vfs = self.source_vfs(&sources)?;
        let job_name = describe("Packen", &sources);
        let title = title_of("pack", &sources);
        let dirs = vec![vpath_text(&destination)];
        let format = format.to_core();
        Ok(self.submit_job_with_lane(
            job_name,
            Some(title),
            dirs,
            crate::lanes::destination_lane(&destination),
            move |_| {
                Box::new(move |job| archive::pack(&*source_vfs, &sources, &target, format, job))
            },
        ))
    }

    pub fn copy(&self, sources: &[String], destination: &str) -> Result<u64, UiError> {
        self.transfer(sources, destination, false)
    }

    pub fn move_items(&self, sources: &[String], destination: &str) -> Result<u64, UiError> {
        self.transfer(sources, destination, true)
    }

    pub fn trash(&self, items: &[String]) -> Result<u64, UiError> {
        let items = self.paths(items)?;
        reject_archive_writes(&items)?;
        // A server has no trash; it is never replaced by deleting for good (that needs the
        // delete plan and its confirmation).
        reject_remote(&items, MSG_NO_REMOTE_TRASH)?;
        reject_roots(&items)?;
        let name = describe("Papierkorb", &items);
        let title = title_of("trash", &items);
        let dirs = parent_dirs(&items);
        let recorder = self.undo_recorder(dirs.clone());
        Ok(self.submit_job_titled(name, title, dirs, move |_| {
            Box::new(move |job| {
                let report = ops::trash_items(&LocalVfs, &items, job);
                recorder.record(recorder.journal.record_trash(&report.done));
                report
            })
        }))
    }

    pub fn restore_from_trash(&self, items: &[String]) -> Result<(), UiError> {
        let items = self.paths(items)?;
        reject_archive_writes(&items)?;
        reject_remote(&items, MSG_NO_REMOTE_TRASH)?;
        let report = ops::restore_from_trash(&items)?;
        if report.done.len() < items.len() {
            return Err(UiError::new(
                "notFound",
                "Nicht alle Einträge sind im Papierkorb vorhanden",
            ));
        }
        for dir in parent_dirs(&items) {
            self.announce_dir(&dir);
        }
        Ok(())
    }

    pub fn plan_delete(&self, items: &[String]) -> Result<DeletePlanDto, UiError> {
        let items = self.paths(items)?;
        reject_archive_writes(&items)?;
        reject_roots(&items)?;
        self.require_writable(&items)?;
        // One plan, one place: a plan on a server is bound to that connection.
        let vfs = self.source_vfs(&items)?;
        let plan = ops::plan_delete(&*vfs, &items);
        let dto = delete_plan_dto(&plan);
        let place = items[0]
            .conn_id()
            .map(|id| (items[0].scheme(), id.to_owned()));
        self.plans.insert(
            plan,
            describe("Löschen", &items),
            title_of("delete", &items),
            parent_dirs(&items),
            place,
        );
        Ok(dto)
    }

    /// Permanent delete: only with the token of a stored plan, and a plan counts once. A
    /// plan made on a server connection only runs on that connection; if it is gone (even if
    /// the same server was connected again, which gets a new connection id) the token is
    /// refused and nothing is deleted.
    pub fn delete_permanent(
        &self,
        plan: &DeletePlanInput,
        confirmed_token: &str,
    ) -> Result<u64, UiError> {
        let mismatch = || {
            UiError::new(
                "invalidToken",
                "Die Bestätigung gehört nicht zu diesem Löschplan",
            )
        };
        if plan.token != confirmed_token {
            return Err(mismatch());
        }
        let stored = self.plans.take(confirmed_token).ok_or_else(mismatch)?;
        let token = stored.plan.token;
        debug_assert_eq!(token_text(token), confirmed_token);
        let vfs: Arc<dyn Vfs> = match &stored.place {
            None => Arc::new(LocalVfs),
            Some((scheme, id)) => {
                let root = match scheme {
                    Scheme::Sftp => VPath::sftp(id, "/"),
                    Scheme::Smb => VPath::smb(id, "/"),
                    Scheme::Webdav => VPath::webdav(id, "/"),
                    Scheme::Plugin => VPath::plugin(id, "/"),
                    _ => VPath::ftp(id, "/"),
                }
                .map_err(|_| mismatch())?;
                self.router.vfs_for(&root).map_err(|_| mismatch())?
            }
        };
        Ok(
            self.submit_job_titled(stored.name, stored.title, stored.dirs, move |_| {
                Box::new(move |job| ops::delete_permanent(&*vfs, &stored.plan, token, job))
            }),
        )
    }

    pub fn rename(&self, path: &str, new_name: &str) -> Result<RenameDto, UiError> {
        let path = self.path(path)?;
        reject_archive_writes(std::slice::from_ref(&path))?;
        reject_root(&path)?;
        self.require_writable(std::slice::from_ref(&path))?;
        check_name(new_name)?;
        let vfs = self.vfs(&path)?;
        let old_path = vpath_text(&path);
        if path.path().file_name().and_then(|n| n.to_str()) == Some(new_name) {
            return Ok(RenameDto {
                new_path: old_path.clone(),
                old_path,
            });
        }
        let result = ops::rename_entry(&*vfs, &path, new_name)?;
        self.undo_recorder(parent_dirs(std::slice::from_ref(&path)))
            .record(self.undo.record_rename(&path, &result.new, false));
        if let Some(parent) = path.parent() {
            self.announce_dir(&vpath_text(&parent));
        }
        Ok(RenameDto {
            old_path,
            new_path: vpath_text(&result.new),
        })
    }

    /// Renames several entries of one folder as a job. Names are checked here for a quick
    /// answer and again by the core, which also checks duplicates, collisions and sources.
    /// The job's `failed` list holds old paths; `done` the new ones.
    pub fn rename_many(&self, folder: &str, items: &[RenameItemDto]) -> Result<u64, UiError> {
        if items.is_empty() {
            return Err(UiError::new(
                "invalidInput",
                "Keine Einträge zum Umbenennen",
            ));
        }
        if items.len() > MAX_ITEMS {
            return Err(UiError::new("invalidInput", "Zu viele Einträge"));
        }
        let folder = self.path(folder)?;
        reject_archive_writes(std::slice::from_ref(&folder))?;
        self.require_writable(std::slice::from_ref(&folder))?;
        let vfs = self.vfs(&folder)?;
        let mut old_paths = Vec::with_capacity(items.len());
        let mut pairs = Vec::with_capacity(items.len());
        for item in items {
            check_name(&item.old_name)?;
            check_name(&item.new_name)?;
            old_paths.push(folder.join(&item.old_name)?);
            pairs.push((item.old_name.clone(), item.new_name.clone()));
        }
        let name = describe("Umbenennen", &old_paths);
        let title = title_of("rename", &old_paths);
        let dirs = vec![vpath_text(&folder)];
        let recorder = self.undo_recorder(dirs.clone());
        Ok(self.submit_job_titled(name, title, dirs, move |_| {
            Box::new(
                move |job| match multirename::rename_many(&*vfs, &folder, &pairs, job) {
                    Ok(report) => {
                        let pairs = report
                            .renamed
                            .iter()
                            .map(|r| (r.old.clone(), r.new.clone()))
                            .collect::<Vec<_>>();
                        recorder.record(recorder.journal.record_renames(&pairs));
                        report.into_op_report()
                    }
                    Err(error) => OpReport {
                        failed: vec![(folder, error)],
                        ..OpReport::default()
                    },
                },
            )
        }))
    }

    pub fn make_dir(&self, parent: &str, name: &str) -> Result<String, UiError> {
        let parent = self.path(parent)?;
        reject_archive_writes(std::slice::from_ref(&parent))?;
        self.require_writable(std::slice::from_ref(&parent))?;
        check_name(name)?;
        let vfs = self.vfs(&parent)?;
        let created = ops::make_dir(&*vfs, &parent, name)?;
        self.undo_recorder(vec![vpath_text(&parent)])
            .record(self.undo.record_make_dir(&created));
        self.announce_dir(&vpath_text(&parent));
        Ok(vpath_text(&created))
    }

    pub fn open_external(&self, path: &str) -> Result<(), UiError> {
        let path = self.path(path)?;
        if path.scheme() == Scheme::Archive {
            return Err(UiError::new(
                "unsupported",
                "Inhalte eines Archivs können nicht direkt geöffnet werden; erst entpacken",
            ));
        }
        if path.scheme().is_remote() {
            // Server/plugin files cannot be handed to xdg-open directly: copy the file into a
            // private temp directory first, then open that local copy.
            let vfs = self.vfs_for_read(&path)?;
            let (dir, bytes) = fetch_remote_to_temp(vfs.as_ref(), &path, MAX_OPEN_EXTERNAL_BYTES)?;
            let local = dir.path().join(remote_open_name(&path));
            let mut child = xdg_open_command(&local).spawn().map_err(|_| {
                UiError::new("unsupported", "xdg-open konnte nicht gestartet werden")
            })?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            // Keep the copy (the opened program may still have it open), bounded by a total budget:
            // the oldest copies are dropped (their files removed) once the budget is exceeded.
            {
                let mut kept = self.open_temp.lock().unwrap();
                kept.push((dir, bytes));
                let mut total: u64 = kept.iter().map(|(_, b)| *b).sum();
                while total > OPEN_TEMP_BUDGET && kept.len() > 1 {
                    let (_, b) = kept.remove(0);
                    total -= b;
                }
            }
            return Ok(());
        }
        std::fs::metadata(path.path())
            .map_err(|error| io_error("Die Datei", path.path(), &error))?;
        let mut child = xdg_open_command(path.path())
            .spawn()
            .map_err(|_| UiError::new("unsupported", "xdg-open konnte nicht gestartet werden"))?;
        // Reap the child so no zombie stays behind; its exit status is of no interest.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    pub fn job_pause(&self, id: u64) -> Result<(), UiError> {
        if !self.jobs.status(id).is_some_and(|s| {
            matches!(
                s.state,
                JobState::Queued | JobState::Running | JobState::Paused
            )
        }) {
            return Err(UiError::new(
                "invalidInput",
                "Der Auftrag kann nicht pausiert werden",
            ));
        }
        self.jobs
            .pause(id)
            .then_some(())
            .ok_or_else(|| UiError::new("invalidInput", "Der Auftrag kann nicht pausiert werden"))
    }

    pub fn job_resume(&self, id: u64) -> Result<(), UiError> {
        if !self.jobs.status(id).is_some_and(|s| {
            matches!(
                s.state,
                JobState::Queued | JobState::Running | JobState::Paused
            )
        }) {
            return Err(UiError::new(
                "invalidInput",
                "Der Auftrag kann nicht fortgesetzt werden",
            ));
        }
        self.jobs.resume(id).then_some(()).ok_or_else(|| {
            UiError::new("invalidInput", "Der Auftrag kann nicht fortgesetzt werden")
        })
    }

    pub fn cancel_job(&self, id: u64) -> Result<(), UiError> {
        let Some(status) = self.jobs.status(id) else {
            return Err(UiError::new(
                "notFound",
                format!("Auftrag {id} nicht gefunden"),
            ));
        };
        if matches!(
            status.state,
            JobState::Queued | JobState::Running | JobState::Paused
        ) {
            self.jobs.cancel(id);
            self.shared.conflicts.cancel(id);
            self.shared.errors.cancel(id);
        }
        Ok(())
    }

    pub fn active_jobs_count(&self) -> usize {
        self.jobs
            .list()
            .iter()
            .filter(|(_, s)| active_job(s.state))
            .count()
    }
    pub fn answer_item_error(
        &self,
        id: u64,
        decision: crate::item_error::ErrorDecisionDto,
    ) -> Result<(), UiError> {
        if self.shared.errors.answer(id, decision.core()) {
            Ok(())
        } else {
            Err(UiError::new("notFound", "Keine offene Fehlerfrage"))
        }
    }
    pub fn answer_conflict(&self, job_id: u64, answer: ConflictAnswerDto) -> Result<(), UiError> {
        if self.shared.conflicts.answer(job_id, answer.to_core()) {
            Ok(())
        } else {
            Err(UiError::new(
                "notFound",
                format!("Auftrag {job_id} wartet auf keine Antwort"),
            ))
        }
    }

    pub fn jobs(&self) -> Vec<JobStatusDto> {
        self.jobs
            .list()
            .iter()
            .map(|(id, status)| self.shared.status_dto(*id, status))
            .collect()
    }

    pub fn load_settings(&self) -> LoadedSettingsDto {
        let (loaded, outcome) = settings::load(&self.config_dir);
        LoadedSettingsDto {
            settings: SettingsDto::from(&loaded),
            outcome: outcome.into(),
        }
    }

    /// The profile list is not part of what the UI saves: the stored profiles (with their
    /// pins) are kept as they are, and the file is written as the current version.
    pub fn save_settings(&self, new: &SettingsDto) -> Result<(), UiError> {
        validate_settings(new)?;
        let _guard = lock(&self.connections.settings_lock);
        let (current, _) = settings::load(&self.config_dir);
        let mut updated = settings::Settings::from(new);
        updated.connections = current.connections;
        updated.plugins = current.plugins;
        updated.version = settings::SETTINGS_VERSION;
        Ok(settings::save(&self.config_dir, &updated, false)?)
    }

    /// Replaces the set of watched folders. An empty list stops watching.
    pub fn watch(&self, paths: &[String]) -> Result<(), UiError> {
        if paths.len() > MAX_WATCHED {
            return Err(UiError::new(
                "invalidPath",
                "Zu viele Ordner zum Beobachten",
            ));
        }
        // Servers have no watcher: their locations are skipped (the UI reloads them by hand).
        let wanted = paths
            .iter()
            .filter(|raw| !is_remote_text(raw))
            .map(|raw| self.path(raw).map(|p| watch_target(&p)))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let Some(watcher) = &self.watcher else {
            return Err(UiError::new(
                "unsupported",
                "Die Ordnerbeobachtung ist nicht verfügbar",
            ));
        };
        // Folders that cannot be watched are simply not reported; refresh stays manual there.
        let _unwatchable = watcher.set_paths(&wanted);
        Ok(())
    }

    /// App exit: cancel all jobs (open questions count as `Cancel`) and wait until they have
    /// cleaned up their temporary files.
    pub fn shutdown(&self) {
        self.shared.conflicts.cancel_all();
        self.shared.errors.close();
        let active: Vec<u64> = self
            .jobs
            .list()
            .into_iter()
            .filter(|(_, status)| {
                matches!(
                    status.state,
                    JobState::Queued | JobState::Running | JobState::Paused
                )
            })
            .map(|(id, _)| id)
            .collect();
        for id in &active {
            self.jobs.cancel(*id);
            self.shared.conflicts.cancel(*id);
        }
        for id in active {
            let _ = self.jobs.wait(id);
        }
        // Remove the temp copies of remotely opened files (dropping each TempDir deletes it).
        self.open_temp.lock().unwrap().clear();
        self.close_connections();
        self.close_plugins();
    }
}

pub(crate) fn active_job(state: JobState) -> bool {
    matches!(
        state,
        JobState::Queued | JobState::Running | JobState::Paused
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn close_counts_only_active_states() {
        use dualfile_core::jobs::JobState::*;
        assert_eq!(
            [Queued, Running, Paused, Done, Failed, Cancelled]
                .into_iter()
                .filter(|s| super::active_job(*s))
                .count(),
            3
        );
    }
    use super::*;
    use crate::convert::ResolutionDto;
    use std::fs;
    use std::sync::mpsc::{channel, Receiver, Sender};

    struct ChannelSink(Mutex<Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = lock(&self.0).send(event);
        }
    }

    #[test]
    fn remote_open_copies_into_a_private_0600_file_and_enforces_the_cap() {
        use dualfile_core::error::Result as CoreResult;
        use dualfile_core::vfs::{Caps, Entry, ReplaceMode, WriteFinish, WriteMode};
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        struct Fake {
            size: u64,
            bytes: Vec<u8>,
        }
        impl Vfs for Fake {
            fn list(&self, _p: &VPath) -> CoreResult<Vec<Entry>> {
                unimplemented!()
            }
            fn stat(&self, _p: &VPath) -> CoreResult<Entry> {
                Ok(Entry {
                    name: "x".into(),
                    kind: EntryKind::File,
                    size: self.size,
                    modified: None,
                    mode: None,
                    hidden: false,
                    link_target: None,
                    broken_link: false,
                    error: None,
                })
            }
            fn open_read(&self, _p: &VPath) -> CoreResult<Box<dyn Read + Send>> {
                Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
            }
            fn create_write(&self, _p: &VPath, _m: WriteMode) -> CoreResult<Box<dyn WriteFinish>> {
                unimplemented!()
            }
            fn mkdir(&self, _p: &VPath) -> CoreResult<()> {
                unimplemented!()
            }
            fn rename(&self, _a: &VPath, _b: &VPath, _m: ReplaceMode) -> CoreResult<()> {
                unimplemented!()
            }
            fn read_link(&self, _p: &VPath) -> CoreResult<String> {
                unimplemented!()
            }
            fn create_symlink(&self, _t: &str, _a: &VPath) -> CoreResult<()> {
                unimplemented!()
            }
            fn trash(&self, _p: &VPath) -> CoreResult<()> {
                unimplemented!()
            }
            fn remove_permanent(&self, _p: &VPath) -> CoreResult<()> {
                unimplemented!()
            }
            fn capabilities(&self) -> Caps {
                Caps::default()
            }
        }

        let vpath = parse_path("/srv/bild.png", Path::new("/home/demo")).unwrap();

        // Normal copy: content and name preserved, file is private (0600).
        let fake = Fake {
            size: 5,
            bytes: b"hello".to_vec(),
        };
        let (dir, bytes) = fetch_remote_to_temp(&fake, &vpath, 1024).unwrap();
        assert_eq!(bytes, 5);
        let local = dir.path().join("bild.png");
        assert_eq!(fs::read(&local).unwrap(), b"hello");
        assert_eq!(
            fs::metadata(&local).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Rejected when the reported size exceeds the cap (nothing is streamed).
        let big = Fake {
            size: 2000,
            bytes: vec![0u8; 10],
        };
        assert!(fetch_remote_to_temp(&big, &vpath, 1024).is_err());

        // Rejected when a lying stat under-reports and the stream exceeds the cap.
        let lying = Fake {
            size: 1,
            bytes: vec![0u8; 4096],
        };
        assert!(fetch_remote_to_temp(&lying, &vpath, 1024).is_err());
    }

    #[test]
    fn remote_open_name_keeps_only_the_last_component() {
        let name = |p: &str| remote_open_name(&parse_path(p, Path::new("/home/demo")).unwrap());
        // Only the final path component is ever used as the local name, so the copy always
        // stays inside its private temp directory (and `parse_path` already rejects `..`).
        assert_eq!(name("/srv/a/b/bild.png"), "bild.png");
        assert_eq!(name("/srv/archiv.tar.gz"), "archiv.tar.gz");
        // A path with no usable final name falls back to a neutral one.
        assert_eq!(name("/"), "datei");
        // A `.desktop` name is neutralised (case-insensitive) so xdg-open shows the copy as
        // text instead of launching it; a name that only contains ".desktop" is untouched.
        assert_eq!(name("/srv/evil.desktop"), "evil.desktop.txt");
        assert_eq!(name("/srv/Evil.DESKTOP"), "Evil.DESKTOP.txt");
        assert_eq!(name("/srv/notes.desktop.txt"), "notes.desktop.txt");
    }

    #[test]
    fn undo_commands_record_and_emit_and_return_fixed_errors() {
        let f = fixture();
        let src = f.file("source/a", "abc");
        let dst = f.dir("dest");
        let job = f
            .state
            .move_items(std::slice::from_ref(&src), &dst)
            .unwrap();
        assert_eq!(f.finish(job).state, JobState::Done);
        let entry = f.state.undo_list().remove(0);
        assert_eq!(entry.kind, "move");
        assert_eq!(entry.label, "undoLabel.move");
        assert!(entry.time > 0);
        f.file("source/a", "occupied");
        let conflict = f.state.undo_run(entry.id);
        assert_eq!(conflict.errors, vec!["conflict"]);
        let status = f
            .state
            .jobs()
            .into_iter()
            .find(|j| j.id == conflict.job_id)
            .unwrap();
        assert_eq!(status.failed[0].path, "");
        assert_eq!(status.failed[0].message, "conflict");
        fs::remove_file(&src).unwrap();
        fs::write(f.p("dest/a"), "different length").unwrap();
        assert_eq!(f.state.undo_run(entry.id).errors, vec!["changed"]);
        let renamed = f.state.rename(&f.p("dest/a"), "b").unwrap();
        assert_eq!(f.state.undo_list()[0].kind, "rename");
        assert!(f.state.undo_run(f.state.undo_list()[0].id).ok);
        assert!(!Path::new(&renamed.new_path).exists());
        assert_eq!(
            f.state.undo_run(f.state.undo_list()[0].id).errors,
            vec!["alreadyUndone"]
        );
        assert_eq!(f.state.undo_run(u64::MAX).errors, vec!["notFound"]);
        let dir = f.state.make_dir(&dst, "new").unwrap();
        assert_eq!(f.state.undo_list()[0].kind, "mkdir");
        fs::write(Path::new(&dir).join("child"), "keep").unwrap();
        assert_eq!(
            f.state.undo_run(f.state.undo_list()[0].id).errors,
            vec!["changed"]
        );
        let events = f.drain();
        assert!(events.contains(&CoreEventDto::UndoChanged));
        assert!(events.contains(&CoreEventDto::DirChanged { path: dst }));
    }

    #[test]
    fn undo_copy_trash_mkdir_and_move_roundtrip() {
        let f = fixture();
        let src = f.file("source/a", "abc");
        let dst = f.dir("dest");
        let copied = f.state.copy(std::slice::from_ref(&src), &dst).unwrap();
        assert_eq!(f.finish(copied).state, JobState::Done);
        let copy = f.state.undo_list()[0].clone();
        assert_eq!(copy.kind, "copy");
        assert!(f.state.undo_run(copy.id).ok);
        assert!(!Path::new(&f.p("dest/a")).exists());
        // Restore and remove the temporary test payload through the public core path.
        f.state.restore_from_trash(&[f.p("dest/a")]).unwrap();
        let trash = f.state.trash(&[f.p("dest/a")]).unwrap();
        assert_eq!(f.finish(trash).state, JobState::Done);
        assert!(f.state.jobs.status(trash).unwrap().lane.is_none());
        let entry = f.state.undo_list()[0].clone();
        assert_eq!(entry.kind, "trash");
        assert!(f.state.undo_run(entry.id).ok);
        assert_eq!(f.read("dest/a"), "abc");
        let dir = f.state.make_dir(&dst, "empty").unwrap();
        assert!(f.state.undo_run(f.state.undo_list()[0].id).ok);
        f.state.restore_from_trash(&[dir]).unwrap();
        let target = f.dir("move-target");
        let job = f
            .state
            .move_items(std::slice::from_ref(&src), &target)
            .unwrap();
        f.finish(job);
        assert!(f.state.undo_run(f.state.undo_list()[0].id).ok);
        assert_eq!(fs::read_to_string(src).unwrap(), "abc");
    }

    #[test]
    fn undo_replacement_and_nonlocal_recording_are_excluded() {
        let f = fixture();
        let src = f.file("source/a", "new");
        let dst = f.dir("dest");
        f.file("dest/a", "old");
        let job = f.state.copy(&[src], &dst).unwrap();
        loop {
            if let CoreEventDto::Conflict { .. } = f.events.recv_timeout(WAIT).unwrap() {
                break;
            }
        }
        f.state
            .answer_conflict(
                job,
                ConflictAnswerDto {
                    resolution: ResolutionDto::Overwrite,
                    apply_to_all: true,
                },
            )
            .unwrap();
        f.finish(job);
        assert!(f.state.undo_list().is_empty());
        let local = VPath::local(f.p("dest/a")).unwrap();
        let remote = VPath::sftp("test", "/a").unwrap();
        let archive = parse_path("archive:/test.zip!/a", Path::new("/")).unwrap();
        let plugin = VPath::plugin("test", "/a").unwrap();
        let recorder = f.state.undo_recorder(vec![]);
        for source in [remote, archive, plugin] {
            recorder.record(f.state.undo.record_move(&[(source, local.clone())], false));
        }
        assert!(f.state.undo_list().is_empty());
        assert!(!f.drain().contains(&CoreEventDto::UndoChanged));
    }

    #[test]
    fn undo_partial_success_retries_only_remaining_items() {
        let f = fixture();
        let a = f.file("src/a", "a");
        let b = f.file("src/b", "b");
        let dest = f.dir("dest");
        let job = f.state.move_items(&[a.clone(), b.clone()], &dest).unwrap();
        f.finish(job);
        let id = f.state.undo_list()[0].id;
        fs::write(&b, "occupied").unwrap();
        let result = f.state.undo_run(id);
        assert_eq!(result.done, 1);
        assert_eq!(result.remaining, 1);
        assert_eq!(result.errors, vec!["conflict"]);
        assert!(!f.state.undo_list()[0].undone);
        assert!(Path::new(&a).exists());
        fs::remove_file(&b).unwrap();
        let result = f.state.undo_run(id);
        assert!(result.ok);
        assert_eq!(result.done, 1);
        assert!(f.state.undo_list()[0].undone);
    }

    #[test]
    fn undo_multirename_simple_batch_and_cycles() {
        let f = fixture();
        f.file("folder/a", "a");
        f.file("folder/b", "b");
        let items = vec![
            RenameItemDto {
                old_name: "a".into(),
                new_name: "c".into(),
            },
            RenameItemDto {
                old_name: "b".into(),
                new_name: "d".into(),
            },
        ];
        let job = f.state.rename_many(&f.p("folder"), &items).unwrap();
        f.finish(job);
        assert_eq!(f.state.undo_list().len(), 1);
        assert_eq!(f.state.undo_run(f.state.undo_list()[0].id).done, 2);
        let items = vec![
            RenameItemDto {
                old_name: "a".into(),
                new_name: "b".into(),
            },
            RenameItemDto {
                old_name: "b".into(),
                new_name: "a".into(),
            },
        ];
        let job = f.state.rename_many(&f.p("folder"), &items).unwrap();
        f.finish(job);
        assert_eq!(f.state.undo_list().len(), 1);
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

    impl Fixture {
        fn p(&self, rel: &str) -> String {
            self.root.path().join(rel).to_string_lossy().into_owned()
        }
        fn dir(&self, rel: &str) -> String {
            fs::create_dir_all(self.root.path().join(rel)).unwrap();
            self.p(rel)
        }
        fn file(&self, rel: &str, content: &str) -> String {
            if let Some(parent) = self.root.path().join(rel).parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(self.root.path().join(rel), content).unwrap();
            self.p(rel)
        }
        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.root.path().join(rel)).unwrap_or_default()
        }
        fn finish(&self, id: u64) -> JobStatus {
            self.state.jobs.wait(id).expect("job exists")
        }
        fn drain(&self) -> Vec<CoreEventDto> {
            self.events.try_iter().collect()
        }
    }

    #[test]
    fn only_running_progress_is_throttled() {
        assert!(!bypasses_throttle(JobState::Running));
        for state in [
            JobState::Paused,
            JobState::Queued,
            JobState::Done,
            JobState::Cancelled,
        ] {
            assert!(
                bypasses_throttle(state),
                "{state:?} darf nicht gedrosselt werden"
            );
        }
    }

    #[test]
    fn pause_resume_commands_and_cancel_paused_conflict() {
        let f = fixture();
        assert_eq!(f.state.job_pause(999).unwrap_err().kind, "invalidInput");
        assert_eq!(f.state.job_resume(999).unwrap_err().kind, "invalidInput");
        let src = f.file("source/a", "new");
        let dst = f.dir("dest");
        f.file("dest/a", "old");
        let id = f.state.copy(&[src], &dst).unwrap();
        loop {
            if matches!(
                f.events.recv_timeout(WAIT).unwrap(),
                CoreEventDto::Conflict { .. }
            ) {
                break;
            }
        }
        assert!(f.state.jobs.status(id).unwrap().lane.is_some());
        // Der Worker wartet auf die Konfliktentscheidung und erreicht keinen
        // Prüfpunkt: die Pause wird erst danach wirksam, der Zustand bleibt Running.
        f.state.job_pause(id).unwrap();
        assert_eq!(f.state.jobs.status(id).unwrap().state, JobState::Running);
        f.state.job_resume(id).unwrap();
        f.state.job_pause(id).unwrap();
        f.state.cancel_job(id).unwrap();
        assert_eq!(f.finish(id).state, JobState::Cancelled);
        assert!(f.state.job_resume(id).is_err());
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn list_returns_normalized_path_and_entries_and_rejects_bad_input() {
        let f = fixture();
        f.dir("a");
        f.file("a/x.txt", "hello");
        let listing = f.state.list(&format!("{}/", f.p("a"))).unwrap();
        assert_eq!(listing.path, f.p("a"));
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "x.txt");
        assert_eq!(listing.entries[0].size, 5);
        assert_eq!(f.state.list("relativ").unwrap_err().kind, "invalidPath");
        assert_eq!(f.state.list("/a/../b").unwrap_err().kind, "invalidPath");
        assert_eq!(f.state.list(&f.p("fehlt")).unwrap_err().kind, "notFound");
    }

    #[test]
    fn tilde_expands_to_the_home_folder() {
        let f = fixture();
        let listing = f.state.list("~").unwrap();
        assert_eq!(listing.path, f.p("home"));
        assert_eq!(f.state.home_dir(), f.p("home"));
    }

    #[test]
    fn copy_runs_as_job_reports_progress_and_refreshes_the_target() {
        let f = fixture();
        let src = f.file("src.txt", "inhalt");
        let dst = f.dir("dst");
        let id = f.state.copy(&[src], &dst).unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Done);
        assert_eq!(f.read("dst/src.txt"), "inhalt");
        let events = f.drain();
        let jobs: Vec<&JobStatusDto> = events
            .iter()
            .filter_map(|e| match e {
                CoreEventDto::Job { status } => Some(status),
                _ => None,
            })
            .collect();
        assert_eq!(jobs.first().map(|s| s.state), Some("queued"));
        assert_eq!(jobs.last().map(|s| s.state), Some("done"));
        assert!(jobs
            .iter()
            .all(|s| s.name == "Kopieren: src.txt" && s.id == id));
        assert!(events.contains(&CoreEventDto::DirChanged { path: dst }));
    }

    #[test]
    fn conflict_blocks_the_job_until_answered_and_never_overwrites_by_default() {
        let f = fixture();
        let src = f.file("a/f.txt", "neu");
        let dst = f.dir("b");
        f.file("b/f.txt", "alt");
        let id = f.state.copy(&[src], &dst).unwrap();
        let conflict = loop {
            match f.events.recv_timeout(WAIT).expect("conflict event") {
                CoreEventDto::Conflict { conflict } => break conflict,
                _ => continue,
            }
        };
        assert_eq!(conflict.job_id, id);
        assert_eq!(conflict.destination_path, f.p("b/f.txt"));
        // While the question is open nothing was touched and the job is still running.
        assert_eq!(f.read("b/f.txt"), "alt");
        assert_eq!(f.state.jobs.status(id).unwrap().state, JobState::Running);
        f.state
            .answer_conflict(
                id,
                ConflictAnswerDto {
                    resolution: ResolutionDto::KeepBoth,
                    apply_to_all: false,
                },
            )
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        assert_eq!(f.read("b/f.txt"), "alt");
        assert_eq!(f.read("b/f (2).txt"), "neu");
        // The question is closed: a second answer is refused.
        let again = f.state.answer_conflict(
            id,
            ConflictAnswerDto {
                resolution: ResolutionDto::Overwrite,
                apply_to_all: false,
            },
        );
        assert_eq!(again.unwrap_err().kind, "notFound");
    }

    #[test]
    fn cancelling_a_job_that_waits_for_an_answer_ends_it_without_changes() {
        let f = fixture();
        let src = f.file("a/f.txt", "neu");
        let dst = f.dir("b");
        f.file("b/f.txt", "alt");
        let id = f.state.copy(&[src], &dst).unwrap();
        loop {
            if let CoreEventDto::Conflict { .. } = f.events.recv_timeout(WAIT).unwrap() {
                break;
            }
        }
        f.state.cancel_job(id).unwrap();
        assert_eq!(f.finish(id).state, JobState::Cancelled);
        assert_eq!(f.read("b/f.txt"), "alt");
        assert_eq!(f.state.cancel_job(9999).unwrap_err().kind, "notFound");
        // Cancelling a finished job is harmless.
        assert!(f.state.cancel_job(id).is_ok());
    }

    #[test]
    fn copy_validates_sources_and_destination_before_any_job_starts() {
        let f = fixture();
        let file = f.file("f.txt", "x");
        assert_eq!(f.state.copy(&[], &f.p("")).unwrap_err().kind, "invalidPath");
        assert_eq!(
            f.state.copy(&["/".into()], &f.p("")).unwrap_err().kind,
            "invalidPath"
        );
        assert_eq!(
            f.state
                .copy(std::slice::from_ref(&file), &f.p("fehlt"))
                .unwrap_err()
                .kind,
            "notFound"
        );
        assert_eq!(
            f.state
                .copy(std::slice::from_ref(&file), &file)
                .unwrap_err()
                .kind,
            "notDirectory"
        );
        assert_eq!(
            f.state.copy(&["../x".into()], &f.p("")).unwrap_err().kind,
            "invalidPath"
        );
        assert!(f.state.jobs().is_empty());
    }

    #[test]
    fn move_refreshes_source_and_target_folders() {
        let f = fixture();
        let src = f.file("a/m.txt", "m");
        let dst = f.dir("b");
        let id = f.state.move_items(&[src], &dst).unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        assert_eq!(f.read("b/m.txt"), "m");
        assert!(!Path::new(&f.p("a/m.txt")).exists());
        let events = f.drain();
        assert!(events.contains(&CoreEventDto::DirChanged { path: f.p("a") }));
        assert!(events.contains(&CoreEventDto::DirChanged { path: f.p("b") }));
        assert!(f
            .state
            .jobs()
            .iter()
            .any(|j| j.name == "Verschieben: m.txt"));
    }

    #[test]
    fn permanent_delete_needs_the_matching_plan_token_exactly_once() {
        let f = fixture();
        let file = f.file("del/x.txt", "x");
        let other = f.file("del/y.txt", "y");
        let plan = f.state.plan_delete(std::slice::from_ref(&file)).unwrap();
        assert_eq!(plan.files, 1);
        let input = DeletePlanInput {
            token: plan.token.clone(),
        };

        // Wrong confirmation: refused, nothing deleted, plan still usable.
        let wrong = f.state.delete_permanent(&input, "00ff").unwrap_err();
        assert_eq!(wrong.kind, "invalidToken");
        // A token that was never planned is refused even when UI plan and token agree.
        let forged = DeletePlanInput {
            token: "abc".into(),
        };
        assert_eq!(
            f.state.delete_permanent(&forged, "abc").unwrap_err().kind,
            "invalidToken"
        );
        assert!(Path::new(&file).exists());

        let id = f.state.delete_permanent(&input, &plan.token).unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        assert!(!Path::new(&file).exists());
        assert!(Path::new(&other).exists());
        let job = f.state.jobs().into_iter().find(|j| j.id == id).unwrap();
        assert_eq!(job.name, "Löschen: x.txt");
        assert_eq!(job.files_done, 1);

        // Second use of the same plan: refused.
        assert_eq!(
            f.state
                .delete_permanent(&input, &plan.token)
                .unwrap_err()
                .kind,
            "invalidToken"
        );
    }

    #[test]
    fn plan_delete_refuses_the_root_and_relative_paths() {
        let f = fixture();
        assert!(f.state.plan_delete(&["/".into()]).is_err());
        assert!(f.state.plan_delete(&["x".into()]).is_err());
    }

    #[test]
    fn rename_validates_names_and_refuses_to_replace() {
        let f = fixture();
        let a = f.file("a.txt", "a");
        f.file("b.txt", "b");
        for bad in ["", ".", "..", "x/y"] {
            assert_eq!(
                f.state.rename(&a, bad).unwrap_err().kind,
                "invalidName",
                "{bad:?}"
            );
        }
        assert_eq!(f.state.rename(&a, "b.txt").unwrap_err().kind, "exists");
        assert_eq!(f.read("b.txt"), "b");
        let same = f.state.rename(&a, "a.txt").unwrap();
        assert_eq!(same.old_path, same.new_path);
        let done = f.state.rename(&a, "c.txt").unwrap();
        assert_eq!(done.new_path, f.p("c.txt"));
        assert_eq!(f.read("c.txt"), "a");
        assert!(f.drain().contains(&CoreEventDto::DirChanged {
            path: f.root.path().to_string_lossy().into_owned()
        }));
        assert_eq!(f.state.rename("/", "x").unwrap_err().kind, "invalidPath");
    }

    fn item(old: &str, new: &str) -> RenameItemDto {
        RenameItemDto {
            old_name: old.into(),
            new_name: new.into(),
        }
    }

    #[test]
    fn rename_many_runs_as_a_job_swaps_names_and_announces_the_folder() {
        let f = fixture();
        let folder = f.dir("work");
        f.file("work/a.txt", "A");
        f.file("work/b.txt", "B");
        f.file("work/c.txt", "C");
        let id = f
            .state
            .rename_many(
                &folder,
                &[
                    item("a.txt", "b.txt"),
                    item("b.txt", "a.txt"),
                    item("c.txt", "d.txt"),
                ],
            )
            .unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Done);
        assert!(status.lane.is_none());
        assert_eq!(f.read("work/a.txt"), "B");
        assert_eq!(f.read("work/b.txt"), "A");
        assert_eq!(f.read("work/d.txt"), "C");
        let events = f.drain();
        let states: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                CoreEventDto::Job { status } => Some(status.state),
                _ => None,
            })
            .collect();
        assert_eq!(states.first(), Some(&"queued"));
        assert_eq!(states.last(), Some(&"done"));
        assert!(events.contains(&CoreEventDto::DirChanged { path: folder }));
        assert!(events.iter().any(|e| matches!(
            e,
            CoreEventDto::Job { status } if status.name == "Umbenennen: a.txt (+2)"
        )));
    }

    #[test]
    fn rename_many_collision_fails_the_job_and_changes_nothing() {
        let f = fixture();
        let folder = f.dir("work");
        f.file("work/a.txt", "A");
        f.file("work/keep.txt", "K");
        let id = f
            .state
            .rename_many(&folder, &[item("a.txt", "keep.txt")])
            .unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Failed);
        let report = status.report.expect("report");
        assert_eq!(report.failed.len(), 1);
        assert_eq!(
            report.failed[0].0.path(),
            std::path::Path::new(&f.p("work/a.txt"))
        );
        assert_eq!(f.read("work/a.txt"), "A");
        assert_eq!(f.read("work/keep.txt"), "K");
    }

    #[test]
    fn rename_many_validates_paths_and_names_before_starting_a_job() {
        let f = fixture();
        let folder = f.dir("work");
        f.file("work/a.txt", "A");
        let ok = [item("a.txt", "b.txt")];
        for bad in ["relativ", "/work/../x", "/x\0y", ""] {
            let error = f.state.rename_many(bad, &ok).unwrap_err();
            assert_eq!(error.kind, "invalidPath", "{bad:?}");
        }
        for bad in ["", ".", "..", "x/y", "x\0y", "../a.txt"] {
            assert_eq!(
                f.state
                    .rename_many(&folder, &[item("a.txt", bad)])
                    .unwrap_err()
                    .kind,
                "invalidName",
                "{bad:?}"
            );
            assert_eq!(
                f.state
                    .rename_many(&folder, &[item(bad, "x")])
                    .unwrap_err()
                    .kind,
                "invalidName",
                "{bad:?}"
            );
        }
        assert_eq!(
            f.state.rename_many(&folder, &[]).unwrap_err().kind,
            "invalidInput"
        );
        assert_eq!(f.read("work/a.txt"), "A");
        assert!(f.drain().is_empty(), "no job was started");
    }

    #[test]
    fn rename_many_with_a_missing_folder_fails_the_job_not_the_files() {
        let f = fixture();
        let id = f
            .state
            .rename_many(&f.p("fehlt"), &[item("a", "b")])
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Failed);
    }

    #[test]
    fn make_dir_creates_validates_and_refuses_duplicates() {
        let f = fixture();
        let parent = f.dir("p");
        let created = f.state.make_dir(&parent, "neu").unwrap();
        assert_eq!(created, f.p("p/neu"));
        assert!(Path::new(&created).is_dir());
        assert_eq!(f.state.make_dir(&parent, "neu").unwrap_err().kind, "exists");
        assert_eq!(
            f.state.make_dir(&parent, "a/b").unwrap_err().kind,
            "invalidName"
        );
        assert_eq!(
            f.state.make_dir("rel", "x").unwrap_err().kind,
            "invalidPath"
        );
    }

    #[test]
    fn open_external_only_accepts_existing_paths() {
        let f = fixture();
        assert_eq!(
            f.state.open_external(&f.p("fehlt")).unwrap_err().kind,
            "notFound"
        );
        assert_eq!(
            f.state.open_external("relativ").unwrap_err().kind,
            "invalidPath"
        );
        assert_eq!(
            f.state.open_external("/a\0b").unwrap_err().kind,
            "invalidPath"
        );
    }

    #[test]
    fn xdg_open_cleans_appimage_environment() {
        // Isolate global environment writes from all other tests, including readers.
        const CHILD: &str = "DUALFILE_TEST_APPENV_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "state::tests::xdg_open_cleans_appimage_environment",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        static ENV: Mutex<()> = Mutex::new(());
        let _lock = ENV.lock().unwrap();
        struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
        let vars = [
            ("APPDIR", "/tmp/.mount_x"),
            ("HOME", "/"),
            ("APPIMAGE", "/home/me/Dualfile.AppImage"),
            ("PATH", "/tmp/.mount_x/bin:/usr/bin:/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_x/lib"),
            ("XDG_DATA_DIRS", "/tmp/.mount_x/share:/usr/share"),
            ("DUALFILE_TEST_UNKNOWN", "/tmp/.mount_x/unknown"),
        ];
        let _restore = Restore(
            vars.iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect(),
        );
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        let command = xdg_open_command(Path::new("/tmp/file"));
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
        let changes: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for key in [
            "APPDIR",
            "APPIMAGE",
            "LD_LIBRARY_PATH",
            "DUALFILE_TEST_UNKNOWN",
        ] {
            assert_eq!(changes.get(std::ffi::OsStr::new(key)), Some(&None), "{key}");
        }
        assert_eq!(
            changes[std::ffi::OsStr::new("PATH")],
            Some(std::ffi::OsStr::new("/usr/bin:/bin"))
        );
        assert_eq!(
            changes[std::ffi::OsStr::new("XDG_DATA_DIRS")],
            Some(std::ffi::OsStr::new("/usr/share"))
        );
        for value in changes.values().flatten() {
            assert!(!value
                .as_encoded_bytes()
                .windows(b"/tmp/.mount_x".len())
                .any(|part| part == b"/tmp/.mount_x"));
        }
    }

    #[test]
    fn xdg_open_is_started_directly_with_one_argument() {
        let command = xdg_open_command(Path::new("/tmp/a b; rm -rf x"));
        assert_eq!(command.get_program(), "xdg-open");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["/tmp/a b; rm -rf x"]);
    }

    #[test]
    fn settings_load_fresh_save_validate_and_round_trip() {
        let f = fixture();
        let loaded = f.state.load_settings();
        assert_eq!(loaded.outcome, crate::convert::LoadOutcomeDto::Fresh);
        let mut changed = loaded.settings.clone();
        changed.theme = "papier".into();
        changed.mirror = true;
        f.state.save_settings(&changed).unwrap();
        let again = f.state.load_settings();
        assert_eq!(again.outcome, crate::convert::LoadOutcomeDto::Loaded);
        assert_eq!(again.settings.theme, "papier");
        assert!(again.settings.mirror);
        changed.accent = "rot".into();
        assert_eq!(
            f.state.save_settings(&changed).unwrap_err().kind,
            "invalidSettings"
        );
        assert!(f.root.path().join("config/settings.json").exists());
    }

    #[test]
    fn lister_commands_validate_paths_before_touching_anything() {
        let f = fixture();
        for bad in ["relativ/datei", "/a/../b", "/a\0b", ""] {
            assert_eq!(
                f.state.file_info(bad).unwrap_err().kind,
                "invalidPath",
                "{bad:?}"
            );
            assert_eq!(
                f.state.read_block(bad, 0, 10).unwrap_err().kind,
                "invalidPath",
                "{bad:?}"
            );
            let found = f
                .state
                .find_in_file(bad, b"x", 0, DirectionDto::Forward, false);
            assert_eq!(found.unwrap_err().kind, "invalidPath", "{bad:?}");
        }
        assert_eq!(
            f.state.file_info(&f.p("fehlt")).unwrap_err().kind,
            "notFound"
        );
    }

    #[test]
    fn lister_commands_read_info_blocks_and_search_a_file() {
        let f = fixture();
        let path = f.file("home/a.txt", "Hallo Welt, hallo Mond");
        let info = f.state.file_info(&path).unwrap();
        assert_eq!((info.size, info.kind), (22, "text"));
        assert_eq!(f.state.read_block(&path, 6, 4).unwrap(), b"Welt");
        // `~` works like in the other commands.
        assert_eq!(f.state.read_block("~/a.txt", 0, 5).unwrap(), b"Hallo");
        let found = f
            .state
            .find_in_file(&path, b"HALLO", 1, DirectionDto::Forward, true)
            .unwrap();
        assert_eq!(found, SearchResultDto::Found { offset: 12 });
        let none = f
            .state
            .find_in_file(&path, b"Sonne", 0, DirectionDto::Forward, false)
            .unwrap();
        assert_eq!(none, SearchResultDto::NotFound);
        let empty = f
            .state
            .find_in_file(&path, b"", 0, DirectionDto::Forward, false);
        assert_eq!(empty.unwrap_err().kind, "invalidInput");
    }

    #[test]
    fn read_block_never_returns_more_than_one_mebibyte() {
        let f = fixture();
        let path = f.p("big.bin");
        fs::write(&path, vec![7u8; (1 << 20) + 10]).unwrap();
        assert_eq!(
            f.state.read_block(&path, 0, u64::MAX).unwrap().len(),
            1 << 20
        );
        assert_eq!(
            f.state.read_block(&path, 0, 1 << 30).unwrap().len(),
            1 << 20
        );
    }

    #[test]
    fn lister_commands_refuse_folders_and_special_files() {
        let f = fixture();
        let dir = f.dir("ordner");
        assert_eq!(f.state.file_info(&dir).unwrap_err().kind, "isADirectory");
        assert!(f.state.read_block(&dir, 0, 1).is_err());
        let fifo = f.p("pipe");
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let state = Arc::new(f.state);
        std::thread::spawn(move || {
            let _ = sender.send(state.read_block(&fifo, 0, 1).map(|_| ()));
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("a FIFO must not block the command");
        assert_eq!(result.unwrap_err().kind, "unsupported");
    }

    #[test]
    fn cancel_search_flags_the_registered_search_and_a_finished_search_unregisters() {
        let f = fixture();
        f.state.cancel_search(); // nothing running
        let path = f.p("long.bin");
        fs::write(&path, vec![0u8; 8 << 20]).unwrap();
        let state = Arc::new(f.state);
        // Register a pending flag like a running search would, then cancel it.
        let flag = Arc::new(AtomicBool::new(false));
        *lock(&state.search) = Some(Arc::clone(&flag));
        state.cancel_search();
        assert!(flag.load(Ordering::Relaxed));
        // A new search replaces the registered one and cancels it.
        let result = state
            .find_in_file(&path, b"x", 0, DirectionDto::Forward, false)
            .unwrap();
        assert_eq!(result, SearchResultDto::NotFound);
        assert!(
            lock(&state.search).is_none(),
            "finished search unregisters itself"
        );
    }

    #[test]
    fn watch_reports_changes_in_the_watched_folder_and_validates_input() {
        let f = fixture();
        let dir = f.dir("w");
        assert_eq!(
            f.state.watch(&["rel".into()]).unwrap_err().kind,
            "invalidPath"
        );
        let many: Vec<String> = (0..=MAX_WATCHED).map(|i| format!("/x{i}")).collect();
        assert!(f.state.watch(&many).is_err());
        f.state.watch(std::slice::from_ref(&dir)).unwrap();
        f.drain();
        f.file("w/new.txt", "x");
        let event = f.events.recv_timeout(WAIT).expect("dirChanged");
        assert_eq!(event, CoreEventDto::DirChanged { path: dir });
        f.state.watch(&[]).unwrap();
    }

    #[test]
    fn shutdown_cancels_a_job_waiting_for_an_answer() {
        let f = fixture();
        let src = f.file("a/f.txt", "neu");
        let dst = f.dir("b");
        f.file("b/f.txt", "alt");
        let id = f.state.copy(&[src], &dst).unwrap();
        loop {
            if let CoreEventDto::Conflict { .. } = f.events.recv_timeout(WAIT).unwrap() {
                break;
            }
        }
        f.state.shutdown();
        assert_eq!(f.state.jobs.status(id).unwrap().state, JobState::Cancelled);
        assert_eq!(f.read("b/f.txt"), "alt");
    }

    #[test]
    fn cancellation_unblocks_an_item_error_and_preserves_other_jobs() {
        let f = fixture();
        let source = f.file("a/missing.txt", "gone");
        fs::remove_file(&source).unwrap();
        let dst = f.dir("b");
        let id = f.state.copy(&[source], &dst).unwrap();
        loop {
            if let CoreEventDto::ItemError { error } = f.events.recv_timeout(WAIT).unwrap() {
                assert_eq!(error.job_id, id);
                assert_eq!(error.name, "missing.txt");
                assert!(!error
                    .message
                    .contains(f.root.path().to_string_lossy().as_ref()));
                break;
            }
        }
        assert_eq!(f.state.active_jobs_count(), 1);
        f.state.cancel_job(id).unwrap();
        f.state.jobs.wait(id).unwrap();
        assert_eq!(f.state.jobs.status(id).unwrap().state, JobState::Cancelled);
        assert_eq!(f.state.active_jobs_count(), 0);
        let source = f.file("a/ok.txt", "ok");
        let other = f.state.copy(&[source], &dst).unwrap();
        f.state.jobs.wait(other).unwrap();
        assert_eq!(f.state.jobs.status(other).unwrap().state, JobState::Done);
    }

    #[test]
    fn progress_is_throttled_per_job() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        let step = Duration::from_millis(40);
        assert!(throttle.allow(1, start, PROGRESS_INTERVAL));
        assert!(!throttle.allow(1, start + step, PROGRESS_INTERVAL));
        assert!(throttle.allow(2, start + step, PROGRESS_INTERVAL));
        assert!(throttle.allow(1, start + PROGRESS_INTERVAL, PROGRESS_INTERVAL));
        throttle.forget(1);
        assert!(throttle.allow(1, start + PROGRESS_INTERVAL, PROGRESS_INTERVAL));
    }

    #[test]
    fn job_names_count_the_additional_items() {
        let p = |s: &str| VPath::local(s).unwrap();
        assert_eq!(describe("Kopieren", &[p("/a/x")]), "Kopieren: x");
        assert_eq!(
            describe("Kopieren", &[p("/a/x"), p("/a/y"), p("/b/z")]),
            "Kopieren: x (+2)"
        );
    }

    #[test]
    fn job_titles_are_language_neutral() {
        let p = |s: &str| VPath::local(s).unwrap();
        assert_eq!(
            title_of("copy", &[p("/a/x")]),
            JobTitleDto::new("copy", "x", 0)
        );
        assert_eq!(
            title_of("move", &[p("/a/x"), p("/a/y"), p("/b/z")]),
            JobTitleDto::new("move", "x", 2)
        );
    }

    // ------------------------------------------------------------ Archives (phase 4b)

    fn failures(status: &JobStatus) -> Vec<String> {
        status.report.as_deref().map_or_else(Vec::new, |report| {
            report
                .failed
                .iter()
                .map(|(path, error)| format!("{}: {error}", path.path().display()))
                .collect()
        })
    }

    /// Packs `files` (relative to the fixture root) into `<root>/<name>` and waits.
    fn pack_zip(f: &Fixture, name: &str, files: &[&str]) {
        let sources: Vec<String> = files.iter().map(|rel| f.p(rel)).collect();
        let id = f
            .state
            .pack_archive(&sources, &f.p(""), name, PackFormatDto::Zip)
            .unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Done, "{:?}", failures(&status));
    }

    /// IPC text of `inner` in the archive file at `file` (with `%` and `!` encoded).
    fn arch(file: &str, inner: &str) -> String {
        format!(
            "archive:{}!{inner}",
            file.replace('%', "%25").replace('!', "%21")
        )
    }

    fn assert_read_only<T: std::fmt::Debug>(result: Result<T, UiError>) {
        let error = result.unwrap_err();
        assert_eq!(error.kind, "readOnly", "{error:?}");
        assert_eq!(error.message, "Archive sind schreibgeschützt");
    }

    #[test]
    fn pack_archive_roundtrip_then_list_view_and_extract_through_archive_paths() {
        let f = fixture();
        f.file("src/a.txt", "alpha");
        f.file("src/sub/b.txt", "beta");
        pack_zip(&f, "pack.zip", &["src"]);
        let zip = f.p("pack.zip");
        // the folder of the new file was announced
        assert!(f.drain().contains(&CoreEventDto::DirChanged {
            path: f.root.path().to_string_lossy().into_owned()
        }));
        // listing: the archive root and a folder inside; the path comes back in IPC format
        let root = f.state.list(&arch(&zip, "/")).unwrap();
        assert_eq!(root.path, arch(&zip, "/"));
        assert_eq!(root.entries.len(), 1);
        assert_eq!(root.entries[0].name, "src");
        let inner = f.state.list(&arch(&zip, "/src/")).unwrap();
        assert_eq!(inner.path, arch(&zip, "/src"));
        let mut names: Vec<_> = inner.entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "sub"]);
        // viewer paths: file info and block reading work inside the archive
        let file = arch(&zip, "/src/a.txt");
        assert_eq!(f.state.file_info(&file).unwrap().size, 5);
        assert_eq!(f.state.read_block(&file, 1, 3).unwrap(), b"lph");
        // search inside the archive file
        let found = f
            .state
            .find_in_file(&file, b"ph", 0, DirectionDto::Forward, false)
            .unwrap();
        assert!(
            matches!(found, SearchResultDto::Found { offset: 2 }),
            "{found:?}"
        );
        // extracting = the normal copy with a source in the archive
        let out = f.dir("out");
        let id = f.state.copy(&[arch(&zip, "/src/sub"), file], &out).unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Done, "{:?}", failures(&status));
        assert_eq!(f.read("out/sub/b.txt"), "beta");
        assert_eq!(f.read("out/a.txt"), "alpha");
        assert!(f.state.undo_list().is_empty());
        assert!(f
            .state
            .jobs()
            .iter()
            .any(|job| job.name == "Entpacken: sub (+1)"));
        // broken or missing archive paths answer with clear errors
        assert_eq!(
            f.state.list(&arch(&zip, "/nope")).unwrap_err().kind,
            "notFound"
        );
        f.file("plain.zip", "just text");
        let error = f.state.list(&arch(&f.p("plain.zip"), "/")).unwrap_err();
        assert_eq!(error.kind, "archive");
        assert_eq!(
            f.state.list("archive:/x.zip!/../../etc").unwrap_err().kind,
            "invalidPath"
        );
    }

    #[test]
    fn archive_names_with_exclamation_and_percent_marks_work() {
        let f = fixture();
        f.file("x.txt", "x");
        let id = f
            .state
            .pack_archive(
                &[f.p("x.txt")],
                &f.p(""),
                "we!rd%name.zip",
                PackFormatDto::TarGz,
            )
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        let zip = f.p("we!rd%name.zip");
        let text = arch(&zip, "/");
        assert!(text.contains("we%21rd%25name.zip"), "{text}");
        let listing = f.state.list(&text).unwrap();
        assert_eq!(listing.path, text);
        assert_eq!(listing.entries[0].name, "x.txt");
    }

    #[test]
    fn every_writing_command_refuses_archive_paths_and_leaves_the_archive_alone() {
        let f = fixture();
        f.file("src/a.txt", "alpha");
        pack_zip(&f, "pack.zip", &["src"]);
        let zip = f.p("pack.zip");
        let before = fs::read(&zip).unwrap();
        let jobs_before = f.state.jobs().len();
        let inside = arch(&zip, "/src/a.txt");
        let folder = arch(&zip, "/src");
        let local = f.file("local.txt", "local");
        let out = f.dir("out");
        assert_read_only(f.state.copy(std::slice::from_ref(&local), &folder));
        assert_read_only(f.state.move_items(std::slice::from_ref(&local), &folder));
        assert_read_only(f.state.move_items(std::slice::from_ref(&inside), &out));
        assert_read_only(f.state.trash(std::slice::from_ref(&inside)));
        assert_read_only(f.state.plan_delete(std::slice::from_ref(&inside)));
        assert_read_only(f.state.restore_from_trash(std::slice::from_ref(&inside)));
        assert_read_only(f.state.rename(&inside, "b.txt"));
        assert_read_only(f.state.make_dir(&folder, "neu"));
        assert_read_only(f.state.rename_many(
            &folder,
            &[RenameItemDto {
                old_name: "a.txt".into(),
                new_name: "b.txt".into(),
            }],
        ));
        assert_read_only(
            f.state
                .pack_archive(&[local], &folder, "x.zip", PackFormatDto::Zip),
        );
        assert_eq!(
            f.state.open_external(&inside).unwrap_err().kind,
            "unsupported"
        );
        assert_eq!(fs::read(&zip).unwrap(), before);
        assert_eq!(f.state.jobs().len(), jobs_before, "nothing was queued");
        assert_eq!(f.read("src/a.txt"), "alpha");
        assert_eq!(fs::read_dir(f.p("out")).unwrap().count(), 0);
    }

    #[test]
    fn the_core_refuses_too_when_a_job_reaches_it_with_an_archive_destination() {
        // belt and braces: ops::copy itself checks `is_read_only`
        let f = fixture();
        f.file("src/a.txt", "alpha");
        pack_zip(&f, "pack.zip", &["src"]);
        let zip = dualfile_core::parse_ipc_path(&arch(&f.p("pack.zip"), "/src")).unwrap();
        let local = VPath::local(f.p("src/a.txt")).unwrap();
        let vfs = f.state.router.vfs_for(&zip).unwrap();
        let report = ops::copy(&LocalVfs, &[local], &*vfs, &zip, None, &JobHandle::silent());
        assert_eq!(report.failed.len(), 1);
    }

    #[test]
    fn pack_archive_never_replaces_checks_input_and_cancels_cleanly() {
        let f = fixture();
        f.file("src/a.txt", "alpha");
        f.file("other/a.txt", "other");
        f.file("precious.zip", "precious");
        let source = f.p("src");
        let call = |name: &str| {
            f.state.pack_archive(
                std::slice::from_ref(&source),
                &f.p(""),
                name,
                PackFormatDto::SevenZ,
            )
        };
        assert_eq!(call("precious.zip").unwrap_err().kind, "exists");
        assert_eq!(f.read("precious.zip"), "precious");
        assert_eq!(call("a/b.7z").unwrap_err().kind, "invalidName");
        assert_eq!(call("..").unwrap_err().kind, "invalidName");
        assert_eq!(
            f.state
                .pack_archive(
                    std::slice::from_ref(&source),
                    &f.p("missing"),
                    "x.7z",
                    PackFormatDto::Zip
                )
                .unwrap_err()
                .kind,
            "notFound"
        );
        assert_eq!(
            f.state
                .pack_archive(
                    std::slice::from_ref(&source),
                    &f.p("src/a.txt"),
                    "x.7z",
                    PackFormatDto::Zip
                )
                .unwrap_err()
                .kind,
            "notDirectory"
        );
        assert_eq!(
            f.state
                .pack_archive(&[], &f.p(""), "x.7z", PackFormatDto::Zip)
                .unwrap_err()
                .kind,
            "invalidPath"
        );
        // sources from two different places are refused (one job reads through one place)
        pack_zip(&f, "two.zip", &["other"]);
        let mixed = f.state.pack_archive(
            &[f.p("src"), arch(&f.p("two.zip"), "/other")],
            &f.p(""),
            "m.zip",
            PackFormatDto::Zip,
        );
        assert_eq!(mixed.unwrap_err().kind, "invalidInput");
        // the archive inside the source folder is refused by the core: reported, not created
        let id = f
            .state
            .pack_archive(&[f.p("src")], &f.p("src"), "in.zip", PackFormatDto::Zip)
            .unwrap();
        let status = f.finish(id);
        assert_eq!(status.state, JobState::Failed);
        assert!(!f.root.path().join("src/in.zip").exists());
        // every format of the UI produces a readable archive
        for (format, name) in [
            (PackFormatDto::Zip, "f.zip"),
            (PackFormatDto::TarGz, "f.tar.gz"),
            (PackFormatDto::TarBz2, "f.tar.bz2"),
            (PackFormatDto::TarXz, "f.tar.xz"),
            (PackFormatDto::SevenZ, "f.7z"),
        ] {
            let id = f
                .state
                .pack_archive(std::slice::from_ref(&source), &f.p(""), name, format)
                .unwrap();
            assert_eq!(f.finish(id).state, JobState::Done, "{name}");
            let listing = f.state.list(&arch(&f.p(name), "/src")).unwrap();
            assert_eq!(listing.entries[0].name, "a.txt", "{name}");
        }
    }

    #[test]
    fn a_cancelled_pack_job_leaves_no_archive_and_no_staging_file() {
        let f = fixture();
        // enough data for many blocks so that the cancel arrives while the job is running
        let big = "x".repeat(8 << 20);
        for i in 0..4 {
            f.file(&format!("src/f{i}.bin"), &big);
        }
        let id = f
            .state
            .pack_archive(&[f.p("src")], &f.p(""), "c.tar.xz", PackFormatDto::TarXz)
            .unwrap();
        f.state.cancel_job(id).unwrap();
        let status = f.finish(id);
        assert!(
            matches!(status.state, JobState::Cancelled | JobState::Done),
            "{:?}",
            status.state
        );
        if status.state == JobState::Cancelled {
            let names: Vec<_> = fs::read_dir(f.p(""))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(!names.iter().any(|n| n.contains("c.tar.xz")), "{names:?}");
        }
    }

    #[test]
    fn watching_an_archive_location_watches_the_folder_of_its_archive_file() {
        let nested =
            dualfile_core::parse_ipc_path("archive:archive:/a/b/o.zip%21/in.tar!/x").unwrap();
        assert_eq!(watch_target(&nested), PathBuf::from("/a/b"));
        assert_eq!(
            watch_target(&VPath::local("/a/b").unwrap()),
            PathBuf::from("/a/b")
        );
        let f = fixture();
        f.file("x.txt", "x");
        pack_zip(&f, "w.zip", &["x.txt"]);
        f.state.watch(&[arch(&f.p("w.zip"), "/")]).unwrap();
    }
}
