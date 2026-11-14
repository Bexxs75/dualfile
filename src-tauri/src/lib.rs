//! Dualfile: Tauri shell around `dualfile-core`. The web UI reaches the file system only
//! through the commands in `commands.rs`; the core reports back through `core-event`.

mod close;
mod close_policy;
mod commands;
mod compare;
mod conflict;
mod connections;
mod convert;
mod desktop_entry;
mod item_error;
mod keychain;
mod lanes;
mod plans;
mod plugins;
mod regex_tools;
mod search;
mod similar_space;
mod space;
#[cfg(unix)]
mod ssh_prompt;
mod state;
mod system;
mod validate;
mod verification;
mod watch;

use convert::CoreEventDto;
use state::{AppState, EventSink};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, RunEvent};

/// Name of the event the UI listens to (`CoreEvent` in `src/ipc/types.ts`).
pub(crate) static CONFIRMED_CLOSE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub const CORE_EVENT: &str = "core-event";

struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: CoreEventDto) {
        // A failing emit means the window is gone; there is nobody left to tell.
        if matches!(event, CoreEventDto::UndoChanged) {
            let _ = self.0.emit("undo-changed", ());
        } else {
            let _ = self.0.emit(CORE_EVENT, event);
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn run() {
    // Wayland application id = start-menu entry name, so the compositor finds the
    // entry (and its icon) for the title bar as well as the task bar.
    #[cfg(target_os = "linux")]
    gtk::glib::set_prgname(Some("de.thebexxs.dualfile"));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Title bar / task bar icon (X11 and XWayland; Wayland desktops take
            // it from the start-menu entry that matches the application id).
            if let (Some(window), Some(icon)) =
                (app.get_webview_window("main"), app.default_window_icon())
            {
                let _ = window.set_icon(icon.clone());
            }
            let sink = Arc::new(TauriSink(app.handle().clone()));
            #[cfg(unix)]
            let state = {
                let bridge = Arc::new(ssh_prompt::SshPromptBridge::new(app.handle().clone()));
                app.manage(bridge.clone());
                AppState::with_remote(
                    sink,
                    home_dir(),
                    dualfile_core::settings::config_dir(),
                    connections::RemoteParts::production_with_prompt(Some(bridge)),
                )
            };
            #[cfg(not(unix))]
            let state = AppState::new(sink, home_dir(), dualfile_core::settings::config_dir());
            app.manage(Arc::new(state));
            #[cfg(target_os = "linux")]
            system::watch_minimize(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if *system::INSTALLING.read().unwrap_or_else(|e| e.into_inner()) {
                    api.prevent_close();
                    return;
                }
                let count = window
                    .try_state::<Arc<AppState>>()
                    .map_or(0, |state| state.active_jobs_count());
                use std::sync::atomic::Ordering;
                match close::decide(
                    count,
                    CONFIRMED_CLOSE.load(Ordering::SeqCst),
                    close::SAVING.load(Ordering::SeqCst),
                    close::READY.load(Ordering::SeqCst),
                    window.app_handle().tray_by_id("dualfile").is_some()
                        && !system::QUIT_REQUESTED.load(Ordering::SeqCst),
                ) {
                    close::Decision::Allow => {}
                    close::Decision::Hide => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    close::Decision::Confirm => {
                        api.prevent_close();
                        let _ = window.emit(CORE_EVENT, CoreEventDto::CloseRequested { count });
                    }
                    close::Decision::Wait => api.prevent_close(),
                    close::Decision::Save => {
                        api.prevent_close();
                        close::begin(window);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            system::system_status,
            system::start_menu_set,
            system::tray_set,
            system::update_check,
            system::update_install,
            system::update_cancel,
            system::release_open,
            commands::undo_list,
            commands::undo_run,
            commands::ssh_prompt_answer,
            commands::system_places_list,
            commands::system_places_import,
            commands::system_mount_connect,
            commands::system_mount_unmount,
            commands::ssh_hosts_list,
            commands::ssh_host_resolve,
            commands::list,
            commands::home_dir,
            commands::copy,
            commands::move_items,
            commands::pack_archive,
            commands::trash,
            commands::restore_from_trash,
            commands::plan_delete,
            commands::delete_permanent,
            commands::rename,
            commands::rename_many,
            commands::regex_replace,
            commands::make_dir,
            commands::open_external,
            commands::app_confirm_close,
            commands::app_close_now,
            commands::answer_item_error,
            commands::job_pause,
            commands::job_resume,
            commands::cancel_job,
            commands::answer_conflict,
            commands::jobs,
            commands::load_settings,
            commands::save_settings,
            commands::watch,
            commands::file_info,
            commands::read_block,
            commands::find_in_file,
            commands::cancel_search,
            commands::compare_dirs,
            commands::space_free,
            commands::usage_start,
            commands::usage_node,
            commands::usage_close,
            commands::dupes_start,
            commands::similar_start,
            commands::similar_page,
            commands::similar_close,
            commands::similar_suggest,
            commands::similar_thumbnail,
            commands::similar_trash,
            commands::dupes_page,
            commands::dupes_close,
            commands::dupes_suggest,
            commands::dupes_trash,
            commands::compare_page,
            commands::compare_close,
            commands::plan_sync,
            commands::sync_plan_page,
            commands::run_sync,
            commands::diff_files,
            commands::search_start,
            commands::search_page,
            commands::search_close,
            commands::connections_list,
            commands::connection_save,
            commands::connection_delete,
            commands::connection_connect,
            commands::connection_trust,
            commands::connection_reset_trust,
            commands::connection_disconnect,
            commands::secret_set,
            commands::secret_clear,
            commands::plugins_list,
            commands::plugin_activate,
            commands::plugin_deactivate,
            commands::plugin_trust_reset,
            commands::plugin_diagnostics,
            commands::plugin_columns,
            commands::plugin_preview,
        ])
        .build(tauri::generate_context!())
        .expect("Dualfile konnte nicht gestartet werden");

    app.run(|handle, event| {
        #[cfg(unix)]
        if matches!(&event, RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main") || matches!(&event, RunEvent::Exit) {
            if let Some(bridge) = handle.try_state::<Arc<ssh_prompt::SshPromptBridge>>() { bridge.close(); }
        }
        if let RunEvent::Exit = event {
            if CONFIRMED_CLOSE.load(std::sync::atomic::Ordering::SeqCst) { return; }
            // Jobs get cancelled and clean up their temporary files; do not wait forever.
            if let Some(state) = handle.try_state::<Arc<AppState>>() {
                let state = Arc::clone(state.inner());
                let (done, finished) = mpsc::channel();
                std::thread::spawn(move || {
                    state.shutdown();
                    let _ = done.send(());
                });
                let _ = finished.recv_timeout(Duration::from_secs(5));
            }
        }
    });
}
