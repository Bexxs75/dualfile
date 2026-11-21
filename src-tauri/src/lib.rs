//! Dualfile: Tauri shell around `dualfile-core`. The web UI reaches the file system only
//! through the commands in `commands.rs`; the core reports back through `core-event`.

mod anonymize;
#[cfg(target_os = "linux")]
mod app_id_migration;
mod autostart;
mod close;
mod close_policy;
mod commands;
mod compare;
mod conflict;
mod connection_editor;
mod connections;
mod convert;
mod desktop_entry;
mod devices;
mod diagnostics;
mod discovery;
mod item_error;
mod keychain;
mod lanes;
#[cfg(target_os = "linux")]
mod legacy_instance;
mod mount_guard;
mod places;
mod plans;
mod plugins;
mod regex_tools;
mod search;
mod settings_window;
mod similar_space;
mod space;
#[cfg(unix)]
mod ssh_prompt;
mod state;
mod system;
mod terminal;
mod validate;
mod verification;
mod watch;
mod window_state;

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
    gtk::glib::set_prgname(Some(desktop_entry::ID));
    diagnostics::install_panic_hook();
    let builder = tauri::Builder::default();
    #[cfg(target_os = "linux")]
    let builder = builder.plugin(legacy_instance::plugin());
    let builder = builder
        // Both instance guards precede migration and state construction. The legacy
        // endpoint also activates 0.4.0 and handles old executables started after us.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                system::bring_to_front(&window);
            }
        }))
        .plugin(diagnostics::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    #[cfg(target_os = "linux")]
    let builder = builder.plugin(app_id_migration::plugin());
    let app = builder
        .plugin(window_state::plugin())
        .setup(|app| {
            diagnostics::log_start();
            window_state::initialize_backend();
            let places = Arc::new(places::Places::default());
            places::start(app.handle().clone(), places.clone());
            app.manage(places);
            app.manage(Arc::new(settings_window::SettingsRelay::default()));
            // Title bar / task bar icon (X11 and XWayland; Wayland desktops take
            // it from the start-menu entry that matches the application id).
            if let (Some(window), Some(icon)) =
                (app.get_webview_window("main"), app.default_window_icon())
            {
                let _ = window.set_icon(icon.clone());
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("Dualfile {}", app.package_info().version));
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
            let settings = state.load_settings().settings;
            state
                .discovery
                .set_enabled(settings.sidebar_discovery, &TauriSink(app.handle().clone()));
            if let Some(window) = app.get_webview_window("main") {
                // Before the window is shown: GTK applies the position when it maps the window.
                window_state::restore(&window);
                system::show_at_start(
                    &window,
                    autostart::minimized_requested(std::env::args_os()),
                    settings.tray_enabled,
                );
            }
            state
                .discovery
                .start(Arc::new(TauriSink(app.handle().clone())));
            app.manage(Arc::new(state));
            app.manage(Arc::new(terminal::TerminalManager::default()));
            #[cfg(target_os = "linux")]
            system::watch_minimize(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == settings_window::SETTINGS_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    settings_window::request_close(
                        window.app_handle(),
                        settings_window::AfterClose::None,
                    );
                }
                return;
            }
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::Moved(position) = event {
                window_state::track(window.app_handle(), *position);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Here the window is still shown, whether it then closes or hides to the tray.
                window_state::save(window.app_handle());
                if *system::INSTALLING.read().unwrap_or_else(|e| e.into_inner()) {
                    api.prevent_close();
                    return;
                }
                if window
                    .app_handle()
                    .get_webview_window(settings_window::SETTINGS_LABEL)
                    .is_some()
                {
                    settings_window::request_close(
                        window.app_handle(),
                        settings_window::AfterClose::Main,
                    );
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
                    system::tray_usable(window.app_handle())
                        && !system::QUIT_REQUESTED.load(Ordering::SeqCst),
                ) {
                    close::Decision::Allow => connection_editor::mark_main_closing(),
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
            places::places_snapshot,
            places::discovery_snapshot,
            places::places_mount_affected,
            commands::places_save_connection,
            devices::device_list,
            devices::device_mount,
            devices::device_unmount,
            devices::device_eject,
            devices::device_power_off,
            places::fstab_mount,
            places::fstab_unmount,
            places::places_mount_unmount,
            commands::trash_empty,
            places::places_space,
            places::places_set_visible,
            places::trash_open_system,
            places::sidebar_resize_window,
            settings_window::settings_window_open,
            settings_window::settings_window_close,
            settings_window::settings_window_attach,
            settings_window::settings_window_push,
            settings_window::settings_window_send,
            terminal::terminal_open,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_close,
            system::system_status,
            system::start_menu_set,
            system::autostart_set,
            system::tray_set,
            system::update_check,
            system::update_install,
            system::update_cancel,
            system::app_restart,
            system::release_open,
            commands::undo_list,
            commands::undo_run,
            commands::ssh_prompt_answer,
            commands::local_drives,
            commands::system_places_list,
            commands::system_places_import,
            commands::system_mount_connect,
            commands::system_mount_unmount,
            commands::ssh_hosts_list,
            commands::ssh_host_resolve,
            connection_editor::connection_editor_open,
            connection_editor::connection_editor_close,
            commands::list,
            commands::home_dir,
            commands::copy,
            commands::move_items,
            commands::pack_archive,
            commands::trash,
            commands::restore_from_trash,
            commands::trash_list,
            commands::trash_restore,
            commands::trash_purge,
            commands::plan_delete,
            commands::delete_permanent,
            commands::rename,
            commands::rename_many,
            commands::regex_replace,
            commands::make_dir,
            commands::open_external,
            commands::open_url,
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
            commands::media_tags,
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
            diagnostics::log_info,
            diagnostics::log_read,
            diagnostics::log_save,
            diagnostics::log_open_folder,
            diagnostics::report_issue_url,
            diagnostics::log_frontend,
        ])
        .build(tauri::generate_context!())
        .expect("Dualfile konnte nicht gestartet werden");

    app.run(|handle, event| {
        if matches!(&event, RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == settings_window::SETTINGS_LABEL) {
            handle.state::<Arc<settings_window::SettingsRelay>>().destroyed(handle);
        }
        // An editor window alone would keep the process alive after the main window is gone.
        if matches!(&event, RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main") {
            connection_editor::mark_main_closing();
            connection_editor::destroy_children(handle);
        }
        if matches!(&event, RunEvent::Exit) || matches!(&event, RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main") {
            if let Some(manager) = handle.try_state::<Arc<terminal::TerminalManager>>() { manager.shutdown(); }
        }
        #[cfg(unix)]
        if matches!(&event, RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main") || matches!(&event, RunEvent::Exit) {
            if let Some(bridge) = handle.try_state::<Arc<ssh_prompt::SshPromptBridge>>() { bridge.close(); }
        }
        if let RunEvent::Exit = event {
            handle.state::<Arc<places::Places>>().stop();
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
