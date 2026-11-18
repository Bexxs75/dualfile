//! Places discovery and interactions; all filesystem access runs in fixed isolated pools.
use crate::convert::{CoreEventDto, UiError};
use dualfile_core::{
    drives, fstab, places_pool, system_places, user_dirs,
    vfs::{LocalVfs, SpaceFree, VPath, Vfs},
};
use serde::Serialize;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

const DEVICE_WATCH: &str = "/dev/disk/by-id";
fn watch_event(
    event: &notify_debouncer_mini::notify::Result<notify_debouncer_mini::notify::Event>,
) -> (bool, bool) {
    use notify_debouncer_mini::notify::{event::ModifyKind, EventKind};
    let Ok(event) = event else {
        return (true, true);
    };
    let relevant = event.paths.iter().any(|p| {
        p.starts_with(DEVICE_WATCH)
            || p == std::path::Path::new("/etc/fstab")
            || bookmark_files().contains(p)
    });
    let lost = event.need_rescan()
        || (event
            .paths
            .iter()
            .any(|p| p == std::path::Path::new(DEVICE_WATCH))
            && matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any
            ));
    (relevant || lost, lost)
}
/// Desktop bookmark files whose change must refresh the sidebar's system places. Their
/// folders are watched (not the files) because Dolphin and GTK replace them by rename.
fn bookmark_files() -> &'static [std::path::PathBuf] {
    static FILES: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();
    FILES.get_or_init(|| {
        let paths = system_places::DiscoveryPaths::system();
        vec![
            paths.data.join("user-places.xbel"),
            paths.config.join("gtk-3.0/bookmarks"),
        ]
    })
}
fn rearm_device_watch(lost: &AtomicBool, rearm: impl FnOnce() -> bool) {
    // Clear before rearming so a concurrent loss notification cannot be overwritten.
    if lost.swap(false, Ordering::AcqRel) && !rearm() {
        lost.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    home: String,
    user_dirs: Vec<user_dirs::UserDir>,
    trash: Trash,
    drives: Vec<drives::Drive>,
    network_mounts: Vec<NetworkMount>,
    /// Desktop network bookmarks that are not mounted; the URL never carries a password.
    system_places: Vec<system_places::SystemPlace>,
    discovered_services: Vec<crate::discovery::Service>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMount {
    #[serde(flatten)]
    place: system_places::SystemPlace,
    mount_key: String,
    can_unmount: bool,
}
impl std::ops::Deref for NetworkMount {
    type Target = system_places::SystemPlace;
    fn deref(&self) -> &Self::Target {
        &self.place
    }
}
fn mount_key(mount: &system_places::SystemPlace) -> String {
    crate::lanes::opaque(&format!(
        "{}:{}",
        mount
            .mounted_at
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default(),
        mount.url
    ))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Trash {
    path: String,
    empty: bool,
}
fn read(path: impl AsRef<std::path::Path>, cap: u64) -> String {
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(cap).read_to_string(&mut text);
    }
    text
}
pub(crate) fn bounded_read(path: std::path::PathBuf, cap: u64) -> Option<String> {
    // Kernel control files do not enter the probed mount. Concurrent pane listings may
    // read them together; per-path exclusion here would spuriously reject the second pane.
    if path.starts_with("/proc") {
        return places_pool::control().run(Duration::from_millis(100), move || read(path, cap));
    }
    places_pool::metadata().run_keyed(
        path.to_string_lossy().into_owned(),
        Duration::from_millis(100),
        move || read(path, cap),
    )
}

fn mount_table() -> &'static drives::MountTableCache {
    static TABLE: OnceLock<drives::MountTableCache> = OnceLock::new();
    TABLE.get_or_init(drives::MountTableCache::default)
}
fn cached_mount_table() -> Option<Arc<str>> {
    mount_table().get_or_read(|| bounded_read("/proc/self/mountinfo".into(), 4 * 1024 * 1024))
}
fn discover(previous: Option<&Snapshot>) -> Option<Snapshot> {
    let paths = system_places::DiscoveryPaths::system();
    let home = paths.home.to_string_lossy().into_owned();
    let user_dirs = user_dirs::parse(
        &bounded_read(paths.config.join("user-dirs.dirs"), 65536).unwrap_or_default(),
        &home,
    )
    .into_iter()
    .filter(|d| {
        let path = d.path.clone();
        places_pool::paths()
            .run_keyed(path.clone(), Duration::from_millis(100), move || {
                std::path::Path::new(&path).is_dir()
            })
            .unwrap_or(false)
    })
    .collect();
    let trash_path = paths.data.join("Trash/files");
    let probe = trash_path.clone();
    let empty = places_pool::paths()
        .run_keyed(
            probe.to_string_lossy().into_owned(),
            Duration::from_millis(100),
            move || std::fs::read_dir(probe).map_or(true, |mut d| d.next().is_none()),
        )
        .unwrap_or(true);
    let table = cached_mount_table()?;
    let appdir = std::env::var("APPDIR").ok();
    let mounts = if table.is_empty() {
        drives::parse_mounts(
            &bounded_read("/proc/mounts".into(), 4 * 1024 * 1024).unwrap_or_default(),
            appdir.as_deref(),
        )
    } else {
        drives::parse_mountinfo(&table, appdir.as_deref())
    };
    let entries =
        fstab::parse_fstab(&bounded_read("/etc/fstab".into(), 256 * 1024).unwrap_or_default());
    let mut drives = fstab::merge(mounts, entries.clone());
    if let Ok(devices) = crate::devices::list() {
        dualfile_core::devices::merge(&mut drives, &devices, &table, &entries, crate::devices::key);
    } else if let Some(previous) = previous {
        // A busy/missing helper is not evidence that an unmounted device was unplugged.
        for old in &previous.drives {
            if old.source == "device"
                && !old.mounted
                && !drives.iter().any(|d| d.device == old.device)
            {
                drives.push(old.clone());
            }
        }
    }
    let user_mounts = current_user_mounts();
    for drive in &mut drives {
        if let Some(info) = &mut drive.fstab {
            info.user_mountable = fstab::permitted(
                info.permission.as_deref(),
                drive.mounted,
                info.permission.as_deref() == Some("owner") && device_owned(&drive.device),
                user_mounts.contains(&drive.path),
            );
        }
    }
    // Each probe owns a bounded slot: a mount point that does not answer in time counts as
    // unknown and stays visible, so a slow disk never removes a usable entry.
    let drives = drives::drop_inaccessible(drives, |drive| {
        let path = drive.path.clone();
        places_pool::paths()
            .run_keyed(path.clone(), Duration::from_millis(100), move || {
                drives::probe_access(&path)
            })
            .unwrap_or(drives::MountAccess::Unknown)
    });
    let (mounts, complete) =
        system_places::probe_mounts(&paths, places_pool::paths(), Duration::from_millis(500));
    // A missed deadline is not evidence that the bookmarks are gone: keep the last list.
    let system_places = match bounded_bookmarks() {
        Some(bookmarks) => system_places::unmounted_bookmarks(bookmarks, &mounts),
        None => previous.map_or_else(Vec::new, |p| p.system_places.clone()),
    };
    let mut network_mounts: Vec<_> = mounts.into_iter().filter_map(public_mount).collect();
    if !complete {
        if let Some(previous) = previous {
            for mount in &previous.network_mounts {
                if !network_mounts
                    .iter()
                    .any(|m| m.mounted_at == mount.mounted_at && m.url == mount.url)
                {
                    network_mounts.push(mount.clone());
                }
            }
        }
    }
    Some(Snapshot {
        home,
        user_dirs,
        trash: Trash {
            path: trash_path.to_string_lossy().into_owned(),
            empty,
        },
        drives,
        network_mounts,
        system_places,
        discovered_services: vec![],
    })
}
/// The bookmark files live in the home directory, which may itself be a slow network
/// mount, so reading them gets its own slot and deadline like every other places probe.
fn bounded_bookmarks() -> Option<Vec<system_places::SystemPlace>> {
    places_pool::metadata().run_keyed(
        "system-bookmarks".into(),
        Duration::from_millis(300),
        || system_places::bookmarks(&system_places::DiscoveryPaths::system()),
    )
}
/// Reading the kernel table is isolated from all probes that can enter a remote mount.
pub async fn navigation_mount(path: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let table = cached_mount_table()?;
        drives::navigation_mount(&path, &table)
    })
    .await
    .ok()
    .flatten()
}

