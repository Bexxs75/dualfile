//! The only interface between the web UI and the Rust side. Every command validates its
//! input in `AppState` and runs on a blocking thread, never on the UI or async executor.

use crate::compare::{
    CompareFilterDto, CompareOptionsDto, ComparePageDto, DiffDto, DiffOptionsDto, SyncModeDto,
    SyncOptionsDto, SyncPlanDto, SyncPlanPageDto,
};
use crate::connections::{
    ConnectOutcomeDto, ConnectionEntryDto, ConnectionProfileDto, SecretStoreDto, TrustDecisionDto,
};
use crate::convert::{
    ConflictAnswerDto, DeletePlanDto, DeletePlanInput, DirectionDto, FileInfoDto, JobStatusDto,
    ListingDto, LoadedSettingsDto, PackFormatDto, RenameDto, RenameItemDto, SearchResultDto,
    SettingsDto, UiError,
};
use crate::plugins::{PluginColumnValuesDto, PluginInfoDto, PluginPreviewDto};
use crate::regex_tools::{self, RegexResultDto};
use crate::search::{SearchOptionsDto, SearchPageDto};
use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

type Shared<'a> = State<'a, Arc<AppState>>;

async fn run<T, F>(state: Shared<'_>, work: F) -> Result<T, UiError>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, UiError> + Send + 'static,
{
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let installing = crate::system::INSTALLING
            .read()
            .map_err(|_| UiError::internal())?;
        if *installing {
            return Err(UiError::new(
                "updateBusy",
                "Update installation in progress",
            ));
        }
        work(&state)
    })
    .await
    .map_err(|_| UiError::internal())?
}

#[tauri::command]
pub async fn list(state: Shared<'_>, path: String) -> Result<ListingDto, UiError> {
    // VFS, archives and ordinary local directories retain their original execution path.
    if !path.starts_with('/') {
        return run(state, move |s| s.list(&path)).await;
    }
    let key = crate::places::navigation_mount(path.clone()).await;
    let Some(key) = key else {
        return run(state, move |s| s.list(&path)).await;
    };
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        dualfile_core::places_pool::navigation()
            .run_keyed_wait(key, std::time::Duration::from_secs(3), move || {
                let installing = crate::system::INSTALLING
                    .read()
                    .map_err(|_| UiError::internal())?;
                if *installing {
                    return Err(UiError::new(
                        "updateBusy",
                        "Update installation in progress",
                    ));
                }
                state.list(&path)
            })
            .unwrap_or_else(|| Err(UiError::new("timeout", "Directory unavailable")))
    })
    .await
    .map_err(|_| UiError::internal())?
}

#[tauri::command]
pub async fn home_dir(state: Shared<'_>) -> Result<String, UiError> {
    run(state, |s| Ok(s.home_dir())).await
}

#[tauri::command]
pub async fn copy(
    state: Shared<'_>,
    sources: Vec<String>,
    destination: String,
) -> Result<u64, UiError> {
    run(state, move |s| s.copy(&sources, &destination)).await
}

/// Creates a new archive from the sources as a job; the result arrives as job events. An
/// existing file of that name is never replaced (`exists`).
#[tauri::command]
pub async fn pack_archive(
    state: Shared<'_>,
    sources: Vec<String>,
    destination: String,
    name: String,
    format: PackFormatDto,
) -> Result<u64, UiError> {
    run(state, move |s| {
        s.pack_archive(&sources, &destination, &name, format)
    })
    .await
}

/// `move` is a Rust keyword, hence `move_items` (the UI layer maps it).
#[tauri::command]
pub async fn move_items(
    state: Shared<'_>,
    sources: Vec<String>,
    destination: String,
) -> Result<u64, UiError> {
    run(state, move |s| s.move_items(&sources, &destination)).await
}

#[tauri::command]
pub async fn trash(state: Shared<'_>, items: Vec<String>) -> Result<u64, UiError> {
    run(state, move |s| s.trash(&items)).await
}

#[tauri::command]
pub async fn trash_list(
    state: Shared<'_>,
) -> Result<dualfile_core::trash_view::TrashList, UiError> {
    run(state, |s| s.trash_list()).await
}

#[tauri::command]
pub async fn trash_restore(state: Shared<'_>, ids: Vec<String>) -> Result<Vec<String>, UiError> {
    run(state, move |s| s.trash_restore(&ids)).await
}

