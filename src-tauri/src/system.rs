//! Native-only system integration; network traffic stays in the updater plugin.
use crate::{
    desktop_entry::{self, DesktopEntry},
    state::AppState,
};
use serde::Serialize;
use std::sync::{Arc, Mutex, RwLock};
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

fn desktop() -> Result<DesktopEntry, String> {
    DesktopEntry::new(desktop_entry::home().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn app_image() -> bool {
    std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty())
}

#[tauri::command]
pub async fn system_status() -> Result<SystemStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let d = desktop()?;
        d.refresh(&desktop_entry::executable().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(SystemStatus {
            app_image: app_image(),
            start_menu_exists: d.exists().map_err(|e| e.to_string())?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn start_menu_set(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let d = desktop()?;
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

/// Set by the tray's Quit item: closing then really exits instead of hiding to the tray.
pub(crate) static QUIT_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub fn tray_set(app: tauri::AppHandle, enabled: bool, language: String) -> Result<bool, String> {
    app.remove_tray_by_id("dualfile");
    if !enabled {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
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
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                    // Reset on Open, so a cancelled quit (job confirmation) hides again later.
                    let quit = event.id.as_ref() == "quit";
                    QUIT_REQUESTED.store(quit, std::sync::atomic::Ordering::SeqCst);
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
                        if w.is_visible().unwrap_or(false) {
                            let _ = w.hide();
                        } else {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
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
        eprintln!("Dualfile: tray unavailable");
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
        }
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
    if result.is_err() {
        eprintln!("Dualfile: update check failed");
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
    if !app_image() {
        return Err("Self-update requires AppImage".into());
    }
    if state.active_jobs_count() != 0 {
        return Err("File jobs are active".into());
    }
    {
        let mut installing = INSTALLING.write().map_err(|_| "Update gate unavailable")?;
        if *installing || state.active_jobs_count() != 0 {
            return Err("File jobs are active".into());
        }
        *installing = true;
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
    update.install(bytes).map_err(|e| e.to_string())?;
    // Keep file operations locked until the frontend invokes the process plugin restart.
    std::mem::forget(guard);
    Ok(())
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
                && handle.tray_by_id("dualfile").is_some()
            {
                native.hide();
            }
            gtk::glib::Propagation::Proceed
        });
    }
    Ok(())
}