fn public_mount(mut mount: system_places::SystemPlace) -> Option<NetworkMount> {
    let mount_key = mount_key(&mount);
    let can_unmount = mount.mounted_at.as_ref().is_some_and(|p| {
        p.starts_with(system_places::DiscoveryPaths::system().runtime.join("gvfs"))
    });
    // Authentication-bearing local paths cannot be navigated by the current IPC contract.
    // Keep a redacted display-only row until opaque navigation is supported.
    let local = mount.mounted_at.as_ref()?.to_str()?;
    if local.contains('@')
        || ["user=", "username=", "password=", "credentials="]
            .iter()
            .any(|key| local.contains(key))
    {
        mount.mounted_at = None;
    }
    mount.name = format!("{}{}", mount.host, mount.path);
    strip_account(&mut mount);
    Some(NetworkMount {
        place: mount,
        mount_key,
        can_unmount,
    })
}
trait SpaceProbe: Send + Sync {
    fn measure(&self, path: &str) -> Option<SpaceFree>;
}
struct LocalSpace;
impl SpaceProbe for LocalSpace {
    fn measure(&self, path: &str) -> Option<SpaceFree> {
        LocalVfs.space_free(&VPath::local(path).ok()?)
    }
}
fn fill_space(
    drives: &mut [drives::Drive],
    pool: &places_pool::Pool,
    probe: Arc<dyn SpaceProbe>,
    budget: Duration,
    per_entry: Duration,
) {
    let deadline = Instant::now() + budget;
    for drive in drives {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if drive.mounted && drives::local_space_type(&drive.fstype) {
            let probe = probe.clone();
            let path = drive.path.clone();
            drive.space = pool
                .run(per_entry.min(remaining), move || probe.measure(&path))
                .flatten();
        }
    }
}
fn measure(path: String) -> Option<SpaceFree> {
    places_pool::network_space()
        .run_keyed(path.clone(), Duration::from_millis(300), move || {
            LocalSpace.measure(&path)
        })
        .flatten()
}
pub struct Places {
    visible: AtomicBool,
    stop: AtomicBool,
    cache: Mutex<Option<Snapshot>>,
    refreshing: Mutex<()>,
    cache_ready: Condvar,
    restored_sidebar: Mutex<Option<Option<f64>>>,
}
impl Default for Places {
    fn default() -> Self {
        Self {
            visible: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            cache: Mutex::new(None),
            refreshing: Mutex::new(()),
            cache_ready: Condvar::new(),
            restored_sidebar: Mutex::new(Some(None)),
        }
    }
}
impl Places {
    fn refresh(&self) -> Option<(Snapshot, bool)> {
        let _refresh = self.refreshing.try_lock().ok()?;
        let previous = self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        mount_table().invalidate();
        let snapshot = discover(previous.as_ref())?;
        let changed = self.cache_snapshot(snapshot.clone());
        Some((snapshot, changed))
    }
    fn cache_snapshot(&self, snapshot: Snapshot) -> bool {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let changed = cache.as_ref().is_some_and(|old| old != &snapshot);
        *cache = Some(snapshot);
        self.cache_ready.notify_all();
        changed
    }
    fn cached_snapshot(&self, timeout: Duration) -> Option<Snapshot> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.cache_ready
            .wait_timeout_while(cache, timeout, |cache| cache.is_none())
            .unwrap_or_else(|e| e.into_inner())
            .0
            .clone()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
#[tauri::command]
pub async fn places_snapshot(
    app_state: tauri::State<'_, Arc<crate::state::AppState>>,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<Places>>,
) -> Result<Snapshot, UiError> {
    main_only(window.label())?;
    let state = state.inner().clone();
    let discovery = app_state.discovery.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut snapshot = state
            .refresh()
            .map(|(s, _)| s)
            .or_else(|| state.cached_snapshot(Duration::from_millis(1500)))
            .ok_or_else(|| UiError::new("timeout", "Places unavailable"))?;
        fill_space(
            &mut snapshot.drives,
            places_pool::space(),
            Arc::new(LocalSpace),
            Duration::from_millis(1500),
            Duration::from_millis(300),
        );
        snapshot.discovered_services = discovery.snapshot();
        Ok(snapshot)
    })
    .await
    .map_err(|_| UiError::internal())?
}
#[tauri::command]
pub fn discovery_snapshot(
    app_state: tauri::State<'_, Arc<crate::state::AppState>>,
    window: tauri::WebviewWindow,
) -> Result<Vec<crate::discovery::Service>, UiError> {
    main_only(window.label())?;
    Ok(app_state.discovery.snapshot())
}
#[tauri::command]
pub fn places_set_visible(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<Places>>,
    visible: bool,
) -> Result<(), UiError> {
    main_only(window.label())?;
    state.visible.store(visible, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
pub async fn places_space(
    window: tauri::WebviewWindow,
    path: String,
) -> Result<Option<SpaceFree>, UiError> {
    main_only(window.label())?;
    if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(UiError::new("invalidPath", "Invalid place"));
    }
    tauri::async_runtime::spawn_blocking(move || measure(path))
        .await
        .map_err(|_| UiError::internal())
}
#[tauri::command]
pub async fn trash_open_system(window: tauri::WebviewWindow) -> Result<(), UiError> {
    main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(|| {
        let mut command = std::process::Command::new("xdg-open");
        command.arg("trash:/");
        dualfile_core::appenv::launch_isolated(command, Duration::from_secs(5))
            .map_err(|_| UiError::new("unavailable", "System trash unavailable"))
    })
    .await
    .map_err(|_| UiError::internal())?
}
pub(crate) fn main_only(label: &str) -> Result<(), UiError> {
    if label == "main" {
        Ok(())
    } else {
        Err(UiError::new("permissionDenied", "Main window only"))
    }
}

fn open_monitor() -> Option<std::fs::File> {
    let mut file = std::fs::File::open("/proc/self/mountinfo").ok()?;
    // Reading acknowledges the proc poll generation before the descriptor is armed.
    let mut sink = std::io::sink();
    std::io::copy(&mut (&mut file).take(4 * 1024 * 1024), &mut sink).ok()?;
    Some(file)
}

#[derive(Default)]
struct WatchDiff {
    pending: Option<Duration>,
    last: Option<Snapshot>,
}
impl WatchDiff {
    fn dirty(&mut self, now: Duration) {
        self.pending = Some(now + Duration::from_millis(300));
    }
    fn due(&mut self, now: Duration) -> bool {
        if self.pending.is_some_and(|deadline| now >= deadline) {
            self.pending = None;
            true
        } else {
            false
        }
    }
    fn publish(&mut self, snapshot: Snapshot) -> bool {
        let changed = self.last.as_ref().is_none_or(|old| old != &snapshot);
        self.last = Some(snapshot);
        changed
    }
}

/// poll is only a wake-up source. Discovery itself has its own workers and deadline.
pub fn start(app: tauri::AppHandle, state: Arc<Places>) {
    let restored = places_pool::metadata()
        .run_keyed(
            "settings-restore".into(),
            Duration::from_millis(300),
            || {
                let path = dualfile_core::settings::config_dir().join("settings.json");
                match std::fs::File::open(path) {
                    Ok(file) => {
                        let mut text = String::new();
                        file.take(1024 * 1024).read_to_string(&mut text).ok()?;
                        let settings = serde_json::from_str(&text).ok()?;
                        Some(restored_sidebar_width(&settings))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0.0),
                    Err(_) => None,
                }
            },
        )
        .flatten();
    *state
        .restored_sidebar
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(restored);
    std::thread::spawn(move || {
        use std::os::fd::AsRawFd;
        let mut file = places_pool::control()
            .run_keyed(
                "mount-monitor".into(),
                Duration::from_millis(300),
                open_monitor,
            )
            .flatten();
        if file.is_none() {
            log::warn!("Mount notifications unavailable; polling places");
        }
        let mut last_poll = Instant::now() - Duration::from_secs(5);
        let mut last_monitor = Duration::ZERO;
        let clock = Instant::now();
        let mut diff = WatchDiff::default();
        if let Some((snapshot, _)) = state.refresh() {
            if diff.publish(snapshot) {
                let _ = app.emit(crate::CORE_EVENT, CoreEventDto::PlacesChanged);
            }
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        use notify_debouncer_mini::notify::{RecursiveMode, Watcher};
        let watch_tx = tx.clone();
        let device_watch_lost = Arc::new(AtomicBool::new(true));
        let watch_lost = device_watch_lost.clone();
        let watcher = places_pool::metadata()
            .run(Duration::from_millis(300), move || {
                let event_lost = watch_lost.clone();
                let mut watcher =
                    notify_debouncer_mini::notify::recommended_watcher(move |event| {
                        let (changed, lost) = watch_event(&event);
                        if lost {
                            event_lost.store(true, Ordering::Release);
                        }
                        if changed {
                            let _ = watch_tx.try_send(());
                        }
                    })
                    .ok()?;
                let _ = watcher.watch(std::path::Path::new("/etc"), RecursiveMode::NonRecursive);
                // Missing folders (no Dolphin, no GTK) are fine: the visible-sidebar poll
                // still picks up bookmark changes within a few seconds.
                for file in bookmark_files() {
                    if let Some(folder) = file.parent() {
                        let _ = watcher.watch(folder, RecursiveMode::NonRecursive);
                    }
                }
                rearm_device_watch(&watch_lost, || {
                    watcher
                        .watch(
                            std::path::Path::new(DEVICE_WATCH),
                            RecursiveMode::NonRecursive,
                        )
                        .is_ok()
                });
                Some(watcher)
            })
            .flatten()
            .map(|w| Arc::new(Mutex::new(w)));
        let mut last_device_watch = Instant::now();
        while !state.stop.load(Ordering::Relaxed) {
            let mut changed = rx.try_recv().is_ok();
            if device_watch_lost.load(Ordering::Acquire)
                && last_device_watch.elapsed() >= Duration::from_secs(5)
            {
                last_device_watch = Instant::now();
                if let Some(watcher) = watcher.clone() {
                    let watch_lost = device_watch_lost.clone();
                    let _ = places_pool::metadata().run_keyed(
                        "device-watch".into(),
                        Duration::from_millis(100),
                        move || {
                            let Ok(mut watcher) = watcher.try_lock() else {
                                return;
                            };
                            rearm_device_watch(&watch_lost, || {
                                let path = std::path::Path::new(DEVICE_WATCH);
                                let _ = watcher.unwatch(path);
                                watcher.watch(path, RecursiveMode::NonRecursive).is_ok()
                            });
                        },
                    );
                }
            }
            if let Some(mounts) = &file {
                let mut fd = libc::pollfd {
                    fd: mounts.as_raw_fd(),
                    events: libc::POLLPRI | libc::POLLERR,
                    revents: 0,
                };
                // SAFETY: fd points to one initialized pollfd for the duration of poll.
                let result = unsafe { libc::poll(&mut fd, 1, 100) };
                if result < 0 {
                    log::warn!("Mount notifications failed; polling places");
                    file = None;
                } else if result > 0 {
                    changed = true;
                    file = places_pool::control()
                        .run_keyed(
                            "mount-monitor".into(),
                            Duration::from_millis(300),
                            open_monitor,
                        )
                        .flatten();
                }
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
            if changed {
                mount_table().invalidate();
                diff.dirty(clock.elapsed());
            }
            retry_monitor(&mut file, &mut last_monitor, clock.elapsed(), || {
                places_pool::control()
                    .run_keyed(
                        "mount-monitor".into(),
                        Duration::from_millis(100),
                        open_monitor,
                    )
                    .flatten()
            });
            let poll_due = poll_due(state.visible.load(Ordering::Relaxed), last_poll.elapsed());
            // Do not ask the main thread for visibility on every 100 ms poll pass.
            let visible_poll = poll_due
                && app
                    .get_webview_window("main")
                    .is_some_and(|window| window.is_visible().unwrap_or(false));
            if poll_due {
                last_poll = Instant::now();
            }
            if diff.due(clock.elapsed()) || visible_poll {
                if let Some((snapshot, _)) = state.refresh() {
                    if diff.publish(snapshot) {
                        let _ = app.emit(crate::CORE_EVENT, CoreEventDto::PlacesChanged);
                    }
                }
            }
        }
    });
}

fn poll_due(visible: bool, elapsed: Duration) -> bool {
    visible && elapsed >= Duration::from_secs(5)
}
fn retry_monitor<T>(
    file: &mut Option<T>,
    last_attempt: &mut Duration,
    now: Duration,
    open: impl FnOnce() -> Option<T>,
) {
    if file.is_none() && now.saturating_sub(*last_attempt) >= Duration::from_secs(5) {
        *last_attempt = now;
        *file = open();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeResult {
    actual_delta: f64,
    can_grow: bool,
    restored_width: f64,
}
fn restored_sidebar_width(settings: &serde_json::Value) -> f64 {
    match settings["sidebar_mode"].as_str() {
        Some("full") => settings["sidebar_width"]
            .as_u64()
            .unwrap_or(210)
            .clamp(160, 400) as f64,
        Some("icons") => 44.0,
        _ => 0.0,
    }
}
fn resized_width(current: u32, delta: i32, maximum: u32, minimum: u32) -> u32 {
    (current as i64 + delta as i64).clamp(minimum.min(maximum) as i64, maximum as i64) as u32
}
fn initial_resize(delta: f64, restored: Option<Option<f64>>) -> (f64, f64) {
    match restored {
        Some(Some(width)) => (delta - width, width),
        Some(None) => (0.0, 0.0), // Unknown restored geometry: never grow it a second time.
        None => (delta, 0.0),
    }
}
fn resize_result(requested: f64, applied: f64, room: bool, restored_width: f64) -> ResizeResult {
    ResizeResult {
        actual_delta: applied,
        can_grow: room && (applied - requested).abs() < 1.0,
        restored_width,
    }
}
#[tauri::command]
pub async fn sidebar_resize_window(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<Places>>,
    delta: f64,
) -> Result<ResizeResult, UiError> {
    main_only(window.label())?;
    if !delta.is_finite() || delta.abs() > 400.0 {
        return Err(UiError::new("invalidInput", "Invalid sidebar width"));
    }
    resize_transaction(delta, &state.restored_sidebar, |delta, credit| {
        resize_geometry(window, delta, credit)
    })
    .await
}
async fn resize_transaction<F, R>(
    delta: f64,
    restored_sidebar: &Mutex<Option<Option<f64>>>,
    resize: F,
) -> Result<ResizeResult, UiError>
where
    F: FnOnce(f64, f64) -> R,
    R: std::future::Future<Output = Result<ResizeResult, UiError>>,
{
    if delta == 0.0 {
        return Ok(resize_result(0.0, 0.0, false, 0.0));
    }
    let restored = restored_sidebar
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let (delta, credit) = initial_resize(delta, restored);
    if delta == 0.0 {
        return Ok(resize_result(0.0, 0.0, false, credit));
    }
    resize(delta, credit).await
}
/// Growth to credit to the sidebar. Fractional scaling rounds sizes by up to `tolerance` px,
/// and compositors may stop short, so near misses and partial moves count; a move the wrong
/// way or past the request (a user drag during the settle time) counts nothing.
fn confirmed_resize_delta(before: u32, requested: u32, actual: u32, tolerance: u32) -> f64 {
    let want = requested as i64 - before as i64;
    let got = actual as i64 - before as i64;
    if want == 0 || got == 0 || want.signum() != got.signum() {
        return 0.0;
    }
    if (requested as i64 - actual as i64).abs() <= tolerance as i64 {
        want as f64
    } else if got.abs() < want.abs() {
        got as f64
    } else {
        0.0
    }
}
/// Maximized and fullscreen windows keep their size. So does a window that is not shown: after
/// a minimized start it was never mapped, the toolkit still reports its creation size instead of
/// the restored one, and resizing from that would overwrite the saved window size.
fn geometry_fixed(visible: bool, maximized: bool, fullscreen: bool) -> bool {
    !visible || maximized || fullscreen
}
async fn resize_geometry(
    window: tauri::WebviewWindow,
    delta: f64,
    credit: f64,
) -> Result<ResizeResult, UiError> {
    let error = |_| UiError::new("unavailable", "Window geometry unavailable");
    let scale = window.scale_factor().map_err(error)?;
    let before = window.inner_size().map_err(error)?;
    if geometry_fixed(
        window.is_visible().map_err(error)?,
        window.is_maximized().map_err(error)?,
        window.is_fullscreen().map_err(error)?,
    ) {
        return Ok(ResizeResult {
            actual_delta: 0.0,
            can_grow: false,
            restored_width: credit,
        });
    }
    let monitor = window
        .current_monitor()
        .map_err(error)?
        .ok_or_else(|| UiError::new("unavailable", "Monitor unavailable"))?;
    let area = monitor.work_area();
    let outer = window.outer_size().map_err(error)?;
    let decoration = outer.width.saturating_sub(before.width);
    let width = resized_width(
        before.width,
        (delta * scale).round() as i32,
        area.size.width.saturating_sub(decoration),
        (900.0 * scale) as u32,
    );
    window
        .set_size(tauri::PhysicalSize::new(width, before.height))
        .map_err(error)?;
    // Let the compositor apply or refuse a tiled-window request; no UI-thread sleep.
    tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(120)))
        .await
        .map_err(|_| UiError::internal())?;
    let actual = window.inner_size().map_err(error)?;
    let applied = if actual.height == before.height {
        confirmed_resize_delta(before.width, width, actual.width, scale.ceil() as u32) / scale
    } else {
        0.0
    };
    if applied != 0.0 && !crate::window_state::is_wayland() {
        if let Ok(position) = window.outer_position() {
            let x = (position.x - (applied * scale).round() as i32).clamp(
                area.position.x,
                area.position.x + area.size.width.saturating_sub(actual.width + decoration) as i32,
            );
            let _ = window.set_position(tauri::PhysicalPosition::new(x, position.y));
        }
    }
    Ok(resize_result(
        delta,
        applied,
        actual.width < area.size.width.saturating_sub(decoration),
        credit,
    ))
}
pub(crate) fn mount_error(code: &str) -> UiError {
    UiError::new(code, code)
}
pub(crate) fn helper_error(error: dualfile_core::appenv::HelperError) -> UiError {
    use dualfile_core::appenv::HelperError;
    match error {
        HelperError::Unavailable => mount_error("helperMissing"),
        HelperError::Timeout => mount_error("timeout"),
        HelperError::Failed(code) => mount_error(match code.as_str() {
            "busy" => "busy",
            "not found" => "notFound",
            "polkit dismissed" => "polkitDismissed",
            _ => "notPermitted",
        }),
    }
}
fn reconcile_mount_result(
    result: Result<(), dualfile_core::appenv::HelperError>,
    unmount: bool,
    read_mounted: impl FnOnce() -> Result<bool, UiError>,
) -> Result<(), UiError> {
    if result.is_ok() || matches!(result, Err(dualfile_core::appenv::HelperError::Timeout)) {
        if read_mounted()? != unmount {
            return Ok(());
        }
        if result.is_ok() {
            return Err(mount_error("busy"));
        }
    }
    result.map_err(helper_error)
}
pub(crate) fn fresh_table() -> Result<String, UiError> {
    bounded_read("/proc/self/mountinfo".into(), 4 * 1024 * 1024)
        .filter(|s| !s.is_empty() && s.len() < 4 * 1024 * 1024)
        .ok_or_else(|| mount_error("timeout"))
}
pub(crate) fn mounted(table: &str, point: &str) -> bool {
    table.lines().any(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.get(4).is_some_and(|v| drives::unescape(v) == point)
            && line
                .split_once(" - ")
                .is_some_and(|(_, tail)| !tail.starts_with("autofs "))
    })
}
fn authorize_fstab(text: &str, table: &str, point: &str, unmount: bool) -> Result<String, UiError> {
    use std::path::{Component, Path};
    let protected = [
        "/boot", "/efi", "/usr", "/etc", "/proc", "/sys", "/dev", "/bin", "/sbin", "/lib", "/lib64",
    ]
    .iter()
    .any(|root| Path::new(point).starts_with(root))
        || ["/", "/home", "/run", "/var"].contains(&point);
    if protected
        || !Path::new(point).is_absolute()
        || Path::new(point)
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(mount_error("notPermitted"));
    }
    let entry = fstab::parse_fstab(text)
        .into_iter()
        .find(|e| e.drive.path == point)
        .ok_or_else(|| mount_error("notFound"))?;
    if !entry.drive.fstab.is_some_and(|f| f.user_mountable) {
        return Err(mount_error("notPermitted"));
    }
    if mounted(table, point) != unmount {
        return Err(mount_error(if unmount { "notFound" } else { "busy" }));
    }
    Ok(entry.drive.path)
}
fn device_owned(device: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let path = if device.starts_with("/dev/") {
        std::path::PathBuf::from(device)
    } else if let Some(id) = device.strip_prefix("UUID=") {
        std::path::Path::new("/dev/disk/by-uuid").join(id)
    } else if let Some(id) = device.strip_prefix("LABEL=") {
        std::path::Path::new("/dev/disk/by-label").join(id)
    } else {
        return false;
    };
    if path
        .components()
        .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return false;
    }
    static POOL: OnceLock<places_pool::Pool> = OnceLock::new();
    POOL.get_or_init(|| places_pool::Pool::new(2))
        .run_keyed(
            path.to_string_lossy().into_owned(),
            Duration::from_millis(100),
            move || std::fs::metadata(path).is_ok_and(|m| m.uid() == unsafe { libc::geteuid() }),
        )
        .unwrap_or(false)
}
fn mounting_user_targets(table: &str, user: &str) -> Vec<String> {
    table
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let target = fields.iter().find_map(|f| f.strip_prefix("TARGET="))?;
            let options = fields.iter().find_map(|f| f.strip_prefix("OPTS="))?;
            options
                .split(',')
                .any(|o| {
                    o.strip_prefix("user=")
                        .is_some_and(|u| drives::unescape(u) == user)
                })
                .then(|| drives::unescape(target))
        })
        .collect()
}
fn current_user_mounts() -> Vec<String> {
    static POOL: OnceLock<places_pool::Pool> = OnceLock::new();
    POOL.get_or_init(|| places_pool::Pool::new(1))
        .run_keyed(
            "mount-user".into(),
            Duration::from_millis(100),
            current_user_mounts_inner,
        )
        .unwrap_or_default()
}
fn current_user_mounts_inner() -> Vec<String> {
    // util-linux stores mounting-user ownership in utab (not kernel mountinfo).
    // Resolve the account locally without trusting USER or emitting it in diagnostics.
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16384];
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || record.pw_name.is_null() {
        return Vec::new();
    }
    let user = unsafe { std::ffi::CStr::from_ptr(record.pw_name) }.to_string_lossy();
    mounting_user_targets(
        &bounded_read("/run/mount/utab".into(), 256 * 1024).unwrap_or_default(),
        &user,
    )
}
fn authorize_current_user(text: &str, point: &str, unmount: bool) -> Result<(), UiError> {
    let entry = fstab::parse_fstab(text)
        .into_iter()
        .find(|e| e.drive.path == point)
        .ok_or_else(|| mount_error("notFound"))?;
    authorize_current_user_entry(&entry, unmount)
}
pub(crate) fn authorize_current_user_entry(
    entry: &fstab::FstabEntry,
    unmount: bool,
) -> Result<(), UiError> {
    let drive = &entry.drive;
    let point = &drive.path;
    let allowed = fstab::permitted(
        drive.fstab.as_ref().and_then(|f| f.permission.as_deref()),
        unmount,
        drive.fstab.as_ref().and_then(|f| f.permission.as_deref()) == Some("owner")
            && device_owned(&drive.device),
        current_user_mounts().iter().any(|p| p == point),
    );
    if allowed {
        Ok(())
    } else {
        Err(mount_error("notPermitted"))
    }
}
fn fstab_action(point: &str, unmount: bool) -> Result<(), UiError> {
    let table = fresh_table()?;
    let fstab =
        bounded_read("/etc/fstab".into(), 256 * 1024).ok_or_else(|| mount_error("timeout"))?;
    if fstab::parse_fstab(&fstab).iter().any(|e| {
        e.drive.path == point
            && e.drive
                .fstab
                .as_ref()
                .is_some_and(|f| f.permission.is_none())
    }) {
        return crate::devices::fstab_action(point, unmount);
    }
    let point = authorize_fstab(&fstab, &table, point, unmount)?;
    authorize_current_user(&fstab, &point, unmount)?;
    // Hold admission across validation, the helper, and timeout reconciliation.
    let lease = Arc::new(crate::mount_guard::registry().unmount(point.clone().into())?);
    let table = fresh_table()?;
    let fstab =
        bounded_read("/etc/fstab".into(), 256 * 1024).ok_or_else(|| mount_error("timeout"))?;
    let point = authorize_fstab(&fstab, &table, &point, unmount)?;
    authorize_current_user(&fstab, &point, unmount)?;
    let mut command = std::process::Command::new(if unmount { "umount" } else { "mount" });
    command.arg("--").arg(&point);
    let result = dualfile_core::appenv::run_isolated_guarded(
        command,
        Duration::from_secs(60),
        lease.clone(),
    );
    reconcile_mount_result(result, unmount, || Ok(mounted(&fresh_table()?, &point)))
}
#[tauri::command]
pub async fn fstab_mount(window: tauri::WebviewWindow, mount_point: String) -> Result<(), UiError> {
    main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(move || fstab_action(&mount_point, false))
        .await
        .map_err(|_| UiError::internal())?
}
#[tauri::command]
pub async fn fstab_unmount(
    window: tauri::WebviewWindow,
    mount_point: String,
) -> Result<(), UiError> {
    main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(move || fstab_action(&mount_point, true))
        .await
        .map_err(|_| UiError::internal())?
}
fn fresh_mounts() -> Result<Vec<system_places::SystemPlace>, UiError> {
    let (mounts, complete) = system_places::probe_mounts(
        &system_places::DiscoveryPaths::system(),
        places_pool::mount_checks(),
        Duration::from_millis(750),
    );
    if !complete {
        return Err(mount_error("timeout"));
    }
    Ok(mounts)
}
fn unmount_gvfs(mount: system_places::SystemPlace) -> Result<(), UiError> {
    let point = mount
        .mounted_at
        .as_ref()
        .ok_or_else(|| mount_error("notFound"))?;
    let runtime = system_places::DiscoveryPaths::system().runtime;
    if !point.is_absolute() || !point.starts_with(runtime.join("gvfs")) {
        return Err(mount_error("notPermitted"));
    }
    let lease = Arc::new(crate::mount_guard::registry().unmount(point.clone())?);
    // Revalidate the opaque identity after taking the lease.
    if !fresh_mounts()?
        .iter()
        .any(|m| mount_key(m) == mount_key(&mount))
    {
        return Err(mount_error("notFound"));
    }
    let mut command = std::process::Command::new("gio");
    command.args(["mount", "-u", "--", &mount.url]);
    let result = dualfile_core::appenv::run_isolated_guarded(
        command,
        Duration::from_secs(30),
        lease.clone(),
    );
    reconcile_mount_result(result, true, || {
        Ok(fresh_mounts()?
            .iter()
            .any(|m| mount_key(m) == mount_key(&mount)))
    })
}