#[tauri::command]
pub async fn trash_purge(state: Shared<'_>, ids: Vec<String>) -> Result<usize, UiError> {
    run(state, move |s| s.trash_purge(&ids)).await
}

#[tauri::command]
pub async fn restore_from_trash(state: Shared<'_>, items: Vec<String>) -> Result<(), UiError> {
    run(state, move |s| s.restore_from_trash(&items)).await
}

#[tauri::command]
pub async fn plan_delete(state: Shared<'_>, items: Vec<String>) -> Result<DeletePlanDto, UiError> {
    run(state, move |s| s.plan_delete(&items)).await
}

#[tauri::command]
pub async fn delete_permanent(
    state: Shared<'_>,
    plan: DeletePlanInput,
    confirmed_token: String,
) -> Result<u64, UiError> {
    run(state, move |s| s.delete_permanent(&plan, &confirmed_token)).await
}

#[tauri::command]
pub async fn rename(
    state: Shared<'_>,
    path: String,
    new_name: String,
) -> Result<RenameDto, UiError> {
    run(state, move |s| s.rename(&path, &new_name)).await
}

/// Starts a job (like copy/move) and returns its number; the result arrives as job events.
#[tauri::command]
pub async fn rename_many(
    state: Shared<'_>,
    folder: String,
    items: Vec<RenameItemDto>,
) -> Result<u64, UiError> {
    run(state, move |s| s.rename_many(&folder, &items)).await
}

/// Regular expressions are evaluated here (linear-time engine), never in the web view.
#[tauri::command]
pub async fn regex_replace(
    state: Shared<'_>,
    names: Vec<String>,
    pattern: String,
    replacement: String,
    ignore_case: bool,
    all: bool,
) -> Result<Vec<RegexResultDto>, UiError> {
    run(state, move |_| {
        regex_tools::regex_replace(&names, &pattern, &replacement, ignore_case, all)
    })
    .await
}

#[tauri::command]
pub async fn make_dir(state: Shared<'_>, parent: String, name: String) -> Result<String, UiError> {
    run(state, move |s| s.make_dir(&parent, &name)).await
}

/// Only plain https links may leave the app through `open_url`.
pub(crate) fn is_safe_external_url(url: &str) -> bool {
    url.starts_with("https://") && url.len() <= 2048 && !url.chars().any(|c| c.is_control())
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), UiError> {
    if !is_safe_external_url(&url) {
        return Err(UiError::new("invalidInput", "Ungültige Adresse"));
    }
    // Same opener as for files, so a missing or failing browser handler is reported instead of
    // the click silently doing nothing.
    // It waits briefly for an early exit, so it runs off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        crate::state::open_with_system(std::path::Path::new(&url)).inspect_err(|e| {
            // Query and fragment may carry prefilled text: neither goes into the log.
            let short = url.split(['?', '#']).next().unwrap_or_default();
            let message = e.message.replace(url.as_str(), short);
            log::warn!("Link konnte nicht geöffnet werden: {short}: {message}");
        })
    })
    .await
    .map_err(|_| UiError::new("unsupported", "Link konnte nicht geöffnet werden"))?
}

#[tauri::command]
pub async fn open_external(state: Shared<'_>, path: String) -> Result<(), UiError> {
    run(state, move |s| s.open_external(&path)).await
}

#[tauri::command]
pub async fn cancel_job(state: Shared<'_>, id: u64) -> Result<(), UiError> {
    run(state, move |s| s.cancel_job(id)).await
}

#[tauri::command]
pub async fn answer_conflict(
    state: Shared<'_>,
    job_id: u64,
    answer: ConflictAnswerDto,
) -> Result<(), UiError> {
    run(state, move |s| s.answer_conflict(job_id, answer)).await
}

#[tauri::command]
pub async fn jobs(state: Shared<'_>) -> Result<Vec<JobStatusDto>, UiError> {
    run(state, |s| Ok(s.jobs())).await
}

#[tauri::command]
pub async fn load_settings(state: Shared<'_>) -> Result<LoadedSettingsDto, UiError> {
    run(state, |s| Ok(s.load_settings())).await
}

#[tauri::command]
pub async fn save_settings(state: Shared<'_>, settings: SettingsDto) -> Result<(), UiError> {
    run(state, move |s| s.save_settings(&settings)).await
}

