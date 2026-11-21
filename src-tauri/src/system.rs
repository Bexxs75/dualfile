//! Native-only system integration; network traffic stays in the updater plugin.
use crate::{
    autostart::{self, Autostart},
    desktop_entry::{self, DesktopEntry},
    state::AppState,
};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{
    ipc::Channel,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::watch;

// Serializes entering installation against all job-submitting IPC calls. No guard is
// held over await; commands fail while installation owns the gate.
pub static INSTALLING: RwLock<bool> = RwLock::new(false);
// The sender exists only while checking/downloading. Removing it under this
// mutex commits to installation, atomically excluding a late cancellation.
static DOWNLOAD_CANCEL: Mutex<Option<watch::Sender<bool>>> = Mutex::new(None);
struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        *DOWNLOAD_CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *INSTALLING.write().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    app_image: bool,
    start_menu_exists: bool,
    /// Why the start-menu entry could not be read or refreshed.
    start_menu_error: Option<String>,
    autostart: autostart::Mode,
    /// Why the autostart entry could not be read or refreshed; the mode is then what is on disk.
    autostart_error: Option<String>,
}
#[derive(Serialize)]
pub struct AvailableUpdate {
    version: String,
    notes: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
    phase: &'static str,
}

fn desktop() -> io::Result<DesktopEntry> {
    DesktopEntry::new(desktop_entry::home()?)
}
fn app_image() -> bool {
    std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty())
}

/// Refreshes a stale Exec path first (when the executable is known), then reads the entry.
fn start_menu_status(
    desktop: io::Result<DesktopEntry>,
    exe: Option<&Path>,
) -> (bool, Option<String>) {
    let desktop = match desktop {
        Ok(desktop) => desktop,
        Err(e) => return (false, Some(e.to_string())),
    };
    let refreshed = exe.map_or(Ok(()), |exe| desktop.refresh(exe));
    match (desktop.exists(), refreshed) {
        (Ok(exists), Ok(())) => (exists, None),
        (Ok(exists), Err(e)) => (exists, Some(e.to_string())),
        (Err(e), _) => (false, Some(e.to_string())),
    }
}
fn autostart_status(
    entry: io::Result<Autostart>,
    exe: Option<&Path>,
) -> (autostart::Mode, Option<String>) {
    let entry = match entry {
        Ok(entry) => entry,
        Err(e) => return (autostart::Mode::Off, Some(e.to_string())),
    };
    let refreshed = exe.map_or(Ok(()), |exe| entry.refresh(exe));
    match (entry.mode(), refreshed) {
        (Ok(mode), Ok(())) => (mode, None),
        (Ok(mode), Err(e)) => (mode, Some(e.to_string())),
        (Err(e), _) => (autostart::Mode::Off, Some(e.to_string())),
    }
}
/// The two entries are independent: a failure of one must not hide or block the other.
fn integration_status(
    desktop: io::Result<DesktopEntry>,
    autostart: io::Result<Autostart>,
    exe: io::Result<PathBuf>,
    app_image: bool,
) -> SystemStatus {
    let exe_error = exe.as_ref().err().map(ToString::to_string);
    let exe = exe.as_deref().ok();
    let (start_menu_exists, start_menu_error) = start_menu_status(desktop, exe);
    let (autostart, autostart_error) = autostart_status(autostart, exe);
    SystemStatus {
        app_image,
        start_menu_exists,
        start_menu_error: start_menu_error.or_else(|| exe_error.clone()),
        autostart,
        autostart_error: autostart_error.or(exe_error),
    }
}