pub fn unmount_profile(url: &str) -> Result<(), UiError> {
    let profile = system_places::parse_url(url, "", "").ok_or_else(|| mount_error("notFound"))?;
    let mounts = fresh_mounts()?;
    let mount =
        system_places::matching_mount(&profile, &mounts).ok_or_else(|| mount_error("notFound"))?;
    unmount_gvfs(mount.clone())
}
#[tauri::command]
pub async fn places_mount_unmount(
    window: tauri::WebviewWindow,
    mount_key: String,
) -> Result<(), UiError> {
    main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mount = fresh_mounts()?
            .into_iter()
            .find(|m| self::mount_key(m) == mount_key)
            .ok_or_else(|| mount_error("notFound"))?;
        unmount_gvfs(mount)
    })
    .await
    .map_err(|_| UiError::internal())?
}

fn strip_account(place: &mut system_places::SystemPlace) {
    place.user = None;
    if let Some((scheme, rest)) = place.url.split_once("://") {
        let boundary = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = rest.split_at(boundary);
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        place.url = format!("{scheme}://{host}{path}");
    }
}
fn import_source(source: &str, fstype: &str) -> Option<system_places::SystemPlace> {
    let url = match fstype {
        "cifs" | "smb3" => format!("smb:{}", source),
        "nfs" | "nfs4" => {
            let (host, path) = source.rsplit_once(":/")?;
            format!("nfs://{host}/{path}")
        }
        "fuse.sshfs" => {
            let (host, path) = source.rsplit_once(":/")?;
            format!("sftp://{host}/{path}")
        }
        "davfs" | "davfs2" => source
            .replacen("https://", "davs://", 1)
            .replacen("http://", "dav://", 1),
        _ => return None,
    };
    let mut place = system_places::parse_url(&url, "mounted", "")?;
    strip_account(&mut place);
    Some(place)
}
/// Resolve saved connection data internally; credentials never reach the request or response.
pub fn importable_place(key: &str) -> Result<system_places::SystemPlace, UiError> {
    if let Some(url) = key.strip_prefix("system:") {
        // A desktop bookmark keeps its own title and account name, as in the import dialog.
        // It is looked up again by URL so the request can only name an existing bookmark.
        return bounded_bookmarks()
            .ok_or_else(|| mount_error("timeout"))?
            .into_iter()
            .find(|place| place.url == url)
            .ok_or_else(|| mount_error("notFound"));
    }
    let mut place = if let Some(key) = key.strip_prefix("mount:") {
        fresh_mounts()?
            .into_iter()
            .find(|m| mount_key(m) == key)
            .ok_or_else(|| mount_error("notFound"))?
    } else {
        let (kind, point) = key.split_once(':').ok_or_else(|| mount_error("notFound"))?;
        let mut found = None;
        if kind == "drive" {
            for line in fresh_table()?.lines() {
                let Some((head, tail)) = line.split_once(" - ") else {
                    continue;
                };
                let head: Vec<_> = head.split_whitespace().collect();
                let tail: Vec<_> = tail.split_whitespace().collect();
                if head.get(4).is_some_and(|p| drives::unescape(p) == point) && tail.len() >= 2 {
                    found = import_source(&drives::unescape(tail[1]), tail[0]);
                }
            }
        } else if kind == "fstab" {
            let text = bounded_read("/etc/fstab".into(), 256 * 1024)
                .ok_or_else(|| mount_error("timeout"))?;
            for line in text.lines() {
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() >= 4 && !f[0].starts_with('#') && drives::unescape(f[1]) == point {
                    found = import_source(&drives::unescape(f[0]), f[2]);
                }
            }
        }
        found.ok_or_else(|| mount_error("notFound"))?
    };
    strip_account(&mut place);
    place.name = format!("{}{}", place.host, place.path);
    place.mounted_at = None;
    Ok(place)
}

