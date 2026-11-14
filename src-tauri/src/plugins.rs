//! Plugins on the Tauri side: the DTOs of the plugin commands, the cleaning of everything a
//! plugin supplies as text, and the registry of the plugins that are switched on.
//!
//! A plugin is never trusted. Text that comes from it (name, version, column titles, cell
//! values, preview text, diagnosis) reaches the UI only cleaned (control characters replaced)
//! and cut to a fixed length, and only as plain text. Errors are built from fixed phrases
//! (`convert::plugin_error`); no plugin text is part of them.

use crate::convert::UiError;
use dualfile_plugin::client::DisabledCause;
use dualfile_plugin::sandbox::{self, SandboxInfo};
use dualfile_plugin::{PluginClient, PluginError, PluginRuntime, RuntimeConfig};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Most files one `plugin_columns` call may ask about.
pub const MAX_COLUMN_PATHS: usize = 500;
/// Most columns one `plugin_columns` call may ask for.
pub const MAX_COLUMN_IDS: usize = 8;
/// Longest cell text handed to the UI, in characters.
pub const MAX_CELL_CHARS: usize = 256;
/// Longest plugin name / version / column title / diagnosis handed to the UI, in characters.
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_VERSION_CHARS: usize = 32;
pub const MAX_TITLE_CHARS: usize = 40;
pub const MAX_DIAGNOSTIC_CHARS: usize = 512;
/// Longest preview text handed to the UI, in characters.
pub const MAX_PREVIEW_CHARS: usize = 64 * 1024;

// ---------------------------------------------------------------- cleaning