#[tauri::command]
pub async fn system_status() -> Result<SystemStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let status = integration_status(
            desktop(),
            Autostart::from_env(),
            desktop_entry::executable(),
            app_image(),
        );
        if let Some(error) = &status.start_menu_error {
            log::warn!("Startmenü-Eintrag: {error}");
        }
        if let Some(error) = &status.autostart_error {
            log::warn!("Autostart-Eintrag: {error}");
        }
        status
    })
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn start_menu_set(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let d = desktop().map_err(|e| e.to_string())?;
        if enabled {
            d.install(&desktop_entry::executable().map_err(|e| e.to_string())?)
        } else {
            d.remove()
        }
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes or removes the autostart entry and answers with what is on disk afterwards.
#[tauri::command]
pub async fn autostart_set(mode: autostart::Mode) -> Result<autostart::Mode, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = Autostart::from_env().map_err(|e| e.to_string())?;
        entry
            .set(
                mode,
                &desktop_entry::executable().map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        entry.mode().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Set while a minimized start keeps the main window hidden until tray icon and host exist.
static START_HIDDEN: AtomicBool = AtomicBool::new(false);
/// True only after the session bus confirmed a StatusNotifier host. Unchecked counts as absent:
/// an icon nobody displays must not swallow the window on minimize or close.
static TRAY_HOST: AtomicBool = AtomicBool::new(false);
/// One host watch at a time (`await_tray`).
static TRAY_WATCH: AtomicBool = AtomicBool::new(false);
/// At login the panel may register after Dualfile starts; wait this long before giving up.
const START_HIDDEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Hiding to the tray is only safe with an icon that some panel really shows.
pub(crate) fn tray_usable(app: &tauri::AppHandle) -> bool {
    may_hide_to_tray(app.tray_by_id("dualfile").is_some())
}
fn may_hide_to_tray(icon: bool) -> bool {
    icon && TRAY_HOST.load(Ordering::SeqCst)
}
/// The window was fetched (second launch, tray click). This ends a hidden start only; the
/// host watch goes on, so its result never depends on who showed the window.
fn window_fetched() {
    START_HIDDEN.store(false, Ordering::SeqCst);
}

/// A watcher without a registered host (no panel applet) displays nothing.
fn host_registered(watcher_owned: bool, registered: Option<bool>) -> bool {
    watcher_owned && registered == Some(true)
}

/// tray-icon reports success without any panel showing the icon, so ask the session bus.
#[cfg(target_os = "linux")]
async fn tray_host_present() -> bool {
    const WATCHER: &str = "org.kde.StatusNotifierWatcher";
    let query = async {
        let bus = zbus::Connection::session().await?;
        let owned: bool = bus
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(WATCHER,),
            )
            .await?
            .body()
            .deserialize()?;
        if !owned {
            return Ok(false);
        }
        let value: zbus::zvariant::OwnedValue = bus
            .call_method(
                Some(WATCHER),
                "/StatusNotifierWatcher",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(WATCHER, "IsStatusNotifierHostRegistered"),
            )
            .await?
            .body()
            .deserialize()?;
        Ok::<_, zbus::Error>(host_registered(owned, bool::try_from(value).ok()))
    };
    tokio::time::timeout(Duration::from_secs(1), query)
        .await
        .is_ok_and(|found| found.unwrap_or(false))
}
#[cfg(not(target_os = "linux"))]
async fn tray_host_present() -> bool {
    true
}

#[derive(Debug, PartialEq)]
enum HiddenStart {
    Wait,
    /// Icon built and a host shows it: the window stays in the tray.
    Tray,
    /// No usable tray in time: minimized in the task bar instead.
    Taskbar,
}
fn hidden_start(host: bool, icon: bool, waited: Duration) -> HiddenStart {
    if host && icon {
        HiddenStart::Tray
    } else if waited >= START_HIDDEN_TIMEOUT {
        HiddenStart::Taskbar
    } else {
        HiddenStart::Wait
    }
}

/// Watches for icon and host until both exist or the time is up, and settles a hidden start.
/// Also run after every successful tray build, so `TRAY_HOST` is confirmed on normal starts.
fn await_tray(w: tauri::WebviewWindow) {
    if TRAY_WATCH.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let started = Instant::now();
        loop {
            let host = runtime
                .as_ref()
                .is_ok_and(|runtime| runtime.block_on(tray_host_present()));
            TRAY_HOST.store(host, Ordering::SeqCst);
            let icon = w.app_handle().tray_by_id("dualfile").is_some();
            let waited = if runtime.is_ok() {
                started.elapsed()
            } else {
                START_HIDDEN_TIMEOUT
            };
            match hidden_start(host, icon, waited) {
                HiddenStart::Wait => std::thread::sleep(Duration::from_millis(500)),
                HiddenStart::Tray => {
                    START_HIDDEN.store(false, Ordering::SeqCst);
                    break;
                }
                HiddenStart::Taskbar => {
                    if START_HIDDEN.swap(false, Ordering::SeqCst) {
                        show_minimized(&w);
                    }
                    break;
                }
            }
        }
        TRAY_WATCH.store(false, Ordering::SeqCst);
    });
}