#[tauri::command]
pub async fn places_mount_affected(
    window: tauri::WebviewWindow,
    mount_key: String,
    paths: Vec<String>,
) -> Result<Vec<usize>, UiError> {
    main_only(window.label())?;
    if paths.len() > 1000 || paths.iter().any(|p| p.len() > 16384) {
        return Err(mount_error("notPermitted"));
    }
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(key) = mount_key.strip_prefix("device:") {
            return crate::devices::affected(key, false, &paths);
        }
        if let Some(key) = mount_key.strip_prefix("whole-device:") {
            return crate::devices::affected(key, true, &paths);
        }
        let point = if let Some(point) = mount_key.strip_prefix("path:") {
            if mounted(&fresh_table()?, point) {
                std::path::PathBuf::from(point)
            } else {
                fresh_mounts()?
                    .into_iter()
                    .filter_map(|m| m.mounted_at)
                    .filter(|p| std::path::Path::new(point).starts_with(p))
                    .max_by_key(|p| p.components().count())
                    .ok_or_else(|| mount_error("notFound"))?
            }
        } else {
            fresh_mounts()?
                .into_iter()
                .find(|m| self::mount_key(m) == mount_key)
                .and_then(|m| m.mounted_at)
                .ok_or_else(|| mount_error("notFound"))?
        };
        crate::mount_guard::affected_paths(&point, &paths)
    })
    .await
    .map_err(|_| UiError::internal())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_watch_filters_noise_and_rearms_only_after_loss() {
        use notify_debouncer_mini::notify::{
            event::{CreateKind, RemoveKind},
            Event, EventKind,
        };
        let event =
            |path: &str| Ok(Event::new(EventKind::Create(CreateKind::Any)).add_path(path.into()));
        assert_eq!(watch_event(&event("/dev/tty99")), (false, false));
        assert_eq!(
            watch_event(&event("/dev/disk/by-uuid/alias")),
            (false, false)
        );
        assert_eq!(watch_event(&event("/etc/unrelated")), (false, false));
        assert_eq!(watch_event(&event("/etc/fstab")), (true, false));
        assert_eq!(
            watch_event(&event("/dev/disk/by-id/usb-example")),
            (true, false)
        );
        for file in bookmark_files() {
            assert_eq!(watch_event(&event(file.to_str().unwrap())), (true, false));
            // Other files in the same busy folder must not wake discovery.
            let noise = file.with_file_name("recently-used.xbel");
            assert_eq!(watch_event(&event(noise.to_str().unwrap())), (false, false));
        }
        let removed =
            Ok(Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(DEVICE_WATCH.into()));
        assert_eq!(watch_event(&removed), (true, true));
        let lost = AtomicBool::new(true);
        rearm_device_watch(&lost, || false);
        assert!(lost.load(Ordering::Acquire));
        rearm_device_watch(&lost, || true);
        assert!(!lost.load(Ordering::Acquire));
        rearm_device_watch(&lost, || panic!("healthy watch must not be rearmed"));
        lost.store(true, Ordering::Release);
        rearm_device_watch(&lost, || {
            lost.store(true, Ordering::Release);
            true
        });
        assert!(lost.load(Ordering::Acquire));
    }
    use super::*;

    #[test]
    fn sidebar_never_resizes_a_hidden_maximized_or_fullscreen_window() {
        assert!(!geometry_fixed(true, false, false));
        // Hidden start (tray, or waiting for the task-bar fallback): the saved size must survive.
        assert!(geometry_fixed(false, false, false));
        assert!(geometry_fixed(true, true, false));
        assert!(geometry_fixed(true, false, true));
    }
    #[test]
    fn user_resize_during_settle_does_not_earn_sidebar_growth_credit() {
        assert_eq!(confirmed_resize_delta(1200, 1410, 1410, 1), 210.0);
        // Work-area clamping still permits confirmed partial growth.
        assert_eq!(confirmed_resize_delta(1200, 1300, 1300, 1), 100.0);
        assert_eq!(confirmed_resize_delta(1200, 1410, 1200, 1), 0.0);
        assert_eq!(confirmed_resize_delta(1200, 1410, 1500, 1), 0.0);
        assert_eq!(confirmed_resize_delta(1200, 990, 1300, 1), 0.0);
    }
    #[test]
    fn fractional_scaling_rounding_and_short_moves_still_earn_credit() {
        // 125 %/150 % scaling: the compositor rounds to logical pixels.
        assert_eq!(confirmed_resize_delta(1200, 1463, 1462, 2), 263.0);
        assert_eq!(confirmed_resize_delta(1200, 1463, 1465, 2), 263.0);
        assert_eq!(confirmed_resize_delta(1463, 1200, 1201, 2), -263.0);
        // Stopped short (clamped or late): count what actually happened, so hiding undoes it exactly.
        assert_eq!(confirmed_resize_delta(1200, 1410, 1350, 1), 150.0);
        assert_eq!(confirmed_resize_delta(1200, 990, 1100, 1), -100.0);
    }
    #[test]
    fn zero_delta_and_unknown_restore_never_touch_window_geometry() {
        let saved = Mutex::new(Some(Some(210.0)));
        let zero = tauri::async_runtime::block_on(resize_transaction(0.0, &saved, |_, _| async {
            panic!("zero delta must not query, resize, sleep or reposition the window")
        }))
        .unwrap();
        assert_eq!(zero.actual_delta, 0.0);
        assert_eq!(*saved.lock().unwrap(), Some(Some(210.0)));
        let failed_read = Mutex::new(Some(None));
        let unknown =
            tauri::async_runtime::block_on(resize_transaction(210.0, &failed_read, |_, _| async {
                panic!("failed restore read must skip the first grow")
            }))
            .unwrap();
        assert_eq!(unknown.actual_delta, 0.0);
        assert_eq!(*failed_read.lock().unwrap(), None);
        let restored =
            tauri::async_runtime::block_on(resize_transaction(210.0, &saved, |_, _| async {
                panic!("restored width must not grow again")
            }))
            .unwrap();
        assert_eq!(restored.restored_width, 210.0);
    }
    #[test]
    fn monitor_loss_retries_until_rearmed_and_polling_requires_visibility() {
        let mut file = Some(1);
        let mut last = Duration::ZERO;
        retry_monitor(&mut file, &mut last, Duration::from_secs(5), || {
            panic!("already armed")
        });
        file = None;
        retry_monitor(&mut file, &mut last, Duration::from_secs(5), || None);
        retry_monitor(&mut file, &mut last, Duration::from_secs(9), || {
            panic!("retry too early")
        });
        retry_monitor(&mut file, &mut last, Duration::from_secs(10), || Some(2));
        assert_eq!(file, Some(2));
        assert!(!poll_due(false, Duration::from_secs(100)));
        assert!(!poll_due(true, Duration::from_secs(4)));
        assert!(poll_due(true, Duration::from_secs(5)));
    }
    #[test]
    fn zero_partial_refused_and_restored_geometry() {
        let zero = resize_result(0.0, 0.0, false, 0.0);
        assert_eq!(zero.actual_delta, 0.0);
        assert!(!zero.can_grow);
        let partial = resize_result(210.0, 100.0, true, 0.0);
        assert_eq!(partial.actual_delta, 100.0);
        assert!(!partial.can_grow);
        assert!(!resize_result(210.0, 0.0, true, 0.0).can_grow);
        assert!(resize_result(210.0, 210.0, true, 0.0).can_grow);
        assert!(!resize_result(210.0, 210.0, false, 0.0).can_grow);
        assert_eq!(initial_resize(210.0, Some(None)), (0.0, 0.0));
        assert_eq!(initial_resize(210.0, Some(Some(210.0))), (0.0, 210.0));
        assert_eq!(initial_resize(210.0, Some(Some(0.0))), (210.0, 0.0));
        assert_eq!(initial_resize(-44.0, None), (-44.0, 0.0));
    }
    #[test]
    fn resize_clamps_to_work_area_and_minimum() {
        assert_eq!(resized_width(1280, 210, 1400, 900), 1400);
        assert_eq!(resized_width(950, -210, 1920, 900), 900);
    }
    #[test]
    fn commands_reject_secondary_windows() {
        for label in ["settings", "connection-editor"] {
            assert!(main_only(label).is_err());
        }
    }
    #[test]
    fn only_known_local_filesystems_are_measured() {
        for fs in ["autofs", "nfs4", "cifs", "fuse.sshfs", "9p", "unknown"] {
            assert!(!drives::local_space_type(fs));
        }
    }
    #[test]
    fn credential_bearing_mount_paths_are_not_public() {
        let mut p = system_places::parse_url("smb://alice@host/share", "mounted", "Share").unwrap();
        p.mounted_at = Some("/run/user/1000/gvfs/smb-share:server=host,user=alice".into());
        let p = public_mount(p).unwrap();
        assert_eq!(p.name, "host/share");
        assert!(p.mounted_at.is_none());
        assert!(!format!("{p:?}").contains("alice"));
        assert!(!serde_json::to_string(&p).unwrap().contains("alice"));
    }
}

