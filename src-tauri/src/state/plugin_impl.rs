//! Commands for plugins: list, activate, deactivate, reset trust, diagnosis, columns, preview.
//! Part of `AppState`; DTOs and helpers live in `crate::plugins`.
//!
//! Rules kept here: nothing is switched on without an explicit command that carries the
//! manifest hash the user saw (and, for the sandbox-free mode, a second confirmation); a
//! changed package is locked and loses its rights; a plugin that keeps failing is switched off;
//! normally activated plugins are registered at program start but their process only starts at
//! first use; plugins confirmed to run without the sandbox are never registered by themselves.

use super::{lock, AppState};
use crate::convert::UiError;
use crate::plugins::{
    clean_multiline, clean_text, plugin_error, ActivePlugin, PluginColumnDto,
    PluginColumnValuesDto, PluginInfoDto, PluginPreviewDto, PluginSandboxDto, PluginStatusDto,
    MAX_CELL_CHARS, MAX_COLUMN_IDS, MAX_COLUMN_PATHS, MAX_DIAGNOSTIC_CHARS, MAX_NAME_CHARS,
    MAX_PREVIEW_CHARS, MAX_TITLE_CHARS, MAX_VERSION_CHARS,
};
use dualfile_core::archive::PackerBinding;
use dualfile_core::plugins::{DisableReason, PluginEntry, MAX_PLUGINS, MSG_PLUGIN_READ_ONLY};
use dualfile_core::settings::{self, LoadOutcome};
use dualfile_core::vfs::{EntryKind, Scheme, VPath};
use dualfile_plugin::client::DisabledCause;
use dualfile_plugin::manifest::is_valid_plugin_id;
use dualfile_plugin::protocol::{self, Preview};
use dualfile_plugin::{
    discover, verify_package, ColumnItem, Kind, PinnedState, PluginClient, PluginError,
    PluginPacker, PluginVfs,
};
use std::sync::Arc;

const MSG_LOCKED_CHANGED: &str =
    "Das Plugin hat sich seit der Anzeige verändert; bitte die Plugin-Liste neu laden";
const MSG_LOCKED_PINNED: &str =
    "Das Plugin wurde verändert; erst das Vertrauen zurücksetzen und es neu prüfen";
const MSG_PROBLEM_LOCKED: &str = "Das Plugin wurde seit der Bestätigung verändert";
const MSG_PROBLEM_FAILURES: &str = "Das Plugin ist nach wiederholten Fehlern deaktiviert";
const MSG_PROBLEM_MISSING: &str = "Das Plugin ist nicht mehr installiert";
const MSG_NEEDS_SECOND_CONFIRM: &str =
    "Ohne Sandbox zu starten braucht eine zweite, ausdrückliche Bestätigung";
const MSG_NOT_ACTIVE: &str = "Das Plugin ist nicht aktiviert";
const MSG_NO_PREVIEW: &str = "Dafür gibt es keine Vorschau";
const MSG_TOO_BIG_PREVIEW: &str = "Die Datei ist für die Vorschau zu groß";
/// Longest preview text asked of a plugin, in characters.
const PREVIEW_ASK_CHARS: usize = 20_000;

fn invalid(message: &str) -> UiError {
    UiError::new("invalidInput", message)
}

fn not_found_plugin() -> UiError {
    UiError::new("notFound", "Das Plugin gibt es nicht")
}

fn plugin_id(id: &str) -> Result<(), UiError> {
    if is_valid_plugin_id(id) {
        Ok(())
    } else {
        Err(invalid("Ungültige Plugin-Kennung"))
    }
}

fn pinned(entry: &PluginEntry) -> PinnedState {
    PinnedState {
        id: entry.id.clone(),
        manifest_sha256: entry.manifest_sha256.clone(),
        unsandboxed: entry.unsandboxed,
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Filesystem => "filesystem",
        Kind::Packer => "packer",
        Kind::Column => "column",
        Kind::Preview => "preview",
    }
}

impl AppState {
    // ------------------------------------------------------------ settings and registry

    /// Changes the plugin list in the settings file (read-modify-write under the lock).
    fn update_plugins(
        &self,
        change: impl FnOnce(&mut Vec<PluginEntry>) -> Result<(), UiError>,
    ) -> Result<(), UiError> {
        let _guard = lock(&self.connections.settings_lock);
        self.update_plugins_locked(change)
    }

    fn update_plugins_locked(
        &self,
        change: impl FnOnce(&mut Vec<PluginEntry>) -> Result<(), UiError>,
    ) -> Result<(), UiError> {
        let mut current = self.load_for_update()?;
        change(&mut current.plugins)?;
        dualfile_core::plugins::validate_plugins(&current.plugins)
            .map_err(|message| UiError::new("invalidSettings", message))?;
        current.version = settings::SETTINGS_VERSION;
        Ok(settings::save(&self.config_dir, &current, false)?)
    }

    fn stored_plugins(&self) -> Vec<PluginEntry> {
        let (loaded, outcome) = settings::load(&self.config_dir);
        match outcome {
            LoadOutcome::NewerVersion(_) => Vec::new(),
            _ => loaded.plugins,
        }
    }

    /// Registers the plugin's file system and packer with the router. Nothing is started.
    fn register_plugin(
        &self,
        client: &Arc<PluginClient>,
        write_granted: bool,
    ) -> Result<(), UiError> {
        let id = client.id().to_owned();
        let manifest = client.manifest();
        let filesystem = manifest.has_kind(Kind::Filesystem);
        let packer = manifest.has_kind(Kind::Packer);
        if filesystem {
            let vfs = PluginVfs::with_write_grant(client.clone(), write_granted);
            self.router.register_plugin(&id, Arc::new(vfs))?;
        }
        if packer {
            let extensions = manifest
                .packer
                .as_ref()
                .map(|details| details.extensions.clone())
                .unwrap_or_default();
            let registered =
                PackerBinding::new(&id, extensions, Arc::new(PluginPacker::new(client.clone())))
                    .and_then(|binding| self.router.register_packer(binding));
            if let Err(error) = registered {
                if filesystem {
                    self.router.unregister_plugin(&id);
                }
                return Err(error.into());
            }
        }
        client.set_disabled_hook(self.plugins.hook());
        lock(&self.plugins.active).insert(
            id,
            ActivePlugin {
                client: client.clone(),
                filesystem,
            },
        );
        Ok(())
    }

    /// Takes the plugin out of the router and the active list; its process keeps running.
    fn detach_plugin(&self, id: &str) -> Option<ActivePlugin> {
        let removed = lock(&self.plugins.active).remove(id);
        self.router.unregister_plugin(id);
        self.router.unregister_packer(id);
        removed
    }

    /// Removes the plugin from the router and ends its process.
    fn unregister_plugin(&self, id: &str) {
        if let Some(active) = self.detach_plugin(id) {
            active.client.shutdown();
        }
    }