/// The main window is created hidden (tauri.conf.json), so a minimized start never flashes it
/// up. With the tray setting on it stays hidden until the icon is built and a panel shows it;
/// without a usable tray it goes to the task bar.
pub(crate) fn show_at_start(w: &tauri::WebviewWindow, minimized: bool, tray: bool) {
    if !minimized {
        let _ = w.show();
        let _ = w.set_focus();
    } else if tray {
        START_HIDDEN.store(true, Ordering::SeqCst);
        await_tray(w.clone());
    } else {
        show_minimized(w);
    }
}

/// Iconified before it is mapped, the window is never drawn on X11. Wayland compositors only
/// take the request for a mapped window, hence the second call.
fn show_minimized(w: &tauri::WebviewWindow) {
    let _ = w.minimize();
    let _ = w.show();
    let _ = w.minimize();
}

/// Brings the main window to the front, also when it is minimized or behind other windows.
pub(crate) fn bring_to_front(w: &tauri::WebviewWindow) {
    window_fetched();
    // Linux: KWin and other window managers refuse present() without a user timestamp
    // (focus-stealing prevention), so a minimized or covered window stays put. Re-mapping
    // it counts as a new window, which the window manager raises and focuses.
    #[cfg(target_os = "linux")]
    if w.is_visible().unwrap_or(false) && !w.is_focused().unwrap_or(false) {
        let _ = w.hide();
    }
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
}

/// Set by the tray's Quit item: closing then really exits instead of hiding to the tray.
pub(crate) static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn tray_set(app: tauri::AppHandle, enabled: bool, language: String) -> Result<bool, String> {
    let removed = app.remove_tray_by_id("dualfile").is_some();
    if !enabled {
        if let Some(w) = app.get_webview_window("main") {
            if START_HIDDEN.swap(false, Ordering::SeqCst) {
                show_minimized(&w);
            } else if removed {
                // Only a window that sat in the tray is fetched; a start minimized to the
                // task bar (no tray) must stay there.
                let _ = w.show();
                let _ = w.unminimize();
            }
        }
        return Ok(true);
    }
    let de = language == "de";
    let result = (|| -> tauri::Result<()> {
        let open = MenuItem::with_id(
            &app,
            "open",
            if de { "Öffnen" } else { "Open" },
            true,
            None::<&str>,
        )?;
        let quit = MenuItem::with_id(
            &app,
            "quit",
            if de { "Beenden" } else { "Quit" },
            true,
            None::<&str>,
        )?;
        let menu = Menu::with_items(&app, &[&open, &quit])?;
        let mut builder = TrayIconBuilder::with_id("dualfile")
            .menu(&menu)
            .tooltip("Dualfile")
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| {
                if let Some(w) = app.get_webview_window("main") {
                    bring_to_front(&w);
                    // Reset on Open, so a cancelled quit (job confirmation) hides again later.
                    let quit = event.id.as_ref() == "quit";
                    QUIT_REQUESTED.store(quit, Ordering::SeqCst);
                    if quit {
                        let _ = w.close();
                    }
                }
            })
            .on_tray_icon_event(|tray, event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    if let Some(w) = tray.app_handle().get_webview_window("main") {
                        // Hide only a window that is really in front; otherwise fetch it.
                        let in_front = w.is_visible().unwrap_or(false)
                            && w.is_focused().unwrap_or(false)
                            && !w.is_minimized().unwrap_or(false);
                        if in_front {
                            let _ = w.hide();
                        } else {
                            bring_to_front(&w);
                        }
                    }
                }
            });
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        builder.build(&app)?;
        Ok(())
    })();
    if result.is_err() {
        log::warn!("Tray-Symbol nicht verfügbar");
        if let Some(w) = app.get_webview_window("main") {
            if START_HIDDEN.swap(false, Ordering::SeqCst) {
                show_minimized(&w);
            } else {
                let _ = w.show();
            }
        }
    } else if let Some(w) = app.get_webview_window("main") {
        await_tray(w);
    }
    Ok(result.is_ok())
}