#[cfg(test)]
mod space_budget_tests {
    use super::*;
    struct Slow;
    impl SpaceProbe for Slow {
        fn measure(&self, _: &str) -> Option<SpaceFree> {
            std::thread::sleep(Duration::from_millis(250));
            Some(SpaceFree {
                available: 1,
                total: 2,
            })
        }
    }
    #[test]
    fn hung_network_space_does_not_starve_local_space() {
        let (release, blocked) = std::sync::mpsc::channel();
        places_pool::network_space().run_keyed(
            "test-network-space".into(),
            Duration::from_millis(20),
            move || blocked.recv().unwrap(),
        );
        struct Fast;
        impl SpaceProbe for Fast {
            fn measure(&self, _: &str) -> Option<SpaceFree> {
                Some(SpaceFree {
                    available: 10,
                    total: 20,
                })
            }
        }
        let mut drives = vec![drives::Drive::new("/local".into(), "ext4", "/dev/a", "rw")];
        fill_space(
            &mut drives,
            places_pool::space(),
            Arc::new(Fast),
            Duration::from_secs(1),
            Duration::from_millis(100),
        );
        release.send(()).unwrap();
        assert_eq!(
            drives[0].space,
            Some(SpaceFree {
                available: 10,
                total: 20
            })
        );
    }
    #[test]
    fn slow_statvfs_cannot_exceed_response_budget_or_spawn_replacements() {
        let mut drives = vec![
            drives::Drive::new("/local".into(), "ext4", "/dev/a", "rw"),
            drives::Drive::new("/net".into(), "nfs", "host:/", "rw"),
        ];
        let pool = places_pool::Pool::new(1);
        let start = Instant::now();
        fill_space(
            &mut drives,
            &pool,
            Arc::new(Slow),
            Duration::from_millis(50),
            Duration::from_millis(30),
        );
        assert!(start.elapsed() < Duration::from_millis(200));
        assert!(drives.iter().all(|d| d.space.is_none()));
    }
}