    fn plugin_client(&self, id: &str) -> Result<Arc<PluginClient>, UiError> {
        lock(&self.plugins.active)
            .get(id)
            .map(|active| active.client.clone())
            .ok_or_else(|| UiError::new("pluginDisabled", MSG_NOT_ACTIVE))
    }

    /// Program start: registers the plugins that were switched on normally. No process is
    /// started (that happens at first use), and plugins confirmed to run without the sandbox
    /// are not touched at all: they need the user's activation again.
    pub(super) fn restore_plugins(&self) {
        let (loaded, outcome) = settings::load(&self.config_dir);
        if matches!(outcome, LoadOutcome::NewerVersion(_)) {
            return;
        }
        for entry in loaded
            .plugins
            .iter()
            .filter(|entry| entry.enabled && !entry.unsandboxed)
        {
            match PluginClient::open(self.plugins.runtime.clone(), pinned(entry)) {
                Ok(client) => {
                    // a plugin that cannot be registered shows up as such in the list
                    let _ = self.register_plugin(&client, entry.write_granted);
                }
                Err(PluginError::Locked) => {
                    self.mark_plugin_off(&entry.id, DisabledCause::Locked, 0)
                }
                Err(_) => {}
            }
        }
    }

    /// Switches the entry off in the settings after the program (not the user) turned it off.
    fn mark_plugin_off(&self, id: &str, cause: DisabledCause, failures: u32) {
        let _ = self.update_plugins(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.enabled = false;
                entry.failures = failures;
                match cause {
                    DisabledCause::Failures => {
                        entry.disabled_reason = Some(DisableReason::Failures)
                    }
                    DisabledCause::Locked => {
                        entry.disabled_reason = Some(DisableReason::Locked);
                        entry.write_granted = false;
                    }
                }
            }
            Ok(())
        });
    }

    /// Applies what the plugins reported about themselves: one that failed too often or turned
    /// out to be changed is removed from the router and switched off in the settings.
    pub(super) fn apply_plugin_failures(&self) {
        for (id, cause) in self.plugins.take_tripped() {
            let failures = lock(&self.plugins.active)
                .get(&id)
                .map_or(0, |active| active.client.health().failures);
            self.unregister_plugin(&id);
            self.mark_plugin_off(&id, cause, failures);
            self.plugins
                .runtime
                .log(&format!("Plugin {id} wurde abgeschaltet"));
        }
    }

    /// App exit: ends all plugin processes.
    pub(super) fn close_plugins(&self) {
        let ids: Vec<String> = lock(&self.plugins.active).keys().cloned().collect();
        for id in ids {
            self.unregister_plugin(&id);
        }
    }

    /// Plugin file systems are only changed with the user's grant; the answer comes before a
    /// job is queued.
    pub(super) fn require_writable(&self, paths: &[VPath]) -> Result<(), UiError> {
        for path in paths.iter().filter(|p| p.scheme() == Scheme::Plugin) {
            if self.router.vfs_for(path)?.is_read_only() {
                return Err(UiError::new("readOnly", MSG_PLUGIN_READ_ONLY));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ list

    /// Locks entries whose package changed since the user confirmed it: rights are gone, the
    /// plugin is removed from the router, and the settings say so.
    fn lock_changed_plugins(
        &self,
        entries: &[PluginEntry],
        installed: &[dualfile_plugin::package::Installed],
    ) {
        for entry in entries.iter().filter(|e| e.enabled || e.write_granted) {
            let changed = installed
                .iter()
                .find(|inst| inst.id == entry.id)
                .is_some_and(|inst| match &inst.package {
                    Ok(package) => package.manifest_sha256 != entry.manifest_sha256,
                    Err(PluginError::Locked) => true,
                    Err(_) => false,
                });
            if changed {
                self.unregister_plugin(&entry.id);
                self.mark_plugin_off(&entry.id, DisabledCause::Locked, 0);
            }
        }
    }

    fn sandbox_ok(&self, sandbox: &PluginSandboxDto, entry: Option<&PluginEntry>) -> bool {
        sandbox.available || entry.is_some_and(|e| e.unsandboxed)
    }

    fn info_for(
        &self,
        id: &str,
        package: Option<&dualfile_plugin::VerifiedPackage>,
        error: Option<&PluginError>,
        entry: Option<&PluginEntry>,
        sandbox: &PluginSandboxDto,
    ) -> PluginInfoDto {
        let active = lock(&self.plugins.active)
            .get(id)
            .map(|a| (a.client.clone(), a.filesystem));
        let health = active.as_ref().map(|(client, _)| client.health());
        let manifest = package.map(|p| &p.manifest);
        let hash = package.map(|p| p.manifest_sha256.clone());
        let changed = match (entry, &hash, error) {
            (Some(entry), Some(hash), _) => &entry.manifest_sha256 != hash,
            (Some(_), None, Some(PluginError::Locked)) => true,
            _ => false,
        };
        let mut problem: Option<String> = None;
        let status = if package.is_none() {
            problem = Some(match error {
                Some(PluginError::Locked) => MSG_PROBLEM_LOCKED.to_owned(),
                Some(other) => other.to_string(),
                None => MSG_PROBLEM_MISSING.to_owned(),
            });
            if changed {
                PluginStatusDto::Locked
            } else {
                PluginStatusDto::Failed
            }
        } else if changed {
            problem = Some(MSG_PROBLEM_LOCKED.to_owned());
            PluginStatusDto::Locked
        } else if let Some(entry) = entry {
            if health.is_some_and(|h| h.disabled) {
                problem = Some(MSG_PROBLEM_FAILURES.to_owned());
                PluginStatusDto::Failed
            } else if active.is_some() {
                if self.sandbox_ok(sandbox, Some(entry)) {
                    PluginStatusDto::Enabled
                } else {
                    PluginStatusDto::SandboxUnavailable
                }
            } else if entry.disabled_reason == Some(DisableReason::Failures) {
                problem = Some(MSG_PROBLEM_FAILURES.to_owned());
                PluginStatusDto::Failed
            } else if !self.sandbox_ok(sandbox, Some(entry)) {
                PluginStatusDto::SandboxUnavailable
            } else {
                PluginStatusDto::Disabled
            }
        } else if sandbox.available {
            PluginStatusDto::New
        } else {
            PluginStatusDto::SandboxUnavailable
        };
        let columns = manifest
            .and_then(|m| m.columns.as_ref())
            .map(|columns| {
                columns
                    .iter()
                    .map(|column| PluginColumnDto {
                        id: format!("{id}:{}", column.id),
                        title: clean_text(&column.title, MAX_TITLE_CHARS),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let is_active = active.is_some();
        PluginInfoDto {
            id: id.to_owned(),
            name: manifest.map_or_else(|| id.to_owned(), |m| clean_text(&m.name, MAX_NAME_CHARS)),
            version: manifest
                .map_or_else(String::new, |m| clean_text(&m.version, MAX_VERSION_CHARS)),
            kinds: manifest
                .map(|m| m.kinds.iter().map(|k| kind_name(*k)).collect())
                .unwrap_or_default(),
            status,
            wants_write: manifest.is_some_and(|m| m.write),
            write_granted: entry.is_some_and(|e| e.write_granted) && !changed,
            unsandboxed: entry.is_some_and(|e| e.unsandboxed),
            enabled: is_active && status == PluginStatusDto::Enabled,
            manifest_hash: hash.unwrap_or_default(),
            sandbox: *sandbox,
            failures: health.map_or_else(|| entry.map_or(0, |e| e.failures), |h| h.failures),
            disabled_reason: entry
                .and_then(|e| e.disabled_reason)
                .map(|reason| match reason {
                    DisableReason::Failures => "failures",
                    DisableReason::Locked => "locked",
                }),
            problem,
            diagnostic: active
                .as_ref()
                .map(|(client, _)| clean_text(&client.diagnostic(), MAX_DIAGNOSTIC_CHARS))
                .unwrap_or_default(),
            columns,
            packer_extensions: manifest
                .and_then(|m| m.packer.as_ref())
                .map(|p| p.extensions.clone())
                .unwrap_or_default(),
            filesystem_root: active
                .as_ref()
                .filter(|(_, filesystem)| *filesystem && is_active)
                .and_then(|_| VPath::plugin(id, "/").ok())
                .map(|root| crate::convert::vpath_text(&root)),
        }
    }

    pub fn plugins_list(&self) -> Vec<PluginInfoDto> {
        self.apply_plugin_failures();
        let entries = self.stored_plugins();
        let installed = discover(&self.plugins.runtime.config().base_dir);
        self.lock_changed_plugins(&entries, &installed);
        let entries = self.stored_plugins();
        let sandbox = self.plugins.sandbox();
        let mut out: Vec<PluginInfoDto> = installed
            .iter()
            .map(|inst| {
                let entry = entries.iter().find(|e| e.id == inst.id);
                match &inst.package {
                    Ok(package) => self.info_for(&inst.id, Some(package), None, entry, &sandbox),
                    Err(error) => self.info_for(&inst.id, None, Some(error), entry, &sandbox),
                }
            })
            .collect();
        for entry in entries
            .iter()
            .filter(|entry| !installed.iter().any(|inst| inst.id == entry.id))
        {
            out.push(self.info_for(&entry.id, None, None, Some(entry), &sandbox));
        }
        out
    }

    fn plugin_info(&self, id: &str) -> Result<PluginInfoDto, UiError> {
        self.plugins_list()
            .into_iter()
            .find(|info| info.id == id)
            .ok_or_else(not_found_plugin)
    }

    // ------------------------------------------------------------ commands

    /// Switches a plugin on after the user confirmed it: only for exactly the package whose
    /// manifest hash the user saw, with the sandbox (or, after a second confirmation, without
    /// it). The process is started right away so a plugin that cannot run is reported now.
    pub fn plugin_activate(
        &self,
        id: &str,
        manifest_hash: &str,
        grant_write: bool,
        unsandboxed: bool,
        confirm_unsandboxed: bool,
    ) -> Result<PluginInfoDto, UiError> {
        plugin_id(id)?;
        if unsandboxed && !confirm_unsandboxed {
            return Err(invalid(MSG_NEEDS_SECOND_CONFIRM));
        }
        self.apply_plugin_failures();
        {
            let _guard = lock(&self.connections.settings_lock);
            let current = self.load_for_update()?;
            let package = verify_package(&self.plugins.runtime.config().base_dir, id)
                .map_err(|e| plugin_error(&e))?;
            if package.manifest_sha256 != manifest_hash {
                return Err(UiError::new("pluginLocked", MSG_LOCKED_CHANGED));
            }
            let existing = current.plugins.iter().find(|entry| entry.id == id);
            if existing.is_some_and(|entry| entry.manifest_sha256 != package.manifest_sha256) {
                return Err(UiError::new("pluginLocked", MSG_LOCKED_PINNED));
            }
            if existing.is_none() && current.plugins.len() >= MAX_PLUGINS {
                return Err(invalid("Es sind schon zu viele Plugins bestätigt"));
            }
            if !unsandboxed && !self.plugins.sandbox().available {
                return Err(plugin_error(&PluginError::SandboxUnavailable));
            }
            let write_granted = grant_write && package.manifest.write;
            let entry = PluginEntry {
                id: id.to_owned(),
                manifest_sha256: package.manifest_sha256.clone(),
                enabled: true,
                unsandboxed,
                write_granted,
                failures: 0,
                disabled_reason: None,
            };
            // The new instance is built and started first; the running one stays untouched
            // until everything else worked.
            let client = PluginClient::open(self.plugins.runtime.clone(), pinned(&entry))
                .map_err(|e| plugin_error(&e))?;
            if unsandboxed {
                self.plugins.runtime.log(&format!(
                    "Plugin {id} wird auf ausdrücklichen Wunsch ohne Sandbox gestartet"
                ));
            }
            client.set_disabled_hook(self.plugins.hook());
            if let Err(error) = client.start() {
                client.shutdown();
                return Err(plugin_error(&error));
            }
            let old_write = existing.is_some_and(|e| e.write_granted);
            let old = self.detach_plugin(id);
            let restore = |this: &Self, old: Option<ActivePlugin>| {
                if let Some(old) = old {
                    if this.register_plugin(&old.client, old_write).is_err() {
                        old.client.shutdown();
                    }
                }
            };
            if let Err(error) = self.register_plugin(&client, write_granted) {
                client.shutdown();
                restore(self, old);
                return Err(error);
            }
            let saved = self.update_plugins_locked(|entries| {
                match entries.iter_mut().find(|e| e.id == id) {
                    Some(existing) => *existing = entry,
                    None => entries.push(entry),
                }
                Ok(())
            });
            if let Err(error) = saved {
                self.unregister_plugin(id);
                restore(self, old);
                return Err(error);
            }
            if let Some(old) = old {
                old.client.shutdown();
            }
        }
        self.plugin_info(id)
    }

    /// Switches the plugin off and withdraws its rights (granted options, sandbox-free mode);
    /// the pinned manifest stays, so it can be switched on again for the same package.
    pub fn plugin_deactivate(&self, id: &str) -> Result<PluginInfoDto, UiError> {
        plugin_id(id)?;
        self.apply_plugin_failures();
        if !self.stored_plugins().iter().any(|entry| entry.id == id) {
            return Err(not_found_plugin());
        }
        // Settings first: if saving fails, the plugin stays active and the file still says so.
        self.update_plugins(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.enabled = false;
                entry.write_granted = false;
                entry.unsandboxed = false;
                entry.failures = 0;
                entry.disabled_reason = None;
            }
            Ok(())
        })?;
        self.unregister_plugin(id);
        self.plugin_info(id)
    }

    /// Removes the pinned manifest and every right: the plugin is "new" again.
    pub fn plugin_trust_reset(&self, id: &str) -> Result<(), UiError> {
        plugin_id(id)?;
        self.apply_plugin_failures();
        let known = self.stored_plugins().iter().any(|entry| entry.id == id)
            || discover(&self.plugins.runtime.config().base_dir)
                .iter()
                .any(|inst| inst.id == id);
        if !known {
            return Err(not_found_plugin());
        }
        self.update_plugins(|entries| {
            entries.retain(|entry| entry.id != id);
            Ok(())
        })?;
        self.unregister_plugin(id);
        Ok(())
    }

    /// The cleaned standard-error text of the running plugin process (plain text, bounded).
    pub fn plugin_diagnostics(&self, id: &str) -> Result<String, UiError> {
        plugin_id(id)?;
        self.apply_plugin_failures();
        let known = self.stored_plugins().iter().any(|entry| entry.id == id)
            || discover(&self.plugins.runtime.config().base_dir)
                .iter()
                .any(|inst| inst.id == id);
        if !known {
            return Err(not_found_plugin());
        }
        let text = lock(&self.plugins.active)
            .get(id)
            .map(|active| active.client.diagnostic())
            .unwrap_or_default();
        Ok(clean_text(&text, MAX_DIAGNOSTIC_CHARS))
    }

    // ------------------------------------------------------------ columns and preview

    /// Values of plugin columns for some files (names, sizes and times go to the plugin, never
    /// paths or content). Directories and unreadable files get `null`.
    pub fn plugin_columns(
        &self,
        paths: &[String],
        column_ids: &[String],
    ) -> Result<Vec<PluginColumnValuesDto>, UiError> {
        if paths.is_empty() || paths.len() > MAX_COLUMN_PATHS {
            return Err(invalid("Zu viele oder keine Dateien für die Spalten"));
        }
        if column_ids.is_empty() || column_ids.len() > MAX_COLUMN_IDS {
            return Err(invalid("Zu viele oder keine Spalten"));
        }
        self.apply_plugin_failures();
        let columns: Vec<(&str, &str)> = column_ids
            .iter()
            .map(|id| match id.split_once(':') {
                Some((plugin, column))
                    if is_valid_plugin_id(plugin) && is_valid_plugin_id(column) =>
                {
                    Ok((plugin, column))
                }
                _ => Err(invalid("Ungültige Spaltenkennung")),
            })
            .collect::<Result<_, _>>()?;
        let vpaths = self.paths(paths)?;
        let mut items: Vec<Option<ColumnItem>> = Vec::with_capacity(vpaths.len());
        for path in &vpaths {
            let entry = self.vfs(path).ok().and_then(|vfs| vfs.stat(path).ok());
            items.push(entry.filter(|e| e.kind == EntryKind::File).map(|entry| {
                ColumnItem {
                    name: path
                        .path()
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    size: Some(entry.size),
                    modified: entry.modified,
                }
            }));
        }
        let present: Vec<usize> = (0..items.len()).filter(|i| items[*i].is_some()).collect();
        let sent: Vec<ColumnItem> = present.iter().filter_map(|i| items[*i].clone()).collect();
        let mut result = Vec::with_capacity(columns.len());
        for ((plugin, column), full_id) in columns.iter().zip(column_ids) {
            let client = self.plugin_client(plugin)?;
            let declared = client
                .manifest()
                .columns
                .as_ref()
                .is_some_and(|list| list.iter().any(|c| c.id == *column));
            if !declared {
                return Err(UiError::new("notFound", "Die Spalte gibt es nicht"));
            }
            let mut values: Vec<Option<String>> = vec![None; items.len()];
            if !sent.is_empty() {
                let answered = client
                    .column_values(column, &sent)
                    .map_err(|e| plugin_error(&e))?;
                for (slot, value) in present.iter().zip(answered) {
                    values[*slot] = value.map(|text| clean_text(&text, MAX_CELL_CHARS));
                }
            }
            result.push(PluginColumnValuesDto {
                column_id: full_id.clone(),
                values,
            });
        }
        Ok(result)
    }

    /// A preview of a file made by a preview plugin: plain text or an image (PNG, JPEG, GIF or
    /// WebP, checked by its first bytes), never markup or a vector image.
    pub fn plugin_preview(
        &self,
        path: &str,
        plugin: Option<&str>,
    ) -> Result<PluginPreviewDto, UiError> {
        self.apply_plugin_failures();
        if let Some(id) = plugin {
            plugin_id(id)?;
        }
        let path = self.path(path)?;
        let name = path
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| UiError::new("invalidPath", "Der Pfad hat keinen Dateinamen"))?;
        let client = match plugin {
            Some(id) => {
                let client = self.plugin_client(id)?;
                if !client.manifest().has_kind(Kind::Preview) {
                    return Err(UiError::new("unsupported", MSG_NO_PREVIEW));
                }
                client
            }
            None => self.preview_plugin_for(&name)?,
        };
        let vfs = self.vfs_for_read(&path)?;
        let size = vfs.stat_file(&path)?.size;
        if size > protocol::MAX_PREVIEW_INPUT as u64 {
            return Err(UiError::new("pluginTooLarge", MSG_TOO_BIG_PREVIEW));
        }
        let data = vfs.read_range(&path, 0, protocol::MAX_PREVIEW_INPUT)?;
        let preview = client
            .preview_render(&name, &data, PREVIEW_ASK_CHARS)
            .map_err(|e| plugin_error(&e))?;
        Ok(match preview {
            Preview::Text(text) => PluginPreviewDto::Text {
                text: clean_multiline(&text, MAX_PREVIEW_CHARS),
            },
            Preview::Image { mime, bytes } => PluginPreviewDto::Image {
                mime,
                data: protocol::encode_bytes(&bytes),
            },
        })
    }

    /// The first active preview plugin that declares the file's extension.
    fn preview_plugin_for(&self, name: &str) -> Result<Arc<PluginClient>, UiError> {
        let extension = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_lowercase())
            .unwrap_or_default();
        let mut candidates: Vec<(String, Arc<PluginClient>)> = lock(&self.plugins.active)
            .iter()
            .map(|(id, active)| (id.clone(), active.client.clone()))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates
            .into_iter()
            .map(|(_, client)| client)
            .find(|client| {
                client
                    .manifest()
                    .preview
                    .as_ref()
                    .is_some_and(|p| !extension.is_empty() && p.extensions.contains(&extension))
            })
            .ok_or_else(|| UiError::new("unsupported", MSG_NO_PREVIEW))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AppState, EventSink};
    use crate::connections::RemoteParts;
    use crate::convert::{ConflictAnswerDto, CoreEventDto, ResolutionDto, UiError};
    use crate::plugins::{PluginInfoDto, PluginParts, PluginPreviewDto, PluginStatusDto};
    use dualfile_core::jobs::JobState;
    use dualfile_core::vfs::VPath;
    use dualfile_plugin::testing::{change_manifest, install_example, InstallSpec};
    use dualfile_plugin::RuntimeConfig;
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct ChannelSink(Mutex<Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    struct Fx {
        state: AppState,
        events: Receiver<CoreEventDto>,
        root: tempfile::TempDir,
    }

    fn config(root: &std::path::Path, simulate_no_sandbox: bool) -> RuntimeConfig {
        let mut config = RuntimeConfig::new(root.join("plugins"));
        config.request_timeout = Duration::from_secs(5);
        config.backoff = Duration::ZERO;
        config.simulate_sandbox_unavailable = simulate_no_sandbox;
        config.log = Some(Arc::new(|_| {}));
        config
    }

    fn build(root: &std::path::Path, simulate: bool) -> (AppState, Receiver<CoreEventDto>) {
        build_with(root, config(root, simulate))
    }

    fn build_with(
        root: &std::path::Path,
        config: RuntimeConfig,
    ) -> (AppState, Receiver<CoreEventDto>) {
        std::fs::create_dir_all(root.join("home")).unwrap();
        let (tx, events) = channel();
        let state = AppState::with_parts(
            Arc::new(ChannelSink(Mutex::new(tx))),
            root.join("home"),
            root.join("config"),
            RemoteParts::production(),
            PluginParts::with_config(config),
        );
        (state, events)
    }

    fn fixture() -> Fx {
        let root = tempfile::tempdir().unwrap();
        // the plugin folder must exist before anything is installed
        std::fs::create_dir_all(root.path().join("plugins")).unwrap();
        let (state, events) = build(root.path(), false);
        Fx {
            state,
            events,
            root,
        }
    }

    /// A fixture that allows only one plugin process at a time, so a second instance of the
    /// same plugin cannot start while the first one runs (a start error that is easy to force).
    fn one_process_fixture() -> Fx {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("plugins")).unwrap();
        let mut cfg = config(root.path(), false);
        cfg.max_processes = 1;
        let (state, events) = build_with(root.path(), cfg);
        Fx {
            state,
            events,
            root,
        }
    }

    /// Makes the settings folder read-only so that saving fails; restores it on drop.
    struct ReadOnlyConfig(PathBuf);
    impl ReadOnlyConfig {
        fn new(path: PathBuf) -> Self {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
            Self(path)
        }
    }
    impl Drop for ReadOnlyConfig {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }

    impl Fx {
        fn base(&self) -> PathBuf {
            self.root.path().join("plugins")
        }
        fn install(&self, spec: &InstallSpec) -> PathBuf {
            install_example(&self.base(), spec)
        }
        fn info(&self, id: &str) -> PluginInfoDto {
            self.state
                .plugins_list()
                .into_iter()
                .find(|info| info.id == id)
                .expect("plugin listed")
        }
        /// Activates like the user would; without a working sandbox on this system (e.g. a
        /// container) the explicit sandbox-free mode is used so the plugin still runs.
        fn activate(&self, id: &str, write: bool) -> Result<PluginInfoDto, UiError> {
            let hash = self.info(id).manifest_hash;
            let free = !self.state.plugins.sandbox().available;
            self.state.plugin_activate(id, &hash, write, free, free)
        }
        fn local(&self, rel: &str, content: &[u8]) -> String {
            let path = self.root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            path.to_string_lossy().into_owned()
        }
        fn finish(&self, id: u64) -> JobState {
            self.state.jobs.wait(id).expect("job").state
        }
        fn stored(&self) -> Vec<dualfile_core::plugins::PluginEntry> {
            self.state.stored_plugins()
        }
    }

    fn kind<T: std::fmt::Debug>(result: Result<T, UiError>) -> String {
        result.expect_err("an error was expected").kind
    }

    fn names(listing: &crate::convert::ListingDto) -> Vec<String> {
        listing.entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn activation_needs_the_matching_hash_and_a_second_confirmation_for_unsandboxed() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        let info = fx.info("ex");
        assert!(matches!(
            info.status,
            PluginStatusDto::New | PluginStatusDto::SandboxUnavailable
        ));
        assert_eq!(info.manifest_hash.len(), 64);
        assert!(!info.enabled && !info.unsandboxed && !info.write_granted);
        // a hash the user did not see
        let wrong = "0".repeat(64);
        assert_eq!(
            kind(fx.state.plugin_activate("ex", &wrong, false, false, false)),
            "pluginLocked"
        );
        // unsandboxed without the second confirmation
        assert_eq!(
            kind(
                fx.state
                    .plugin_activate("ex", &info.manifest_hash, false, true, false)
            ),
            "invalidInput"
        );
        assert!(fx.stored().is_empty(), "nothing is stored after a refusal");
        assert_eq!(fx.state.plugins.runtime.live_processes(), 0);
        assert_eq!(
            kind(
                fx.state
                    .plugin_activate("../x", &wrong, false, false, false)
            ),
            "invalidInput"
        );
        assert_eq!(
            kind(
                fx.state
                    .plugin_activate("nope", &wrong, false, false, false)
            ),
            "pluginNotInstalled"
        );
        eprintln!(
            "Sandbox verfügbar: {}",
            fx.state.plugins.sandbox().available
        );
        let on = fx.activate("ex", false).unwrap();
        assert_eq!(on.status, PluginStatusDto::Enabled);
        assert!(on.enabled && on.filesystem_root.as_deref() == Some("plugin:ex!/"));
        assert_eq!(on.unsandboxed, !fx.state.plugins.sandbox().available);
        assert!(fx.stored()[0].enabled);
    }

    #[test]
    fn without_a_sandbox_the_plugin_is_not_started() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("plugins")).unwrap();
        let (state, _events) = build(root.path(), true);
        install_example(&root.path().join("plugins"), &InstallSpec::normal("ex"));
        let info = state.plugins_list().remove(0);
        assert_eq!(info.status, PluginStatusDto::SandboxUnavailable);
        assert!(!info.sandbox.available);
        let result = state.plugin_activate("ex", &info.manifest_hash, false, false, false);
        assert_eq!(kind(result), "pluginSandboxUnavailable");
        assert_eq!(state.plugins.runtime.live_processes(), 0);
        assert!(state.stored_plugins().is_empty());
        assert_eq!(kind(state.list("plugin:ex!/")), "disconnected");
    }

    #[test]
    fn a_changed_manifest_locks_the_plugin_and_withdraws_its_rights() {
        let fx = fixture();
        let dir = fx.install(&InstallSpec::testbin("ex").writable());
        fx.activate("ex", true).unwrap();
        assert!(fx.info("ex").write_granted);
        fx.state.make_dir("plugin:ex!/", "granted").unwrap();
        change_manifest(&dir);
        let info = fx.info("ex");
        assert_eq!(info.status, PluginStatusDto::Locked);
        assert!(!info.write_granted && !info.enabled);
        let entry = &fx.stored()[0];
        assert!(!entry.enabled && !entry.write_granted);
        assert_eq!(kind(fx.state.list("plugin:ex!/")), "disconnected");
        // the user saw the old hash, the new one: both are refused until trust is reset
        assert_eq!(
            kind(
                fx.state
                    .plugin_activate("ex", &info.manifest_hash, true, false, false)
            ),
            "pluginLocked"
        );
        fx.state.plugin_trust_reset("ex").unwrap();
        assert!(fx.stored().is_empty());
        assert!(matches!(
            fx.info("ex").status,
            PluginStatusDto::New | PluginStatusDto::SandboxUnavailable
        ));
        fx.activate("ex", false).unwrap();
        assert!(!fx.info("ex").write_granted);
        assert_eq!(kind(fx.state.plugin_trust_reset("nope")), "notFound");
    }

    #[test]
    fn a_change_between_program_runs_locks_at_start() {
        let fx = fixture();
        let dir = fx.install(&InstallSpec::normal("ex"));
        fx.activate("ex", false).unwrap();
        // normally activated plugins come back at the next start (unsandboxed ones do not)
        let sandboxed = !fx.stored()[0].unsandboxed;
        let (again, _e) = build(fx.root.path(), false);
        assert_eq!(again.router.has_plugin("ex"), sandboxed);
        assert_eq!(
            again.plugins.runtime.live_processes(),
            0,
            "no process at start"
        );
        if sandboxed {
            assert!(!again.list("plugin:ex!/").unwrap().entries.is_empty());
        }
        change_manifest(&dir);
        let (third, _e) = build(fx.root.path(), false);
        assert!(!third.router.has_plugin("ex"));
        let entry = &third.stored_plugins()[0];
        assert!(!entry.enabled);
        assert_eq!(
            entry.disabled_reason,
            Some(dualfile_core::plugins::DisableReason::Locked)
        );
    }

    #[test]
    fn listing_reading_and_copying_go_through_the_router() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        fx.activate("ex", false).unwrap();
        let root = fx.state.list("plugin:ex!/").unwrap();
        assert_eq!(root.path, "plugin:ex!/");
        assert!(names(&root).contains(&"hello.txt".to_owned()));
        let info = fx.state.file_info("plugin:ex!/hello.txt").unwrap();
        assert_eq!(info.size, 11);
        assert_eq!(
            fx.state.read_block("plugin:ex!/hello.txt", 0, 100).unwrap(),
            b"Hallo Welt\n"
        );
        // copy plugin -> local is an ordinary job
        let dst = fx.root.path().join("out");
        std::fs::create_dir_all(&dst).unwrap();
        let id = fx
            .state
            .copy(&["plugin:ex!/hello.txt".into()], &dst.to_string_lossy())
            .unwrap();
        assert_eq!(fx.finish(id), JobState::Done);
        assert_eq!(
            std::fs::read(dst.join("hello.txt")).unwrap(),
            b"Hallo Welt\n"
        );
        // compare reads only
        let id = fx
            .state
            .compare_dirs(
                "plugin:ex!/docs",
                &dst.to_string_lossy(),
                &crate::compare::CompareOptionsDto::default(),
            )
            .unwrap();
        assert_eq!(fx.finish(id), JobState::Done);
        // plugin text never shows up in an error
        let error = fx.state.list("plugin:ex!/missing").unwrap_err();
        assert_eq!(error.kind, "notFound");
        assert!(!error.message.contains("no such"), "{}", error.message);
        // watching skips plugin locations
        fx.state.watch(&["plugin:ex!/".into()]).unwrap();
        // writing to a plugin location is refused as a target of a sync
        assert_eq!(
            kind(fx.state.pack_archive(
                &["plugin:ex!/hello.txt".into()],
                "plugin:ex!/",
                "a.zip",
                crate::convert::PackFormatDto::Zip
            )),
            "unsupported"
        );
    }

    #[test]
    fn writing_needs_the_grant_and_goes_through_the_usual_jobs() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex").writable());
        fx.activate("ex", false).unwrap();
        let src = fx.local("up/a.txt", b"neu");
        // no grant: refused before any job
        assert_eq!(kind(fx.state.make_dir("plugin:ex!/", "d")), "readOnly");
        assert_eq!(
            kind(fx.state.rename("plugin:ex!/hello.txt", "x.txt")),
            "readOnly"
        );
        assert_eq!(
            kind(fx.state.copy(std::slice::from_ref(&src), "plugin:ex!/docs")),
            "readOnly"
        );
        assert_eq!(
            kind(
                fx.state
                    .move_items(&["plugin:ex!/hello.txt".into()], &fx.local("m/x", b""))
            ),
            "readOnly"
        );
        assert_eq!(
            kind(fx.state.plan_delete(&["plugin:ex!/hello.txt".into()])),
            "readOnly"
        );
        // with the grant
        fx.activate("ex", true).unwrap();
        assert!(fx.info("ex").write_granted);
        fx.state.make_dir("plugin:ex!/", "d").unwrap();
        let id = fx
            .state
            .copy(std::slice::from_ref(&src), "plugin:ex!/d")
            .unwrap();
        assert_eq!(fx.finish(id), JobState::Done);
        assert_eq!(
            fx.state.read_block("plugin:ex!/d/a.txt", 0, 10).unwrap(),
            b"neu"
        );
        // the same name again: the conflict question comes first, nothing is replaced
        let id = fx.state.copy(&[src], "plugin:ex!/d").unwrap();
        let conflict = loop {
            match fx
                .events
                .recv_timeout(Duration::from_secs(5))
                .expect("conflict")
            {
                CoreEventDto::Conflict { conflict } => break conflict,
                _ => continue,
            }
        };
        assert_eq!(conflict.job_id, id);
        fx.state
            .answer_conflict(
                id,
                ConflictAnswerDto {
                    resolution: ResolutionDto::Skip,
                    apply_to_all: false,
                },
            )
            .unwrap();
        fx.finish(id);
        // deleting needs the plan and its token
        let plan = fx
            .state
            .plan_delete(&["plugin:ex!/d/a.txt".into()])
            .unwrap();
        let wrong = fx.state.delete_permanent(
            &crate::convert::DeletePlanInput {
                token: plan.token.clone(),
            },
            "falsch",
        );
        assert_eq!(kind(wrong), "invalidToken");
        let id = fx
            .state
            .delete_permanent(
                &crate::convert::DeletePlanInput {
                    token: plan.token.clone(),
                },
                &plan.token,
            )
            .unwrap();
        assert_eq!(fx.finish(id), JobState::Done);
        assert_eq!(
            kind(fx.state.read_block("plugin:ex!/d/a.txt", 0, 10)),
            "notFound"
        );
        // never a trash, with or without the grant
        assert_eq!(
            kind(fx.state.trash(&["plugin:ex!/d".into()])),
            "unsupported"
        );
        assert_eq!(
            kind(fx.state.restore_from_trash(&["plugin:ex!/d".into()])),
            "unsupported"
        );
        // a second plugin that claims the same archive extension is refused and leaves no trace
        fx.install(&InstallSpec::normal("ro"));
        let hash = fx.info("ro").manifest_hash;
        let free = !fx.state.plugins.sandbox().available;
        assert_eq!(
            kind(fx.state.plugin_activate("ro", &hash, true, free, free)),
            "invalidInput"
        );
        assert!(!fx.state.router.has_plugin("ro"));
        assert!(fx.stored().iter().all(|e| e.id != "ro"));
    }

    #[test]
    fn a_manifest_without_write_stays_read_only_even_with_the_grant() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ro"));
        fx.activate("ro", true).unwrap();
        assert!(!fx.info("ro").write_granted);
        assert!(!fx.info("ro").wants_write);
        assert_eq!(kind(fx.state.make_dir("plugin:ro!/", "x")), "readOnly");
    }

    #[test]
    fn deactivating_removes_the_plugin_and_its_rights() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex").writable());
        fx.activate("ex", true).unwrap();
        let off = fx.state.plugin_deactivate("ex").unwrap();
        assert_eq!(off.status, PluginStatusDto::Disabled);
        assert!(!off.write_granted && !off.unsandboxed);
        assert_eq!(kind(fx.state.list("plugin:ex!/")), "disconnected");
        assert_eq!(fx.state.plugins.runtime.live_processes(), 0);
        assert_eq!(kind(fx.state.plugin_deactivate("nope")), "notFound");
        assert_eq!(kind(fx.state.plugin_deactivate("Bad/Id")), "invalidInput");
    }

    #[test]
    fn a_failed_reactivation_leaves_the_running_plugin_untouched() {
        let fx = one_process_fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        assert!(!names(&fx.state.list("plugin:ex!/").unwrap()).is_empty());
        // the second instance cannot start (process limit): the first one must stay usable
        assert!(fx.activate("ex", false).is_err());
        assert_eq!(fx.state.plugins.runtime.live_processes(), 1);
        assert!(!names(&fx.state.list("plugin:ex!/").unwrap()).is_empty());
        assert!(fx.stored()[0].enabled);
    }

    #[test]
    fn a_failed_save_keeps_the_old_state_on_activate_deactivate_and_reset() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        let before = fx.stored();
        {
            let _ro = ReadOnlyConfig::new(fx.root.path().join("config"));
            if std::fs::write(fx.root.path().join("config").join("probe"), b"x").is_ok() {
                // read-only folders are not enforced here (e.g. running as root)
                return;
            }
            assert!(fx.activate("ex", false).is_err());
            assert_eq!(fx.state.plugins.runtime.live_processes(), 1);
            assert!(!names(&fx.state.list("plugin:ex!/").unwrap()).is_empty());
            assert!(fx.state.plugin_deactivate("ex").is_err());
            assert!(fx.state.plugin_trust_reset("ex").is_err());
            assert_eq!(fx.state.plugins.runtime.live_processes(), 1);
            assert!(!names(&fx.state.list("plugin:ex!/").unwrap()).is_empty());
        }
        assert_eq!(fx.stored(), before);
    }

    #[test]
    fn reactivating_replaces_the_instance_without_a_leftover_process() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        fx.activate("ex", false).unwrap();
        assert_eq!(fx.state.plugins.runtime.live_processes(), 1);
        assert!(!names(&fx.state.list("plugin:ex!/").unwrap()).is_empty());
        fx.state.plugin_deactivate("ex").unwrap();
        assert_eq!(fx.state.plugins.runtime.live_processes(), 0);
    }

    #[test]
    fn a_crash_loop_switches_the_plugin_off() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        let client = fx.state.plugin_client("ex").unwrap();
        for _ in 0..3 {
            let _ = client.raw_call("debug_misbehave", serde_json::json!({"mode": "crash"}));
        }
        assert!(client.health().disabled);
        let info = fx.info("ex");
        assert_eq!(info.status, PluginStatusDto::Failed);
        assert_eq!(info.disabled_reason, Some("failures"));
        assert!(!info.enabled);
        assert_eq!(kind(fx.state.list("plugin:ex!/")), "disconnected");
        let entry = &fx.stored()[0];
        assert!(!entry.enabled && entry.failures >= 3);
        // the user can switch it on again
        fx.activate("ex", false).unwrap();
        assert_eq!(fx.info("ex").status, PluginStatusDto::Enabled);
    }

    #[test]
    fn a_plugin_that_is_busy_or_broken_gives_fixed_errors() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        let client = fx.state.plugin_client("ex").unwrap();
        let _ = client.raw_call("debug_misbehave", serde_json::json!({"mode": "badjson"}));
        // the broken process counts as one failure and is replaced by a fresh one
        assert_eq!(client.health().failures, 1);
        assert!(fx.state.list("plugin:ex!/").is_ok());
        assert_eq!(client.health().failures, 0);
        // a plugin error answer carries no plugin text into the UI error
        let error = fx.state.read_block("plugin:ex!/docs", 0, 10).unwrap_err();
        assert!(!error.message.contains("is a folder"), "{}", error.message);
    }

    #[test]
    fn packer_archives_open_through_the_archive_path_with_the_usual_rules() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        fx.activate("ex", false).unwrap();
        let file = fx.local("a.xyz", b"docs/a.txt\tAAA\n../evil.txt\tX\nb.txt\tBB\n");
        let outer = VPath::local(&file).unwrap();
        let root = dualfile_core::format_ipc_path(&VPath::archive(outer.clone(), "/").unwrap());
        let listing = fx.state.list(&root).unwrap();
        let listed = names(&listing);
        assert!(listed.contains(&"docs".to_owned()) && listed.contains(&"b.txt".to_owned()));
        let bad: Vec<_> = listing
            .entries
            .iter()
            .filter(|e| e.error.is_some())
            .collect();
        assert_eq!(bad.len(), 1);
        assert!(bad[0].name.starts_with("[unsicher"));
        let inner =
            dualfile_core::format_ipc_path(&VPath::archive(outer.clone(), "/docs/a.txt").unwrap());
        assert_eq!(fx.state.read_block(&inner, 0, 10).unwrap(), b"AAA");
        let bad_path = dualfile_core::format_ipc_path(
            &VPath::archive(outer, &format!("/{}", bad[0].name)).unwrap(),
        );
        assert!(fx.state.read_block(&bad_path, 0, 10).is_err());
        // extracting is an ordinary job; writing into the archive is refused
        let out = fx.root.path().join("extract");
        std::fs::create_dir_all(&out).unwrap();
        let id = fx.state.copy(&[inner], &out.to_string_lossy()).unwrap();
        assert_eq!(fx.finish(id), JobState::Done);
        assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"AAA");
        assert_eq!(kind(fx.state.make_dir(&root, "n")), "readOnly");
        // once the plugin is off the format is gone
        fx.state.plugin_deactivate("ex").unwrap();
        assert!(fx.state.list(&root).is_err());
        // an archive above the plugin's limit gets a clear message
        let big = fx.local(
            "big.xyz",
            &vec![b'x'; dualfile_plugin::protocol::MAX_PACKER_INPUT + 1],
        );
        fx.activate("ex", false).unwrap();
        let big_root = dualfile_core::format_ipc_path(
            &VPath::archive(VPath::local(&big).unwrap(), "/").unwrap(),
        );
        let error = fx.state.list(&big_root).unwrap_err();
        assert_eq!(error.kind, "archive");
        assert!(error.message.contains("zu groß"));
    }

    #[test]
    fn columns_run_in_bounded_batches_and_cells_are_text() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        fx.activate("ex", false).unwrap();
        let file = fx.local("data/lang.txt", b"x");
        let dir = fx.root.path().join("data").to_string_lossy().into_owned();
        let paths = vec![
            file,
            "plugin:ex!/hello.txt".to_owned(),
            dir,
            "/does/not/exist".to_owned(),
        ];
        let result = fx
            .state
            .plugin_columns(&paths, &["ex:len".into(), "ex:upper".into()])
            .unwrap();
        assert_eq!(result[0].column_id, "ex:len");
        assert_eq!(
            result[0].values,
            vec![Some("8".into()), Some("9".into()), None, None]
        );
        assert_eq!(result[1].values[1].as_deref(), Some("HELLO.TXT"));
        // limits and unknown columns
        let many: Vec<String> = (0..=500).map(|i| format!("/x/{i}")).collect();
        assert_eq!(
            kind(fx.state.plugin_columns(&many, &["ex:len".into()])),
            "invalidInput"
        );
        assert_eq!(
            kind(fx.state.plugin_columns(&[], &["ex:len".into()])),
            "invalidInput"
        );
        let ids: Vec<String> = (0..9).map(|_| "ex:len".to_owned()).collect();
        assert_eq!(kind(fx.state.plugin_columns(&paths, &ids)), "invalidInput");
        assert_eq!(
            kind(fx.state.plugin_columns(&paths, &["ex:nope".into()])),
            "notFound"
        );
        assert_eq!(
            kind(fx.state.plugin_columns(&paths, &["off:len".into()])),
            "pluginDisabled"
        );
        assert_eq!(
            kind(fx.state.plugin_columns(&paths, &["bad".into()])),
            "invalidInput"
        );
        assert_eq!(
            kind(fx.state.plugin_columns(&paths, &["../x:len".into()])),
            "invalidInput"
        );
    }

    #[test]
    fn previews_are_text_or_checked_images_and_never_markup() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        let text = fx.local("p/a.txt", b"Erste Zeile\nzweite\n");
        let png = fx.local("p/b.png", b"irrelevant");
        let other = fx.local("p/c.bin", b"zzz");
        assert_eq!(
            kind(fx.state.plugin_preview(&text, None)),
            "unsupported",
            "no plugin yet"
        );
        fx.activate("ex", false).unwrap();
        match fx.state.plugin_preview(&text, None).unwrap() {
            PluginPreviewDto::Text { text } => assert_eq!(text, "Vorschau: Erste Zeile"),
            other => panic!("{other:?}"),
        }
        match fx.state.plugin_preview(&png, Some("ex")).unwrap() {
            PluginPreviewDto::Image { mime, data } => {
                assert_eq!(mime, "image/png");
                assert!(!data.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(kind(fx.state.plugin_preview(&other, None)), "unsupported");
        assert_eq!(
            kind(fx.state.plugin_preview(&text, Some("off"))),
            "pluginDisabled"
        );
        assert_eq!(
            kind(fx.state.plugin_preview(&text, Some("Bad"))),
            "invalidInput"
        );
        // a file the plugin may not be fed
        let big = fx.local(
            "p/big.txt",
            &vec![b'a'; dualfile_plugin::protocol::MAX_PREVIEW_INPUT + 1],
        );
        assert_eq!(kind(fx.state.plugin_preview(&big, None)), "pluginTooLarge");
        // a folder is no file
        assert!(fx
            .state
            .plugin_preview(&fx.root.path().join("p").to_string_lossy(), Some("ex"))
            .is_err());
    }

    #[test]
    fn markup_and_vector_images_from_a_plugin_are_refused() {
        for mode in ["html_preview_always", "svg_preview_always"] {
            let fx = fixture();
            fx.install(&InstallSpec::testbin("ex").mode(mode));
            fx.activate("ex", false).unwrap();
            let text = fx.local("a.txt", b"x");
            let error = fx.state.plugin_preview(&text, Some("ex")).unwrap_err();
            assert_eq!(error.kind, "pluginProtocol", "{mode}");
            assert!(!error.message.contains("script") && !error.message.contains("svg"));
        }
    }

    #[test]
    fn diagnostics_are_bounded_plain_text() {
        let fx = fixture();
        fx.install(&InstallSpec::testbin("ex"));
        fx.activate("ex", false).unwrap();
        assert_eq!(fx.state.plugin_diagnostics("ex").unwrap(), "");
        let client = fx.state.plugin_client("ex").unwrap();
        let _ = client.raw_call(
            "debug_misbehave",
            serde_json::json!({"mode": "stderr_flood"}),
        );
        let text = fx.state.plugin_diagnostics("ex").unwrap();
        assert!(text.chars().count() <= crate::plugins::MAX_DIAGNOSTIC_CHARS);
        assert!(!text.chars().any(char::is_control));
        assert!(fx.info("ex").diagnostic.chars().count() <= crate::plugins::MAX_DIAGNOSTIC_CHARS);
        assert_eq!(kind(fx.state.plugin_diagnostics("nope")), "notFound");
    }

    #[test]
    fn saving_the_ui_settings_keeps_the_plugin_list_and_the_file_round_trips() {
        let fx = fixture();
        fx.install(&InstallSpec::normal("ex"));
        fx.activate("ex", false).unwrap();
        let before = fx.stored();
        let loaded = fx.state.load_settings();
        fx.state.save_settings(&loaded.settings).unwrap();
        assert_eq!(fx.stored(), before);
        let (reloaded, outcome) = dualfile_core::settings::load(fx.root.path().join("config"));
        assert_eq!(outcome, dualfile_core::settings::LoadOutcome::Loaded);
        assert_eq!(reloaded.version, dualfile_core::settings::SETTINGS_VERSION);
        assert_eq!(reloaded.plugins, before);
        // a plugin location is not kept in the saved tabs
        let mut bad = fx.state.load_settings().settings;
        bad.session.windows[0].tabs[0].path = "plugin:ex!/".into();
        assert_eq!(kind(fx.state.save_settings(&bad)), "invalidSettings");
    }

    #[test]
    fn plugin_paths_that_are_forged_or_unknown_are_refused() {
        let fx = fixture();
        for raw in ["plugin:A!/x", "plugin:a!/../x", "plugin:a!x", "plugin:"] {
            assert_eq!(kind(fx.state.list(raw)), "invalidPath", "{raw}");
        }
        assert_eq!(kind(fx.state.list("plugin:unknown!/")), "disconnected");
    }
}