#[tauri::command]
pub async fn watch(state: Shared<'_>, paths: Vec<String>) -> Result<(), UiError> {
    run(state, move |s| s.watch(&paths)).await
}

#[tauri::command]
pub async fn file_info(state: Shared<'_>, path: String) -> Result<FileInfoDto, UiError> {
    run(state, move |s| s.file_info(&path)).await
}

#[tauri::command]
pub async fn media_tags(
    state: Shared<'_>,
    paths: Vec<String>,
) -> Result<Vec<dualfile_core::mediatags::MediaTags>, UiError> {
    run(state, move |s| Ok(s.media_tags(paths))).await
}

/// Returns the bytes as a raw binary IPC response (JS: `ArrayBuffer`). The alternatives are
/// worse: a serialized `Vec<u8>` becomes a JSON number array (several bytes of text per byte,
/// slow to parse for 1 MiB blocks), and Base64 would cost 33 % more size plus an extra
/// dependency and a decode step in the UI. `tauri::ipc::Response` is built into Tauri 2.
#[tauri::command]
pub async fn read_block(
    state: Shared<'_>,
    path: String,
    offset: u64,
    len: u64,
) -> Result<tauri::ipc::Response, UiError> {
    let bytes = run(state, move |s| s.read_block(&path, offset, len)).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// `needle` arrives as a number array (small, at most `lister::MAX_NEEDLE` bytes, checked by
/// the core).
#[tauri::command]
pub async fn find_in_file(
    state: Shared<'_>,
    path: String,
    needle: Vec<u8>,
    start: u64,
    direction: DirectionDto,
    ignore_case: bool,
) -> Result<SearchResultDto, UiError> {
    run(state, move |s| {
        s.find_in_file(&path, &needle, start, direction, ignore_case)
    })
    .await
}

#[tauri::command]
pub async fn cancel_search(state: Shared<'_>) -> Result<(), UiError> {
    run(state, |s| {
        s.cancel_search();
        Ok(())
    })
    .await
}

/// Compares two folders as a job; the answer is the job number, which is also the id of the
/// result (`compare_page`, `compare_close`).
#[tauri::command]
pub async fn compare_dirs(
    state: Shared<'_>,
    left: String,
    right: String,
    options: CompareOptionsDto,
) -> Result<u64, UiError> {
    run(state, move |s| s.compare_dirs(&left, &right, &options)).await
}

#[tauri::command]
pub async fn compare_page(
    state: Shared<'_>,
    id: u64,
    offset: u64,
    limit: u64,
    filter: CompareFilterDto,
) -> Result<ComparePageDto, UiError> {
    run(state, move |s| s.compare_page(id, offset, limit, &filter)).await
}

#[tauri::command]
pub async fn compare_close(state: Shared<'_>, id: u64) -> Result<(), UiError> {
    run(state, move |s| s.compare_close(id)).await
}

#[tauri::command]
pub async fn plan_sync(
    state: Shared<'_>,
    compare_id: u64,
    mode: SyncModeDto,
    options: SyncOptionsDto,
    selection: Option<Vec<String>>,
) -> Result<SyncPlanDto, UiError> {
    run(state, move |s| {
        s.plan_sync(compare_id, mode, &options, selection.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn sync_plan_page(
    state: Shared<'_>,
    token: String,
    offset: u64,
    limit: u64,
) -> Result<SyncPlanPageDto, UiError> {
    run(state, move |s| s.sync_plan_page(&token, offset, limit)).await
}

/// Runs a confirmed plan as a job; only the token counts, and a plan is valid once.
#[tauri::command]
pub async fn run_sync(state: Shared<'_>, token: String) -> Result<u64, UiError> {
    run(state, move |s| s.run_sync(&token)).await
}

#[tauri::command]
pub async fn diff_files(
    state: Shared<'_>,
    left: String,
    right: String,
    options: DiffOptionsDto,
) -> Result<DiffDto, UiError> {
    run(state, move |s| s.diff_files(&left, &right, &options)).await
}

/// Searches below `root` (name, text, archives) as a job; the answer is the job number,
/// which is also the id of the result (`search_page`, `search_close`). Cancel with
/// `cancel_job`.
#[tauri::command]
pub async fn search_start(
    state: Shared<'_>,
    root: String,
    options: SearchOptionsDto,
) -> Result<u64, UiError> {
    run(state, move |s| s.search_start(&root, &options)).await
}

#[tauri::command]
pub async fn search_page(
    state: Shared<'_>,
    id: u64,
    offset: u64,
    limit: u64,
) -> Result<SearchPageDto, UiError> {
    run(state, move |s| s.search_page(id, offset, limit)).await
}

#[tauri::command]
pub async fn search_close(state: Shared<'_>, id: u64) -> Result<(), UiError> {
    run(state, move |s| s.search_close(id)).await
}

// ---------------------------------------------------------------- Server connections

#[tauri::command]
pub async fn connections_list(state: Shared<'_>) -> Result<Vec<ConnectionEntryDto>, UiError> {
    run(state, |s| Ok(s.connections_list())).await
}

#[tauri::command]
pub async fn connection_save(
    state: Shared<'_>,
    profile: ConnectionProfileDto,
) -> Result<ConnectionEntryDto, UiError> {
    run(state, move |s| s.connection_save(profile)).await
}

#[tauri::command]
pub async fn connection_delete(state: Shared<'_>, profile_id: String) -> Result<(), UiError> {
    run(state, move |s| s.connection_delete(&profile_id)).await
}

/// The only way a password enters (once, as the argument of this call); it is not kept unless
/// the profile says so, and no answer or error contains it.
#[tauri::command]
pub async fn connection_connect(
    state: Shared<'_>,
    profile_id: String,
    secret: Option<String>,
    expected: Option<ConnectionProfileDto>,
) -> Result<ConnectOutcomeDto, UiError> {
    run(state, move |s| {
        s.connection_connect(&profile_id, secret, expected.as_ref())
    })
    .await
}

#[tauri::command]
pub async fn connection_trust(
    state: Shared<'_>,
    profile_id: String,
    decision: TrustDecisionDto,
    fingerprint: String,
) -> Result<ConnectOutcomeDto, UiError> {
    run(state, move |s| {
        s.connection_trust(&profile_id, decision, &fingerprint)
    })
    .await
}

#[tauri::command]
pub async fn connection_reset_trust(state: Shared<'_>, profile_id: String) -> Result<(), UiError> {
    run(state, move |s| s.connection_reset_trust(&profile_id)).await
}

#[tauri::command]
pub async fn connection_disconnect(state: Shared<'_>, profile_id: String) -> Result<(), UiError> {
    run(state, move |s| s.connection_disconnect(&profile_id)).await
}

#[tauri::command]
pub async fn secret_set(
    state: Shared<'_>,
    profile_id: String,
    secret: String,
) -> Result<SecretStoreDto, UiError> {
    run(state, move |s| s.secret_set(&profile_id, secret)).await
}

#[tauri::command]
pub async fn secret_clear(state: Shared<'_>, profile_id: String) -> Result<(), UiError> {
    run(state, move |s| s.secret_clear(&profile_id)).await
}

// ---------------------------------------------------------------- Plugins

#[tauri::command]
pub async fn plugins_list(state: Shared<'_>) -> Result<Vec<PluginInfoDto>, UiError> {
    run(state, |s| Ok(s.plugins_list())).await
}

/// Switches a plugin on after the user's confirmation; only for the package with exactly this
/// manifest hash. `unsandboxed` needs `confirm_unsandboxed` as a second, explicit parameter.
#[tauri::command]
pub async fn plugin_activate(
    state: Shared<'_>,
    id: String,
    manifest_hash: String,
    grant_write: bool,
    unsandboxed: bool,
    confirm_unsandboxed: bool,
) -> Result<PluginInfoDto, UiError> {
    run(state, move |s| {
        s.plugin_activate(
            &id,
            &manifest_hash,
            grant_write,
            unsandboxed,
            confirm_unsandboxed,
        )
    })
    .await
}

#[tauri::command]
pub async fn plugin_deactivate(state: Shared<'_>, id: String) -> Result<PluginInfoDto, UiError> {
    run(state, move |s| s.plugin_deactivate(&id)).await
}

#[tauri::command]
pub async fn plugin_trust_reset(state: Shared<'_>, id: String) -> Result<(), UiError> {
    run(state, move |s| s.plugin_trust_reset(&id)).await
}

#[tauri::command]
pub async fn plugin_diagnostics(state: Shared<'_>, id: String) -> Result<String, UiError> {
    run(state, move |s| s.plugin_diagnostics(&id)).await
}

#[tauri::command]
pub async fn plugin_columns(
    state: Shared<'_>,
    paths: Vec<String>,
    column_ids: Vec<String>,
) -> Result<Vec<PluginColumnValuesDto>, UiError> {
    run(state, move |s| s.plugin_columns(&paths, &column_ids)).await
}

#[tauri::command]
pub async fn plugin_preview(
    state: Shared<'_>,
    path: String,
    plugin_id: Option<String>,
) -> Result<PluginPreviewDto, UiError> {
    run(state, move |s| {
        s.plugin_preview(&path, plugin_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn undo_list(state: Shared<'_>) -> Result<Vec<crate::convert::UndoEntryDto>, UiError> {
    run(state, |s| Ok(s.undo_list())).await
}
#[tauri::command]
pub async fn undo_run(
    state: Shared<'_>,
    id: u64,
) -> Result<crate::convert::UndoResultDto, UiError> {
    run(state, move |s| Ok(s.undo_run(id))).await
}

#[cfg(unix)]
#[tauri::command]
pub fn ssh_prompt_answer(
    bridge: tauri::State<'_, std::sync::Arc<crate::ssh_prompt::SshPromptBridge>>,
    window: tauri::WebviewWindow,
    id: String,
    text: Option<String>,
) -> Result<(), &'static str> {
    if window.label() != "main" {
        return Err("SSH-Abfrage nicht verfügbar");
    }
    if bridge.answer(&id, text) {
        Ok(())
    } else {
        Err("SSH-Abfrage abgelaufen")
    }
}

#[cfg(not(unix))]
#[tauri::command]
pub fn ssh_prompt_answer(id: String, text: Option<String>) -> Result<(), &'static str> {
    let _ = (id, text);
    Err("SSH-Abfragen nicht verfügbar")
}

/// The SSH host list and resolver are also needed by the connection editor window.
fn ssh_config_window(label: &str) -> Result<(), UiError> {
    if label == "main" || label == crate::connection_editor::EDITOR_LABEL {
        Ok(())
    } else {
        Err(UiError::new(
            "unsupported",
            "SSH-Konfiguration nicht verfügbar",
        ))
    }
}

fn ssh_main_window(label: &str) -> Result<(), UiError> {
    if label == "main" {
        Ok(())
    } else {
        Err(UiError::new(
            "unsupported",
            "SSH-Konfiguration nicht verfügbar",
        ))
    }
}

#[tauri::command]
pub async fn ssh_hosts_list(window: tauri::WebviewWindow) -> Result<Vec<String>, UiError> {
    ssh_config_window(window.label())?;
    #[cfg(unix)]
    {
        tauri::async_runtime::spawn_blocking(|| {
            dualfile_remote::ssh_config::list_hosts().map_err(|e| {
                crate::convert::connect_error(&dualfile_remote::manager::ConnectError::Transport(e))
            })
        })
        .await
        .map_err(|_| UiError::new("remote", "SSH-Konfiguration nicht verfügbar"))?
    }
    #[cfg(not(unix))]
    {
        Err(UiError::new(
            "unsupported",
            "SSH-Konfiguration nicht verfügbar",
        ))
    }
}

#[tauri::command]
pub async fn ssh_host_resolve(
    window: tauri::WebviewWindow,
    alias: String,
) -> Result<serde_json::Value, UiError> {
    ssh_config_window(window.label())?;
    #[cfg(unix)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let host = dualfile_remote::ssh_config::resolve(&alias).map_err(|e| {
                crate::convert::connect_error(&dualfile_remote::manager::ConnectError::Transport(e))
            })?;
            serde_json::to_value(host)
                .map_err(|_| UiError::new("remote", "SSH-Konfiguration nicht verfügbar"))
        })
        .await
        .map_err(|_| UiError::new("remote", "SSH-Konfiguration nicht verfügbar"))?
    }
    #[cfg(not(unix))]
    {
        let _ = alias;
        Err(UiError::new(
            "unsupported",
            "SSH-Konfiguration nicht verfügbar",
        ))
    }
}

#[cfg(test)]
mod ssh_command_tests {
    #[test]
    fn ssh_configuration_is_available_to_main_and_the_editor() {
        assert!(super::ssh_config_window("main").is_ok());
        assert!(super::ssh_config_window("connection-editor").is_ok());
        for label in ["", "preview", "MAIN", "connection-editor2"] {
            assert!(super::ssh_config_window(label).is_err());
        }
    }

    #[test]
    fn ssh_places_stay_main_only() {
        assert!(super::ssh_main_window("connection-editor").is_err());
    }

    #[test]
    fn ssh_configuration_is_only_available_to_main() {
        assert!(super::ssh_main_window("main").is_ok());
        for label in ["", "preview", "MAIN"] {
            assert!(super::ssh_main_window(label).is_err());
        }
    }
}

#[tauri::command]
pub async fn job_pause(window: tauri::Window, state: Shared<'_>, id: u64) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.job_pause(id)).await
}

#[tauri::command]
pub async fn job_resume(window: tauri::Window, state: Shared<'_>, id: u64) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.job_resume(id)).await
}

fn job_main_window(label: &str) -> Result<(), UiError> {
    if label == "main" {
        Ok(())
    } else {
        Err(UiError::new(
            "permission",
            "Aufträge sind nur im Hauptfenster verfügbar",
        ))
    }
}
#[cfg(test)]
mod job_window_tests {
    #[test]
    fn main_only() {
        assert!(super::job_main_window("main").is_ok());
        for label in ["", "preview", "settings"] {
            assert_eq!(
                super::job_main_window(label).unwrap_err().kind,
                "permission"
            );
        }
    }
}

#[tauri::command]
pub async fn answer_item_error(
    window: tauri::Window,
    state: Shared<'_>,
    job_id: u64,
    decision: crate::item_error::ErrorDecisionDto,
) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.answer_item_error(job_id, decision)).await
}