#[cfg(test)]
mod restored_geometry_tests {
    use super::*;
    #[test]
    fn upgrades_add_width_once_and_existing_settings_preserve_restored_size() {
        assert_eq!(
            restored_sidebar_width(&serde_json::json!({"version":6})),
            0.0
        );
        assert_eq!(
            restored_sidebar_width(&serde_json::json!({"sidebar_mode":"full","sidebar_width":260})),
            260.0
        );
        assert_eq!(
            restored_sidebar_width(&serde_json::json!({"sidebar_mode":"icons"})),
            44.0
        );
        assert_eq!(
            restored_sidebar_width(&serde_json::json!({"sidebar_mode":"hidden"})),
            0.0
        );
    }
}

#[cfg(test)]
mod watcher_tests {
    use super::*;
    fn snapshot(table: &str) -> Snapshot {
        Snapshot {
            home: "/home/test".into(),
            user_dirs: vec![],
            trash: Trash {
                path: "/trash".into(),
                empty: true,
            },
            drives: drives::parse_mountinfo(table, None),
            network_mounts: vec![],
            system_places: vec![],
            discovered_services: vec![],
        }
    }
    #[test]
    fn unmounted_device_changes_are_debounced_and_diffed() {
        let mut diff = WatchDiff::default();
        let mut s = snapshot("");
        assert!(diff.publish(s.clone()));
        let devices = dualfile_core::devices::parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdb","maj:min":"8:16","type":"disk","fstype":"vfat","rm":true}]}"#).unwrap();
        dualfile_core::devices::merge(&mut s.drives, &devices, "", &[], crate::devices::key);
        diff.dirty(Duration::ZERO);
        diff.dirty(Duration::from_millis(50));
        assert!(!diff.due(Duration::from_millis(300)));
        assert!(diff.due(Duration::from_millis(350)));
        assert!(diff.publish(s.clone()));
        assert!(!diff.publish(s));
        assert!(diff.publish(snapshot("")));
    }
    #[test]
    fn first_snapshot_waits_for_inflight_refresh_and_first_publish_notifies() {
        let state = Places::default();
        let refreshing = state.refreshing.lock().unwrap();
        assert!(state.refresh().is_none());
        let expected = snapshot("1 0 8:1 / / rw - ext4 /dev/a rw");
        std::thread::scope(|scope| {
            let (done, result) = std::sync::mpsc::channel();
            let state = &state;
            scope.spawn(move || {
                done.send(state.cached_snapshot(Duration::from_secs(1)))
                    .unwrap()
            });
            assert_eq!(
                result.recv_timeout(Duration::from_millis(20)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            );
            state.cache_snapshot(expected.clone());
            assert_eq!(
                result.recv_timeout(Duration::from_secs(1)).unwrap(),
                Some(expected.clone())
            );
        });
        drop(refreshing);
        let mut diff = WatchDiff::default();
        assert!(diff.publish(expected.clone()));
        assert!(!diff.publish(expected));
        let empty = Places::default();
        let now = Instant::now();
        assert_eq!(empty.cached_snapshot(Duration::from_millis(20)), None);
        assert!(now.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn injected_mount_tables_and_clock_emit_once_per_visible_burst() {
        let mut diff = WatchDiff::default();
        let base = "1 0 8:1 / / rw - ext4 /dev/a rw";
        assert!(diff.publish(snapshot(base)));
        diff.dirty(Duration::from_millis(0));
        diff.dirty(Duration::from_millis(100));
        assert!(!diff.due(Duration::from_millis(399)));
        assert!(diff.due(Duration::from_millis(400)));
        assert!(!diff.due(Duration::from_millis(500)));
        assert!(!diff.publish(snapshot(&format!(
            "{base}\n2 1 0:1 / /container rw - overlay overlay rw"
        ))));
        assert!(diff.publish(snapshot(&format!(
            "{base}\n3 1 8:2 / /media/USB rw - vfat /dev/b rw"
        ))));
        assert!(!diff.publish(snapshot(&format!(
            "{base}\n3 1 8:2 / /media/USB rw - vfat /dev/b rw"
        ))));
    }
    #[test]
    fn gvfs_poll_diff_detects_changes_inside_existing_runtime_mount() {
        let mut diff = WatchDiff::default();
        let mut s = snapshot("");
        assert!(diff.publish(s.clone()));
        let mut mount = system_places::parse_url("smb://host/share", "mounted", "Share").unwrap();
        mount.mounted_at = Some("/run/user/1000/gvfs/smb-share:server=host,share=share".into());
        s.network_mounts.push(public_mount(mount).unwrap());
        assert!(diff.publish(s.clone()));
        assert!(!diff.publish(s.clone()));
        s.network_mounts.clear();
        assert!(diff.publish(s));
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    #[test]
    fn account_redaction_preserves_at_signs_in_url_paths() {
        for authority in ["host", "alice@host", "alice:p@ss@host"] {
            let raw = format!("davs://{authority}/remote.php/dav/files/me@mail.de/");
            let mut place = system_places::parse_url(&raw, "mounted", "").unwrap();
            // GVfs discovery may return literal @ characters, not URL-encoded ones.
            place.url = raw;
            place.mounted_at = Some("/run/user/1000/gvfs/dav:host=host".into());
            let public = public_mount(place.clone()).unwrap();
            strip_account(&mut place);
            assert_eq!(place.url, "davs://host/remote.php/dav/files/me@mail.de/");
            assert_eq!(public.url, place.url);
            assert_eq!(public.host, "host");
            assert_eq!(public.user, None);
            assert!(!format!("{public:?}").contains("alice"));
        }
    }
    #[test]
    fn utab_user_ownership_is_exact_and_stays_private() {
        let text = "SRC=/dev/a TARGET=/mnt/one ROOT=/ OPTS=rw,user=alice\nSRC=/dev/b TARGET=/mnt/two ROOT=/ OPTS=rw,user=bob\n";
        assert_eq!(mounting_user_targets(text, "alice"), vec!["/mnt/one"]);
        assert!(mounting_user_targets(text, "ali").is_empty());
        assert!(!device_owned("//alice:secret@host/share"));
    }
    #[test]
    fn timeout_rechecks_actual_state_and_never_assumes_success() {
        use dualfile_core::appenv::HelperError;
        for unmount in [false, true] {
            let mut read = false;
            assert!(
                reconcile_mount_result(Err(HelperError::Timeout), unmount, || {
                    read = true;
                    Ok(!unmount)
                })
                .is_ok()
            );
            assert!(read);
            assert_eq!(
                reconcile_mount_result(Err(HelperError::Timeout), unmount, || Ok(unmount))
                    .unwrap_err()
                    .kind,
                "timeout"
            );
            assert_eq!(
                reconcile_mount_result(Ok(()), unmount, || Ok(unmount))
                    .unwrap_err()
                    .kind,
                "busy"
            );
            assert_eq!(
                reconcile_mount_result(Err(HelperError::Unavailable), unmount, || panic!(
                    "missing helper did not run"
                ))
                .unwrap_err()
                .kind,
                "helperMissing"
            );
        }
    }
    #[test]
    fn import_strips_all_userinfo_from_davfs_and_sshfs() {
        for (source, fs, url) in [
            (
                "https://alice:p@ss@host/folder",
                "davfs",
                "davs://host/folder",
            ),
            (
                "alice:p@ss@host:/folder",
                "fuse.sshfs",
                "sftp://host/folder",
            ),
        ] {
            let place = import_source(source, fs).unwrap();
            assert_eq!(place.url, url);
            assert_eq!(place.user, None);
            let public = format!("{place:?} {}", serde_json::to_string(&place).unwrap());
            assert!(!public.contains("alice") && !public.contains("p@ss"));
        }
    }
    #[test]
    fn network_sources_are_resolved_without_fstab_options() {
        for (source, fs, url) in [
            ("//host/share", "cifs", "smb://host/share"),
            ("host:/export", "nfs4", "nfs://host/export"),
            ("host:/data", "fuse.sshfs", "sftp://host/data"),
        ] {
            assert_eq!(import_source(source, fs).unwrap().url, url);
        }
        assert!(import_source("/dev/sda1", "ext4").is_none());
    }
    #[test]
    fn fstab_authorization_matrix_and_safe_errors() {
        let table = "1 0 0:1 / /mnt/share rw - nfs host:/private rw\n";
        for permission in ["user", "users", "owner"] {
            let text = format!("//alice:secret@host/share /mnt/share cifs {permission},credentials=/secret,username=alice");
            assert!(authorize_fstab(&text, "", "/mnt/share", false).is_ok());
            assert!(authorize_fstab(&text, table, "/mnt/share", true).is_ok());
            assert!(authorize_fstab(&text, table, "/mnt/share", false).is_err());
            assert!(authorize_fstab(&text, "", "/mnt/share", true).is_err());
        }
        for (text, point) in [
            ("a / ext4 user", "/"),
            ("a /boot ext4 user", "/boot"),
            ("a /mnt/share nfs defaults", "/mnt/share"),
            ("a /mnt/share nfs user", "/forged"),
            ("a /a/../b nfs user", "/a/../b"),
        ] {
            assert!(authorize_fstab(text, table, point, false).is_err());
        }
        let error = helper_error(dualfile_core::appenv::HelperError::Failed(
            "credentials=/secret username=alice smb://alice:secret@host".into(),
        ));
        let public = serde_json::to_string(&error).unwrap();
        assert!(!public.contains("alice") && !public.contains("secret"));
    }
}