/// Plugin text as plain text for the UI: control characters become U+FFFD, at most `max_chars`.
pub fn clean_text(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .take(max_chars)
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// Like `clean_text`, for multi-line text: line feed and tab stay.
pub fn clean_multiline(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .take(max_chars)
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

// ---------------------------------------------------------------- DTOs

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginStatusDto {
    New,
    Enabled,
    Disabled,
    Locked,
    SandboxUnavailable,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSandboxDto {
    pub available: bool,
    pub landlock_abi: u32,
    pub landlock_net: bool,
    pub landlock_scoped: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginColumnDto {
    /// `<plugin id>:<column id>`
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfoDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kinds: Vec<&'static str>,
    pub status: PluginStatusDto,
    pub wants_write: bool,
    pub write_granted: bool,
    pub unsandboxed: bool,
    pub enabled: bool,
    pub manifest_hash: String,
    pub sandbox: PluginSandboxDto,
    pub failures: u32,
    pub disabled_reason: Option<&'static str>,
    pub problem: Option<String>,
    pub diagnostic: String,
    pub columns: Vec<PluginColumnDto>,
    pub packer_extensions: Vec<String>,
    pub filesystem_root: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginColumnValuesDto {
    pub column_id: String,
    pub values: Vec<Option<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PluginPreviewDto {
    Text { text: String },
    Image { mime: &'static str, data: String },
}

impl From<SandboxInfo> for PluginSandboxDto {
    fn from(info: SandboxInfo) -> Self {
        Self {
            available: true,
            landlock_abi: info.landlock_abi,
            landlock_net: info.landlock_net,
            landlock_scoped: info.landlock_scoped,
        }
    }
}

// ---------------------------------------------------------------- state

/// A plugin that is switched on: its client and what was registered for it.
pub struct ActivePlugin {
    pub client: Arc<PluginClient>,
    pub filesystem: bool,
}

/// How the app reaches the plugins: where they are installed and how their processes run.
pub struct PluginParts {
    pub config: RuntimeConfig,
}

impl PluginParts {
    /// The real plugin folder (`~/.local/share/dualfile/plugins`).
    pub fn production() -> Self {
        let base = dualfile_plugin::package::default_plugin_dir()
            .unwrap_or_else(|| PathBuf::from("/nonexistent/dualfile-plugins"));
        Self {
            config: RuntimeConfig::new(base),
        }
    }

    /// Test seam: any plugin folder and runtime settings.
    #[cfg(test)]
    pub fn with_config(config: RuntimeConfig) -> Self {
        Self { config }
    }
}

pub struct PluginState {
    pub runtime: Arc<PluginRuntime>,
    pub active: Mutex<HashMap<String, ActivePlugin>>,
    /// Plugins that switched themselves off (set by the client's hook, applied later by
    /// `AppState::apply_plugin_failures`; the hook itself must not do any real work).
    tripped: Arc<Mutex<Vec<(String, DisabledCause)>>>,
    tripped_flag: Arc<AtomicBool>,
}

impl PluginState {
    pub fn new(parts: PluginParts) -> Self {
        Self {
            runtime: PluginRuntime::new(parts.config),
            active: Mutex::default(),
            tripped: Arc::default(),
            tripped_flag: Arc::default(),
        }
    }

    /// The function a client calls when it switches itself off.
    pub fn hook(&self) -> dualfile_plugin::DisabledHook {
        let tripped = Arc::clone(&self.tripped);
        let flag = Arc::clone(&self.tripped_flag);
        Arc::new(move |id, cause| {
            if let Ok(mut list) = tripped.lock() {
                list.push((id.to_owned(), cause));
                flag.store(true, Ordering::SeqCst);
            }
        })
    }

    /// Takes what the hooks noted since the last call.
    pub fn take_tripped(&self) -> Vec<(String, DisabledCause)> {
        if !self.tripped_flag.swap(false, Ordering::SeqCst) {
            return Vec::new();
        }
        self.tripped
            .lock()
            .map(|mut list| std::mem::take(&mut *list))
            .unwrap_or_default()
    }

    /// Whether a plugin could be started with the sandbox on this system right now.
    pub fn sandbox(&self) -> PluginSandboxDto {
        let none = PluginSandboxDto {
            available: false,
            landlock_abi: 0,
            landlock_net: false,
            landlock_scoped: false,
        };
        if self.runtime.config().simulate_sandbox_unavailable {
            return none;
        }
        sandbox::probe().map(Into::into).unwrap_or(none)
    }
}

/// A plugin failure as the UI sees it: fixed kind and fixed text, never plugin text.
pub fn plugin_error(error: &PluginError) -> UiError {
    UiError::new(error.ui_kind(), error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_text_is_cleaned_and_cut() {
        assert_eq!(
            clean_text("a\u{1}b\nc\u{7f}", 10),
            "a\u{fffd}b\u{fffd}c\u{fffd}"
        );
        assert_eq!(clean_text("ääääää", 3), "äää");
        assert_eq!(
            clean_multiline("a\u{1}b\nc\td\r", 20),
            "a\u{fffd}b\nc\td\u{fffd}"
        );
        assert!(
            clean_text(&"x".repeat(10_000), MAX_CELL_CHARS)
                .chars()
                .count()
                <= MAX_CELL_CHARS
        );
    }

    #[test]
    fn plugin_errors_have_fixed_kinds_and_no_plugin_text() {
        let reported = PluginError::Reported {
            code: dualfile_plugin::ErrorCode::Other,
            diagnostic: dualfile_plugin::Diagnostic::new("SECRET-TOKEN rm -rf"),
        };
        let ui = plugin_error(&reported);
        assert_eq!(ui.kind, "pluginFailed");
        assert!(!ui.message.contains("SECRET"));
        for (error, kind) in [
            (PluginError::Locked, "pluginLocked"),
            (PluginError::SandboxUnavailable, "pluginSandboxUnavailable"),
            (PluginError::Timeout, "pluginTimeout"),
            (PluginError::Protocol("x"), "pluginProtocol"),
            (PluginError::Crashed, "pluginCrashed"),
            (PluginError::Disabled, "pluginDisabled"),
            (PluginError::TooManyProcesses, "pluginBusy"),
            (PluginError::Backoff, "pluginBusy"),
            (PluginError::NotInstalled, "pluginNotInstalled"),
            (PluginError::Manifest("x"), "pluginUnsafe"),
            (PluginError::UnsafePackage("x"), "pluginUnsafe"),
            (PluginError::Spawn, "pluginFailed"),
            (PluginError::Root, "pluginFailed"),
        ] {
            assert_eq!(plugin_error(&error).kind, kind);
        }
    }
}