#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let result: Result<Option<AvailableUpdate>, String> = async {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?;
        Ok(update.map(|u| AvailableUpdate {
            version: u.version,
            notes: u.body.unwrap_or_default(),
        }))
    }
    .await;
    if let Err(error) = &result {
        log::warn!("Update-Prüfung fehlgeschlagen: {error}");
    }
    result
}
#[tauri::command]
pub async fn update_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    version: String,
    progress: Channel<UpdateProgress>,
) -> Result<(), String> {
    log::info!("Update auf {version} wird geladen");
    let result = install_update(app, state, version, progress).await;
    match &result {
        Ok(()) => log::info!("Update heruntergeladen und eingespielt, Neustart folgt"),
        Err(error) => log::warn!("Update fehlgeschlagen: {error}"),
    }
    result
}

async fn install_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    version: String,
    progress: Channel<UpdateProgress>,
) -> Result<(), String> {
    if !app_image() {
        return Err("Self-update requires AppImage".into());
    }
    if state.active_jobs_count() != 0 {
        return Err("File jobs are active".into());
    }
    // Reject only a second concurrent update here; do NOT raise the file-operation gate yet.
    // The gate (INSTALLING) is raised just before the replace, so the panes keep listing
    // normally during the (large) download instead of erroring for its whole duration.
    {
        let downloading = DOWNLOAD_CANCEL
            .lock()
            .map_err(|_| "Update gate unavailable")?
            .is_some();
        let installing = *INSTALLING.read().map_err(|_| "Update gate unavailable")?;
        if downloading || installing {
            return Err("Update installation in progress".into());
        }
    }
    let guard = InstallGuard;
    let (cancel, mut cancelled) = watch::channel(false);
    *DOWNLOAD_CANCEL
        .lock()
        .map_err(|_| "Update cancellation unavailable")? = Some(cancel);
    let download = async {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Update is no longer available")?;
        if update.version != version {
            return Err("Available update changed; check again".to_string());
        }
        let mut downloaded = 0u64;
        let bytes = update
            .download(
                |chunk, total| {
                    downloaded += chunk as u64;
                    let _ = progress.send(UpdateProgress {
                        downloaded,
                        total,
                        phase: "download",
                    });
                },
                || {},
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok((update, bytes))
    };
    let (update, bytes) = tokio::select! {
        biased;
        _ = cancelled.changed() => return Err("Update cancelled".into()),
        result = download => result?,
    };
    {
        let mut cancellation = DOWNLOAD_CANCEL
            .lock()
            .map_err(|_| "Update cancellation unavailable")?;
        if *cancelled.borrow() {
            return Err("Update cancelled".into());
        }
        *cancellation = None;
    }
    let _ = progress.send(UpdateProgress {
        downloaded: 0,
        total: None,
        phase: "install",
    });
    // Download finished: now lock file operations for the actual replace and the short
    // window until the frontend triggers the restart. InstallGuard clears it on failure.
    // A file job may have started during the download: re-check under the gate's write lock so
    // none can be killed by the restart.
    {
        let mut gate = INSTALLING.write().map_err(|_| "Update gate unavailable")?;
        if state.active_jobs_count() != 0 {
            return Err("File jobs are active".into());
        }
        *gate = true;
    }
    update.install(bytes).map_err(|e| e.to_string())?;
    // Keep file operations locked until the frontend invokes the process plugin restart.
    std::mem::forget(guard);
    Ok(())
}
/// Restart after an update. Tauri's own restart starts the new AppImage with this process's
/// environment and working directory, which still point into the old AppImage mount: the new
/// runtime then loads the old bundled libraries (on newer systems it does not start at all) and
/// hands them on to every program Dualfile opens. Start it as the desktop would instead.
#[tauri::command]
pub fn app_restart(app: tauri::AppHandle) -> Result<(), String> {
    if app
        .get_webview_window(crate::settings_window::SETTINGS_LABEL)
        .is_some()
    {
        crate::settings_window::request_close(&app, crate::settings_window::AfterClose::Restart);
        return Ok(());
    }
    restart_now(app)
}
pub(crate) fn restart_now(app: tauri::AppHandle) -> Result<(), String> {
    let env = app.env();
    let path = tauri::process::current_binary(&env).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(path);
    // The autostart flag belongs to the login start only; a restart shows the window again.
    command.args(
        env.args_os
            .iter()
            .skip(1)
            .filter(|arg| *arg != autostart::MINIMIZED_FLAG),
    );
    dualfile_core::appenv::apply_clean_env(&mut command);
    dualfile_core::appenv::apply_external_current_dir(&mut command);
    // The process exits below without a close request, which is where the position is saved.
    crate::window_state::save(&app);
    // As Tauri's own restart does: release the single-instance lock first, or the new process
    // may find this one still running and quit as a second instance.
    app.cleanup_before_exit();
    command.spawn().map_err(|e| {
        log::error!("Neustart nach dem Update fehlgeschlagen: {e}");
        e.to_string()
    })?;
    std::process::exit(0)
}

#[tauri::command]
pub fn update_cancel() -> Result<bool, String> {
    let cancellation = DOWNLOAD_CANCEL
        .lock()
        .map_err(|_| "Update cancellation unavailable")?;
    Ok(cancellation
        .as_ref()
        .is_some_and(|sender| sender.send(true).is_ok()))
}

#[tauri::command]
pub async fn release_open() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut command = std::process::Command::new("xdg-open");
        dualfile_core::appenv::clean_env(
            std::env::vars_os(),
            std::env::var_os("APPDIR").as_deref(),
        )
        .apply(&mut command);
        let status = command
            .arg("https://github.com/Bexxs75/dualfile/releases/latest")
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("Could not open release page".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// GTK reports iconification directly; window resizing is not a minimize event.
///
/// X11 only in effect. On Wayland a window is minimized by the compositor (KWin draws the title
/// bar buttons) and xdg-shell has no minimized state, so GDK never sets ICONIFIED there
/// (`gdk_wayland_window_iconify` only sends the request) and this handler does not run: the
/// window stays a minimized task bar entry. The only hint a compositor gives is the xdg-shell
/// "suspended" state, which KWin also sets on a desktop switch or when the screens turn off,
/// and which the GTK bundled in the AppImage (3.24.38) does not pass on at all.
#[cfg(target_os = "linux")]
pub fn watch_minimize(app: &tauri::AppHandle) -> tauri::Result<()> {
    use gtk::prelude::*;
    if let Some(window) = app.get_webview_window("main") {
        let native = window.gtk_window()?;
        let handle = app.clone();
        native.connect_window_state_event(move |native, event| {
            if event
                .new_window_state()
                .contains(gtk::gdk::WindowState::ICONIFIED)
                && tray_usable(&handle)
            {
                if handle
                    .get_webview_window(crate::settings_window::SETTINGS_LABEL)
                    .is_some()
                {
                    if crate::settings_window::request_close(
                        &handle,
                        crate::settings_window::AfterClose::Hide,
                    ) {
                        if let Some(main) = handle.get_webview_window("main") {
                            bring_to_front(&main);
                        }
                    }
                } else {
                    native.hide();
                }
            }
            gtk::glib::Propagation::Proceed
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hidden_start_needs_icon_and_host_and_falls_back_to_the_task_bar() {
        let soon = Duration::from_secs(1);
        assert_eq!(hidden_start(true, true, soon), HiddenStart::Tray);
        assert_eq!(
            hidden_start(true, true, START_HIDDEN_TIMEOUT),
            HiddenStart::Tray
        );
        // tray-icon builds an icon without any panel: that alone must not keep the window hidden.
        assert_eq!(hidden_start(false, true, soon), HiddenStart::Wait);
        assert_eq!(hidden_start(true, false, soon), HiddenStart::Wait);
        assert_eq!(
            hidden_start(false, true, START_HIDDEN_TIMEOUT),
            HiddenStart::Taskbar
        );
        assert_eq!(
            hidden_start(true, false, START_HIDDEN_TIMEOUT),
            HiddenStart::Taskbar
        );
        assert_eq!(
            hidden_start(false, false, START_HIDDEN_TIMEOUT),
            HiddenStart::Taskbar
        );
    }

    #[test]
    fn second_launch_before_the_timeout_does_not_make_an_unconfirmed_tray_usable() {
        // Minimized start, tray icon built, no host confirmed yet.
        START_HIDDEN.store(true, Ordering::SeqCst);
        TRAY_HOST.store(false, Ordering::SeqCst);
        // A second launch fetches the window before the watch has finished.
        window_fetched();
        assert!(!START_HIDDEN.load(Ordering::SeqCst));
        // Minimize and close must not hide it: nobody shows the icon.
        assert!(!may_hide_to_tray(true));
        // The watch itself is not aborted by that: it keeps waiting, then gives up without a host.
        assert_eq!(
            hidden_start(false, true, Duration::from_secs(2)),
            HiddenStart::Wait
        );
        assert_eq!(
            hidden_start(false, true, START_HIDDEN_TIMEOUT),
            HiddenStart::Taskbar
        );
        assert!(!may_hide_to_tray(true));
        // Only a positive check makes hide-to-tray available, and never without an icon.
        TRAY_HOST.store(true, Ordering::SeqCst);
        assert!(may_hide_to_tray(true) && !may_hide_to_tray(false));
        TRAY_HOST.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_watcher_without_a_registered_host_is_no_tray() {
        assert!(host_registered(true, Some(true)));
        assert!(!host_registered(true, Some(false)));
        assert!(!host_registered(true, None));
        assert!(!host_registered(false, Some(true)));
    }

    #[cfg(unix)]
    #[test]
    fn start_menu_failure_neither_hides_nor_blocks_autostart() {
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        // A symlinked ~/.local makes the start-menu integration refuse to work.
        std::os::unix::fs::symlink(other.path(), home.path().join(".local")).unwrap();
        let autostart = Autostart::new(config.path());
        autostart
            .set(
                autostart::Mode::Minimized,
                Path::new("/old/Dualfile.AppImage"),
            )
            .unwrap();
        let status = integration_status(
            DesktopEntry::new(home.path().to_owned()),
            Ok(Autostart::new(config.path())),
            Ok(PathBuf::from("/new/Dualfile.AppImage")),
            true,
        );
        assert!(!status.start_menu_exists && status.start_menu_error.is_some());
        assert_eq!(status.autostart, autostart::Mode::Minimized);
        assert_eq!(status.autostart_error, None);
        let entry = fs::read_to_string(
            config
                .path()
                .join("autostart/com.dualfileapp.Dualfile.desktop"),
        )
        .unwrap();
        assert!(entry.contains("Exec=\"/new/Dualfile.AppImage\" --minimized\n"));
    }

    #[cfg(unix)]
    #[test]
    fn autostart_failure_leaves_the_start_menu_status_intact_and_errors_stay_separate() {
        let home = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let exe = Path::new("/tmp/Dualfile.AppImage");
        DesktopEntry::new(home.path().to_owned())
            .unwrap()
            .install(exe)
            .unwrap();
        let entry = config
            .path()
            .join("autostart/com.dualfileapp.Dualfile.desktop");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(config.path().join("elsewhere"), &entry).unwrap();
        let status = integration_status(
            DesktopEntry::new(home.path().to_owned()),
            Ok(Autostart::new(config.path())),
            Ok(exe.to_owned()),
            false,
        );
        assert!(status.start_menu_exists && status.start_menu_error.is_none());
        assert_eq!(status.autostart, autostart::Mode::Off);
        assert!(status.autostart_error.is_some());
        // Without a known executable nothing is refreshed, but both entries are still read.
        let unknown = integration_status(
            DesktopEntry::new(home.path().to_owned()),
            Err(io::Error::other("no home")),
            Err(io::Error::other("no executable")),
            false,
        );
        assert!(unknown.start_menu_exists);
        assert_eq!(unknown.start_menu_error.as_deref(), Some("no executable"));
        assert_eq!(unknown.autostart_error.as_deref(), Some("no home"));
    }
}