#[tauri::command]
pub async fn app_confirm_close(window: tauri::Window, state: Shared<'_>) -> Result<(), UiError> {
    job_main_window(window.label())?;
    if crate::CONFIRMED_CLOSE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    let state = std::sync::Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            state.shutdown();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
        crate::close::begin(&window);
    });
    Ok(())
}

#[tauri::command]
pub async fn app_close_now(window: tauri::Window) -> Result<(), UiError> {
    job_main_window(window.label())?;
    crate::close::finish(&window, true, 0);
    Ok(())
}

#[tauri::command]
pub async fn usage_start(
    window: tauri::Window,
    state: Shared<'_>,
    path: String,
    options: crate::space::UsageOptionsDto,
) -> Result<u64, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.usage_start(&path, options)).await
}

#[tauri::command]
pub async fn usage_node(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    node_path: Option<String>,
) -> Result<crate::space::UsagePageDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.usage_node(id, node_path.as_deref())).await
}

#[tauri::command]
pub async fn usage_close(window: tauri::Window, state: Shared<'_>, id: u64) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.usage_close(id)).await
}

#[tauri::command]
pub async fn dupes_start(
    window: tauri::Window,
    state: Shared<'_>,
    roots: Vec<String>,
    options: crate::space::DupesOptionsDto,
) -> Result<u64, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.dupes_start(&roots, options)).await
}

#[tauri::command]
pub async fn dupes_page(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    offset: usize,
    limit: usize,
) -> Result<crate::space::DupesPageDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.dupes_page(id, offset, limit)).await
}

#[tauri::command]
pub async fn dupes_close(window: tauri::Window, state: Shared<'_>, id: u64) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.dupes_close(id)).await
}

#[tauri::command]
pub async fn dupes_suggest(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    rule: crate::space::KeepRuleDto,
    folder: String,
) -> Result<Vec<crate::space::KeepSuggestionDto>, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.dupes_suggest(id, rule, &folder)).await
}

#[tauri::command]
pub async fn dupes_trash(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    choices: Vec<crate::space::KeepChoice>,
) -> Result<crate::space::TrashResultDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.dupes_trash(id, choices)).await
}

#[tauri::command]
pub async fn similar_start(
    window: tauri::Window,
    state: Shared<'_>,
    roots: Vec<String>,
    options: crate::similar_space::SimilarOptionsDto,
) -> Result<u64, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.similar_start(&roots, options)).await
}

#[tauri::command]
pub async fn similar_page(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    offset: usize,
    limit: usize,
) -> Result<crate::similar_space::SimilarPageDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.similar_page(id, offset, limit)).await
}

#[tauri::command]
pub async fn similar_close(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.similar_close(id)).await
}

#[tauri::command]
pub async fn similar_suggest(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    rule: crate::similar_space::SimilarRuleDto,
) -> Result<Vec<crate::space::KeepChoice>, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.similar_suggest(id, rule)).await
}

#[tauri::command]
pub async fn similar_trash(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    choices: Vec<crate::similar_space::TrashChoice>,
) -> Result<crate::space::TrashResultDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.similar_trash(id, choices)).await
}

#[tauri::command]
pub async fn similar_thumbnail(
    window: tauri::Window,
    state: Shared<'_>,
    id: u64,
    group_index: usize,
    entry_index: usize,
) -> Result<crate::similar_space::ThumbnailDto, UiError> {
    job_main_window(window.label())?;
    run(state, move |s| {
        s.similar_thumbnail(id, group_index, entry_index)
    })
    .await
}

#[tauri::command]
pub async fn local_drives(
    window: tauri::WebviewWindow,
) -> Result<Vec<dualfile_core::drives::Drive>, UiError> {
    ssh_main_window(window.label())?;
    Ok(tauri::async_runtime::spawn_blocking(|| {
        dualfile_core::places_pool::metadata()
            .run(
                std::time::Duration::from_millis(1500),
                dualfile_core::drives::local_drives,
            )
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default())
}

#[tauri::command]
pub async fn system_places_list(
    window: tauri::WebviewWindow,
) -> Result<Vec<dualfile_core::system_places::SystemPlace>, UiError> {
    ssh_main_window(window.label())?;
    tauri::async_runtime::spawn_blocking(|| {
        dualfile_core::places_pool::metadata()
            .run(std::time::Duration::from_millis(1500), || {
                dualfile_core::system_places::discover(
                    &dualfile_core::system_places::DiscoveryPaths::system(),
                )
            })
            .unwrap_or_default()
    })
    .await
    .map_err(|_| UiError::new("remote", "Netzwerkorte nicht verfügbar"))
}
#[tauri::command]
pub async fn system_places_import(
    state: Shared<'_>,
    urls: Vec<String>,
) -> Result<Vec<ConnectionEntryDto>, UiError> {
    run(state, move |s| s.system_places_import(urls)).await
}
#[tauri::command]
pub async fn system_mount_connect(
    state: Shared<'_>,
    profile_id: String,
) -> Result<String, UiError> {
    run(state, move |s| s.system_mount_connect(&profile_id)).await
}
#[tauri::command]
pub async fn system_mount_unmount(state: Shared<'_>, profile_id: String) -> Result<(), UiError> {
    run(state, move |s| s.system_mount_unmount(&profile_id)).await
}

#[tauri::command]
pub async fn space_free(
    window: tauri::Window,
    state: Shared<'_>,
    path: String,
) -> Result<Option<dualfile_core::vfs::SpaceFree>, UiError> {
    if job_main_window(window.label()).is_err() {
        return Ok(None);
    }
    // A stuck system mount may not honor cancellation. Bound both waiting and worker count.
    static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    use std::sync::atomic::Ordering;
    if ACTIVE.fetch_add(1, Ordering::SeqCst) >= 2 {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        return Ok(None);
    }
    let state = Arc::clone(state.inner());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("free-space".into())
        .spawn(move || {
            struct Permit;
            impl Drop for Permit {
                fn drop(&mut self) {
                    ACTIVE.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _permit = Permit;
            let _ = tx.send(state.space_free(&path));
        });
    if worker.is_err() {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        return Ok(None);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_millis(750))
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::is_safe_external_url;

    #[test]
    fn only_short_https_urls_without_control_characters_pass() {
        assert!(is_safe_external_url("https://github.com/Bexxs75/dualfile"));
        for bad in [
            "http://github.com",
            "ftp://x.org/a",
            "file:///etc/passwd",
            "-https://x",
            "",
            "https://x.org/\n",
            "https://x.org/\0",
        ] {
            assert!(!is_safe_external_url(bad), "{bad:?}");
        }
        let long = format!("https://x.org/{}", "a".repeat(2048));
        assert!(!is_safe_external_url(&long));
    }
}

#[tauri::command]
pub async fn trash_empty(
    window: tauri::WebviewWindow,
    state: Shared<'_>,
) -> Result<crate::convert::TrashEmptyDto, UiError> {
    job_main_window(window.label())?;
    run(state, |s| s.trash_empty()).await
}

#[tauri::command]
pub async fn places_save_connection(
    window: tauri::WebviewWindow,
    state: Shared<'_>,
    key: String,
) -> Result<(), UiError> {
    job_main_window(window.label())?;
    run(state, move |s| s.places_save_connection(&key)).await
}
