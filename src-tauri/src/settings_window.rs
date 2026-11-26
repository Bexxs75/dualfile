//! Least-privilege settings relay. Cache, session, sink and delivery share one lock.
use crate::connection_editor::{
    append_appearance, only_label, open_child, sanitize_title, Appearance,
};
use crate::convert::{SettingsDto, UiError};
use dualfile_core::settings::WindowSize;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, Emitter, Manager};

pub(crate) const SETTINGS_LABEL: &str = "settings";
type Sink = Box<dyn Fn(&Value) -> bool + Send>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AfterClose {
    #[default]
    None,
    Main,
    Hide,
    Restart,
    /// Closed by the main window's own flush: nothing follows, but the user is not to be asked
    /// anything (the main window only waits a few seconds).
    Quiet,
}
/// Standard size and minimum size of the settings window (logical px).
const STANDARD_SIZE: (f64, f64) = (720.0, 640.0);
const MIN_SIZE: (f64, f64) = (520.0, 420.0);
/// Space kept free around a remembered size on the screen: title bar, panels, borders.
const SCREEN_MARGIN: (f64, f64) = (16.0, 96.0);
#[derive(Default)]
struct RelayState {
    latest: Option<Value>,
    sink: Option<Sink>,
    session: String,
    ack: u64,
    closing: bool,
    after: AfterClose,
    close_generation: u64,
    unresponsive: bool,
    destroying: bool,
    /// The client shows a question before closing; the close timeout waits (see `hold`).
    holding: bool,
}
#[derive(Default)]
pub(crate) struct SettingsRelay {
    state: Mutex<RelayState>,
}
fn invalid() -> UiError {
    UiError::new("invalidInput", "Ungültige Einstellungsnachricht")
}
impl RelayState {
    fn deliver(&mut self, value: Value) {
        if self.sink.as_ref().is_some_and(|sink| !sink(&value)) {
            self.sink = None;
        }
    }
    fn abandon_close(&mut self) {
        if self.destroying {
            return;
        }
        self.closing = false;
        self.holding = false;
        self.after = AfterClose::None;
        self.close_generation += 1;
    }
    fn request_close(&mut self, after: AfterClose) -> bool {
        if self.destroying {
            return false;
        }
        if !self.closing {
            self.after = after;
            self.close_generation += 1;
        } else if after != AfterClose::None {
            self.after = after;
        }
        self.closing = true;
        let session = self.session.clone();
        if !session.is_empty() && self.sink.is_some() && !self.unresponsive {
            let interactive = self.after == AfterClose::None;
            self.deliver(json!({"kind":"close", "session":session, "interactive":interactive}));
            if self.sink.is_some() {
                return true;
            }
        }
        self.destroying = true;
        false
    }
    fn expire_close(&mut self, generation: u64) -> bool {
        // While the client asks the user something, an answer may take as long as it takes.
        if !self.closing || self.destroying || self.holding || self.close_generation != generation {
            return false;
        }
        self.abandon_close();
        self.unresponsive = true;
        let session = self.session.clone();
        self.deliver(json!({"kind":"timeout", "session":session}));
        true
    }
    fn authorize_close(
        &mut self,
        session: &str,
        seq: u64,
        force: bool,
        cancel: bool,
    ) -> Result<bool, UiError> {
        if cancel {
            if session != self.session {
                return Err(invalid());
            }
            self.abandon_close();
            return Ok(false);
        }
        if !force && (session.is_empty() || session != self.session || seq == 0 || seq != self.ack)
        {
            self.abandon_close();
            return Err(invalid());
        }
        if force {
            self.abandon_close();
        }
        self.destroying = true;
        Ok(true)
    }
    fn take_destroyed(&mut self) -> (String, u64, AfterClose) {
        let result = (std::mem::take(&mut self.session), self.ack, self.after);
        self.sink = None;
        self.ack = 0;
        self.closing = false;
        self.holding = false;
        self.after = AfterClose::None;
        self.unresponsive = false;
        self.destroying = false;
        self.close_generation += 1;
        result
    }
}

impl SettingsRelay {
    fn attach(&self, sink: Sink) -> String {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Each attachment is a new client sequence space, including a webview reload.
        state.session = format!(
            "{:016x}{:016x}",
            std::collections::hash_map::RandomState::new().hash_one(0),
            std::collections::hash_map::RandomState::new().hash_one(1)
        );
        state.ack = 0;
        state.unresponsive = false;
        state.holding = false;
        state.sink = Some(sink);
        let session = state.session.clone();
        state.deliver(json!({"kind":"session", "session":session}));
        if let Some(mut snapshot) = state.latest.clone() {
            if snapshot["session"] != session {
                snapshot["session"] = json!(session);
                snapshot["ack"] = json!(0);
                snapshot["errors"] = json!([]);
            }
            state.deliver(json!({"kind":"snapshot", "snapshot":snapshot}));
        }
        if state.closing {
            let interactive = state.after == AfterClose::None;
            state.deliver(json!({"kind":"close", "session":session, "interactive":interactive}));
        }
        session
    }
    fn push(&self, snapshot: Value) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.session.is_empty() && snapshot["session"] != state.session {
            return;
        }
        let ack = snapshot["ack"].as_u64().unwrap_or(0);
        if ack < state.ack {
            return;
        }
        state.ack = ack;
        state.latest = Some(snapshot.clone());
        state.deliver(json!({"kind":"snapshot", "snapshot":snapshot}));
    }
    pub(crate) fn destroyed(&self, app: &tauri::AppHandle) {
        let (session, seq, after) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_destroyed();
        let _ = app.emit_to(
            "main",
            "settings-window",
            json!({"kind":"closed", "session":session, "seq":seq}),
        );
        // Window removal is asynchronous: resume main only after the child label is gone.
        run_after(app, after);
    }
    #[cfg(test)]
    fn detach(&self) {
        self.state.lock().unwrap().take_destroyed();
    }
}

pub(crate) fn settings_query(
    tab: Option<&str>,
    appearance: &Appearance,
) -> Result<String, UiError> {
    if tab.is_some_and(|t| {
        ![
            "allgemein",
            "darstellung",
            "dateiliste",
            "seitenleiste",
            "terminal",
            "kopieren",
            "tastatur",
            "info",
            "appearance",
            "system",
            "keyboard",
        ]
        .contains(&t)
    }) {
        return Err(invalid());
    }
    let mut url = tauri::Url::parse("http://localhost/?view=settings").map_err(|_| invalid())?;
    if let Some(tab) = tab {
        url.query_pairs_mut().append_pair("tab", tab);
    }
    append_appearance(&mut url, appearance)?;
    Ok(format!("index.html?{}", url.query().unwrap_or_default()))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent_automatic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    density: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_size: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_weight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_parent_entry: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extension_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_permissions_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_uid_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_gid_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_tint_strength: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_color_left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_color_right: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_colors: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verify_policy: Option<dualfile_core::ops::VerifyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tray_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_updates_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keybindings: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_collapsed: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_hidden: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_discovery: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings_window_size_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings_window_size: Option<WindowSize>,

    last_update_check: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent_automatic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    density: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_size: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_weight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_parent_entry: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extension_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_permissions_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_uid_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_gid_column: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_tint_strength: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_color_left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_color_right: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_colors: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verify_policy: Option<dualfile_core::ops::VerifyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tray_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_updates_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keybindings: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_collapsed: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_hidden: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_discovery: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings_window_size_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings_window_size: Option<WindowSize>,
}
impl SettingsPatch {
    fn validate(&self) -> Result<(), UiError> {
        // Using the save validator makes the patch/save invariant hold as rules evolve.
        let mut settings = SettingsDto::from(&dualfile_core::settings::Settings::default());
        if let Some(value) = &self.theme {
            settings.theme = value.clone();
        }
        if let Some(value) = &self.accent {
            settings.accent = value.clone();
        }
        if let Some(value) = &self.accent_automatic {
            settings.accent_automatic = *value;
        }
        if let Some(value) = &self.density {
            settings.density = value.clone();
        }
        if let Some(value) = &self.font_size {
            settings.font_size = *value;
        }
        if let Some(value) = &self.font_weight {
            settings.font_weight = value.clone();
        }
        if let Some(value) = &self.language {
            settings.language = value.clone();
        }
        if let Some(value) = &self.show_parent_entry {
            settings.show_parent_entry = *value;
        }
        if let Some(value) = self.show_permissions_column {
            settings.show_permissions_column = value;
        }
        if let Some(value) = self.show_uid_column {
            settings.show_uid_column = value;
        }
        if let Some(value) = self.show_gid_column {
            settings.show_gid_column = value;
        }
        if let Some(value) = self.pane_tint_strength {
            settings.pane_tint_strength = value;
        }
        if let Some(value) = &self.extension_column {
            settings.extension_column = *value;
        }
        if let Some(value) = &self.pane_color_left {
            settings.pane_color_left = value.clone();
        }
        if let Some(value) = &self.pane_color_right {
            settings.pane_color_right = value.clone();
        }
        if let Some(value) = &self.terminal_colors {
            settings.terminal_colors = value.clone();
        }
        if let Some(value) = &self.terminal_bg {
            settings.terminal_bg = value.clone();
        }
        if let Some(value) = &self.terminal_fg {
            settings.terminal_fg = value.clone();
        }
        if let Some(value) = &self.verify_policy {
            settings.verify_policy = *value;
        }
        if let Some(value) = &self.terminal_shell {
            settings.terminal_shell = value.clone();
        }
        if let Some(value) = &self.tray_enabled {
            settings.tray_enabled = *value;
        }
        if let Some(value) = &self.check_updates_on_start {
            settings.check_updates_on_start = *value;
        }
        if let Some(value) = &self.sidebar_mode {
            settings.sidebar_mode = value.clone();
        }
        if let Some(value) = &self.sidebar_width {
            settings.sidebar_width = *value;
        }
        if let Some(value) = &self.sidebar_collapsed {
            settings.sidebar_collapsed = value.clone();
        }
        if let Some(value) = self.sidebar_discovery {
            settings.sidebar_discovery = value;
        }
        if let Some(value) = &self.sidebar_hidden {
            settings.sidebar_hidden = value.clone();
        }
        if let Some(value) = &self.settings_window_size_mode {
            settings.settings_window_size_mode = value.clone();
        }
        if let Some(value) = self.settings_window_size {
            settings.settings_window_size = value;
        }
        if let Some(value) = &self.keybindings {
            settings.keybindings = value.clone();
        }
        crate::validate::validate_settings(&settings)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SettingsWindowMessage {
    Patch {
        session: String,
        seq: u64,
        patch: Box<SettingsPatch>,
    },
    CheckUpdates {
        session: String,
        seq: u64,
    },
    SetTray {
        session: String,
        seq: u64,
        enabled: bool,
    },
    SetStartMenu {
        session: String,
        seq: u64,
        enabled: bool,
    },
    RefreshStatus {
        session: String,
        seq: u64,
    },
    SetAutostart {
        session: String,
        seq: u64,
        mode: crate::autostart::Mode,
    },
    Flush {
        session: String,
        seq: u64,
    },
}
impl SettingsWindowMessage {
    fn identity(&self) -> (&str, u64) {
        match self {
            Self::Patch { session, seq, .. }
            | Self::CheckUpdates { session, seq }
            | Self::RefreshStatus { session, seq }
            | Self::SetTray { session, seq, .. }
            | Self::SetStartMenu { session, seq, .. }
            | Self::SetAutostart { session, seq, .. }
            | Self::Flush { session, seq } => (session, *seq),
        }
    }
}
#[tauri::command]
pub async fn settings_window_open(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    tab: Option<String>,
    title: String,
    appearance: Appearance,
    size: Option<WindowSize>,
) -> Result<bool, UiError> {
    only_label(window.label(), "main")?;
    if app
        .state::<Arc<SettingsRelay>>()
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .latest
        .as_ref()
        .is_some_and(|snapshot| snapshot["system"]["installing"] == true)
    {
        return Err(UiError::new(
            "updateBusy",
            "Update installation in progress",
        ));
    }
    let page = settings_query(tab.as_deref(), &appearance)?;
    // A remembered size (never a position) is limited to the screen the main window is on.
    let wanted = size.filter(|size| *size != WindowSize::default() && size.is_valid());
    let initial = wanted.map_or(STANDARD_SIZE, |size| {
        fit_size((size.width, size.height), MIN_SIZE, screen_area(&window))
    });
    open_child(
        &app,
        &window,
        SETTINGS_LABEL,
        page,
        title,
        initial,
        MIN_SIZE,
    )
}
/// Usable area (logical px) of the monitor the window is on; `None` if no monitor is known.
fn screen_area(window: &tauri::WebviewWindow) -> Option<(f64, f64)> {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())?;
    let area = monitor.work_area().size;
    let scale = monitor.scale_factor();
    (scale > 0.0 && area.width > 0 && area.height > 0).then(|| {
        (
            f64::from(area.width) / scale,
            f64::from(area.height) / scale,
        )
    })
}
/// Limits a remembered size to the visible screen area (smaller screen, monitor gone) and never
/// below the window's minimum size; the minimum wins if the screen is even smaller.
pub(crate) fn fit_size(
    wanted: (u32, u32),
    min: (f64, f64),
    area: Option<(f64, f64)>,
) -> (f64, f64) {
    let (mut width, mut height) = (f64::from(wanted.0), f64::from(wanted.1));
    if let Some((area_width, area_height)) = area {
        width = width.min(area_width - SCREEN_MARGIN.0);
        height = height.min(area_height - SCREEN_MARGIN.1);
    }
    (width.max(min.0).round(), height.max(min.1).round())
}
/// True while the settings window is maximized, fullscreen or minimized. Its size is then the
/// screen's or meaningless, not a size the user chose, so nothing is asked or remembered.
/// Only this flag comes from the native side; the size itself is measured in the web view
/// (see `src/app/settingsWindow.ts`), because the size the toolkit reports is not the size the
/// window was given: on GNOME/Wayland (GTK3 with client-side decorations) a window created with
/// 720 x 640 reports 772 x 729 in its configure event, and `tao` feeds both `Resized` and
/// `inner_size()` from exactly that event.
#[tauri::command]
pub fn settings_window_zoomed(window: tauri::WebviewWindow) -> Result<bool, UiError> {
    only_label(window.label(), SETTINGS_LABEL)?;
    Ok(window.is_maximized().unwrap_or(false)
        || window.is_fullscreen().unwrap_or(false)
        || window.is_minimized().unwrap_or(false))
}
/// The client is about to ask the user something before it closes (`on`), or is done asking.
/// Without this the five-second close timeout would expire during the question.
#[tauri::command]
pub fn settings_window_hold(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    relay: tauri::State<'_, Arc<SettingsRelay>>,
    session: String,
    on: bool,
) -> Result<(), UiError> {
    only_label(window.label(), SETTINGS_LABEL)?;
    let rearm = {
        let mut state = relay.state.lock().unwrap_or_else(|e| e.into_inner());
        if session.is_empty() || session != state.session {
            return Err(invalid());
        }
        state.holding = on;
        (!on && state.closing && !state.destroying).then_some(state.close_generation)
    };
    // A timer that ran out during the question did nothing; start a new one for the rest.
    if let Some(generation) = rearm {
        arm_close_timer(&app, Arc::clone(relay.inner()), generation);
    }
    Ok(())
}
#[tauri::command]
pub fn settings_window_attach(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    relay: tauri::State<'_, Arc<SettingsRelay>>,
    channel: Channel<Value>,
) -> Result<(), UiError> {
    only_label(window.label(), SETTINGS_LABEL)?;
    let session = relay.attach(Box::new(move |value| channel.send(value.clone()).is_ok()));
    app.emit_to(
        "main",
        "settings-window",
        json!({"kind":"session", "session":session, "seq":0}),
    )
    .map_err(|_| UiError::internal())
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushControl {
    #[serde(default)]
    cancel_close: bool,
    error: Option<String>,
}
fn validate_projection(value: &Value) -> Result<(), UiError> {
    let _: SettingsProjection = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    Ok(())
}
#[tauri::command]
pub fn settings_window_push(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    relay: tauri::State<'_, Arc<SettingsRelay>>,
    snapshot: Value,
    title: String,
    close: bool,
    control: Option<PushControl>,
) -> Result<bool, UiError> {
    only_label(window.label(), "main")?;
    if serde_json::to_vec(&snapshot).map_err(|_| invalid())?.len() > 256 * 1024 {
        return Err(invalid());
    }
    validate_projection(&snapshot["settings"])?;
    let control = control.unwrap_or_default();
    if control.cancel_close || control.error.is_some() {
        let mut state = relay.state.lock().unwrap_or_else(|e| e.into_inner());
        state.abandon_close();
        if let Some(message) = control.error {
            let session = state.session.clone();
            state.deliver(json!({"kind":"error", "session":session, "message":message}));
        } else {
            let session = state.session.clone();
            state.deliver(json!({"kind":"timeout", "session":session}));
        }
    } else {
        relay.push(snapshot);
    }
    if let Some(child) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = child.set_title(&sanitize_title(&title));
    }
    if close {
        request_close(&app, AfterClose::Quiet);
    }
    Ok(app.get_webview_window(SETTINGS_LABEL).is_some())
}
#[tauri::command]
pub fn settings_window_send(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    relay: tauri::State<'_, Arc<SettingsRelay>>,
    message: SettingsWindowMessage,
) -> Result<(), UiError> {
    only_label(window.label(), SETTINGS_LABEL)?;
    let (session, seq) = message.identity();
    let state = relay.state.lock().unwrap_or_else(|e| e.into_inner());
    if session != state.session || session.is_empty() || seq == 0 || seq > 9_007_199_254_740_991 {
        return Err(invalid());
    }
    if let SettingsWindowMessage::Patch { patch, .. } = &message {
        patch.validate()?;
    }
    app.emit_to("main", "settings-window", &message)
        .map_err(|_| UiError::internal())
}
fn run_after(app: &tauri::AppHandle, after: AfterClose) {
    if let Some(main) = app.get_webview_window("main") {
        match after {
            AfterClose::Main => {
                let _ = main.close();
            }
            AfterClose::Hide => {
                let _ = main.hide();
            }
            AfterClose::Restart => {
                let _ = crate::system::restart_now(app.clone());
            }
            AfterClose::None | AfterClose::Quiet => {}
        }
    }
}
/// Gives the client five seconds to answer a close request (longer while it holds, see
/// `settings_window_hold`), then abandons the request.
fn arm_close_timer(app: &tauri::AppHandle, relay: Arc<SettingsRelay>, generation: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let (expired, session) = {
            let mut state = relay.state.lock().unwrap_or_else(|e| e.into_inner());
            (state.expire_close(generation), state.session.clone())
        };
        if expired {
            let _ = app.emit_to(
                "main",
                "settings-window",
                json!({"kind":"closeFailed", "session":session, "seq":0}),
            );
        }
    });
}
/// True means a live client is draining. With no live client, destroy directly;
/// callers still defer their parent action to `destroyed` while the child exists.
pub(crate) fn request_close(app: &tauri::AppHandle, after: AfterClose) -> bool {
    let Some(window) = app.get_webview_window(SETTINGS_LABEL) else {
        return false;
    };
    let relay = app.state::<Arc<SettingsRelay>>();
    let (waiting, generation) = {
        let mut state = relay.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.request_close(after), state.close_generation)
    };
    if !waiting {
        let _ = window.destroy();
        return false;
    }
    arm_close_timer(app, Arc::clone(relay.inner()), generation);
    true
}
#[tauri::command]
pub fn settings_window_close(
    window: tauri::WebviewWindow,
    relay: tauri::State<'_, Arc<SettingsRelay>>,
    session: String,
    last_seq: u64,
    force: bool,
    cancel: bool,
) -> Result<(), UiError> {
    only_label(window.label(), SETTINGS_LABEL)?;
    let destroy = relay
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .authorize_close(&session, last_seq, force, cancel)?;
    if destroy {
        window.destroy().map_err(|_| UiError::internal())?;
    } else {
        let _ = window.app_handle().emit_to(
            "main",
            "settings-window",
            json!({"kind":"closeFailed", "session":session, "seq":0}),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(session: &str, ack: u64) -> Value {
        json!({"session": session, "ack": ack, "errors": [], "settings": {"fontSize":13}})
    }
    fn sink() -> (Sink, Arc<Mutex<Vec<Value>>>) {
        let values = Arc::new(Mutex::new(Vec::new()));
        let copy = values.clone();
        (
            Box::new(move |value| {
                copy.lock().unwrap().push(value.clone());
                true
            }),
            values,
        )
    }
    #[test]
    fn query_checks_tab_and_appearance() {
        let mut a = Appearance {
            theme: "papier".into(),
            accent: "#123abc".into(),
            accent_automatic: false,
            density: "normal".into(),
            font_size: 13.0,
            font_weight: "normal".into(),
            lang: "en".into(),
        };
        let q = settings_query(Some("keyboard"), &a).unwrap();
        assert!(q.contains("view=settings&tab=keyboard"));
        assert!(q.contains("accent=%23123abc"));
        for tab in [
            "allgemein",
            "darstellung",
            "dateiliste",
            "seitenleiste",
            "terminal",
            "kopieren",
            "tastatur",
            "info",
            "appearance",
            "system",
            "keyboard",
        ] {
            assert!(settings_query(Some(tab), &a).is_ok());
        }
        assert!(settings_query(Some("info&x=y"), &a).is_err());
        a.theme = "bad".into();
        assert!(settings_query(None, &a).is_err());
    }
    #[test]
    fn patches_share_save_validation() {
        for value in [
            json!({"bookmarks": []}),
            json!({"theme":"bad"}),
            json!({"language":"fr"}),
            json!({"accent":"#abc"}),
            json!({"fontSize":7}),
            json!({"fontSize":33}),
            json!({"keybindings":{"x":"a\u{0}b"}}),
            json!({"terminalShell":"/bin/sh"}),
        ] {
            assert!(serde_json::from_value::<SettingsPatch>(value)
                .map(|p| p.validate().is_err())
                .unwrap_or(true));
        }
        let bindings: serde_json::Map<String, Value> =
            (0..201).map(|i| (format!("k{i}"), json!("F1"))).collect();
        assert!(
            serde_json::from_value::<SettingsPatch>(json!({"keybindings":bindings}))
                .unwrap()
                .validate()
                .is_err()
        );
        let patch: SettingsPatch = serde_json::from_value(json!({"paneColorLeft":"", "fontSize":32, "theme":"papier", "verifyPolicy":"always", "keybindings":{"copy":"Ctrl+C"}})).unwrap();
        patch.validate().unwrap();
        let mut base = serde_json::to_value(SettingsDto::from(
            &dualfile_core::settings::Settings::default(),
        ))
        .unwrap();
        for (k, v) in serde_json::to_value(patch).unwrap().as_object().unwrap() {
            base[k] = v.clone();
        }
        crate::validate::validate_settings(&serde_json::from_value(base).unwrap()).unwrap();
    }
    #[test]
    fn message_shape_is_strict() {
        for value in [
            json!({"kind":"patch", "session":"s", "seq":1, "patch":{"bookmarks":[]}}),
            json!({"kind":"unknown", "session":"s", "seq":1}),
            json!({"kind":"checkUpdates", "session":"s"}),
            json!({"kind":"checkUpdates", "session":"s", "seq":-1}),
            json!({"kind":"checkUpdates", "session":"s", "seq":1, "extra":true}),
        ] {
            assert!(serde_json::from_value::<SettingsWindowMessage>(value).is_err());
        }
    }
    #[test]
    fn relay_cache_detach_failure_and_sessions() {
        let relay = SettingsRelay::default();
        relay.push(snapshot("", 20));
        let (send, values) = sink();
        let first = relay.attach(send);
        assert_eq!(values.lock().unwrap()[1]["snapshot"]["ack"], 0);
        relay.push(snapshot(&first, 1));
        relay.detach();
        relay.push(snapshot(&first, 2));
        assert_eq!(values.lock().unwrap().len(), 3);
        let (send, next) = sink();
        let second = relay.attach(send);
        assert_ne!(first, second);
        assert_eq!(next.lock().unwrap()[1]["snapshot"]["ack"], 0);
        relay.push(snapshot(&first, 50));
        assert_eq!(next.lock().unwrap().len(), 2);
        relay.attach(Box::new(|_| false));
        assert!(relay.state.lock().unwrap().sink.is_none());
    }
    #[test]
    fn concurrent_attach_push_and_reattach_never_deliver_old_after_new() {
        let relay = Arc::new(SettingsRelay::default());
        let (send, _) = sink();
        let session = relay.attach(send);
        let worker = relay.clone();
        let thread = std::thread::spawn(move || {
            for ack in 1..=100 {
                let id = worker.state.lock().unwrap().session.clone();
                worker.push(snapshot(&id, ack));
            }
        });
        let (send, values) = sink();
        let next_session = relay.attach(send);
        assert_ne!(next_session, session);
        thread.join().unwrap();
        relay.push(snapshot(&next_session, 101));
        let values = values.lock().unwrap();
        let acks: Vec<_> = values
            .iter()
            .filter_map(|v| v["snapshot"]["ack"].as_u64())
            .collect();
        assert!(acks.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(acks.last(), Some(&101));
    }
    #[test]
    fn capabilities_and_command_gates() {
        let settings: Value =
            serde_json::from_str(include_str!("../capabilities/settings-window.json")).unwrap();
        assert_eq!(settings["windows"], json!(["settings"]));
        assert_eq!(
            settings["permissions"],
            json!([
                "allow-settings-window-attach",
                "allow-settings-window-send",
                "allow-settings-window-close",
                "allow-settings-window-zoomed",
                "allow-settings-window-hold",
                "allow-open-url",
                "allow-log-info",
                "allow-log-read",
                "allow-log-save",
                "allow-log-open-folder",
                "allow-report-issue-url"
            ])
        );
        let main: Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        for (command, label) in [
            ("open", "main"),
            ("push", "main"),
            ("attach", "settings"),
            ("send", "settings"),
            ("close", "settings"),
            ("zoomed", "settings"),
            ("hold", "settings"),
        ] {
            let source = include_str!("settings_window.rs");
            let body = source
                .split(&format!("fn settings_window_{command}("))
                .nth(1)
                .unwrap()
                .split("#[tauri::command]")
                .next()
                .unwrap();
            let gate = if label == "main" {
                "only_label(window.label(), \"main\")?"
            } else {
                "only_label(window.label(), SETTINGS_LABEL)?"
            };
            assert_eq!(
                body.split_once("{\n")
                    .unwrap()
                    .1
                    .lines()
                    .next()
                    .unwrap()
                    .trim(),
                format!("{gate};"),
                "missing first-statement gate for {command}"
            );
            assert!(include_str!("../build.rs").contains(&format!("\"settings_window_{command}\"")));
            let permission = json!(format!("allow-settings-window-{command}"));
            assert_eq!(
                main["permissions"]
                    .as_array()
                    .unwrap()
                    .contains(&permission),
                label == "main"
            );
        }
        for entry in
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities")).unwrap()
        {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let cap: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            if cap["identifier"] == "default" {
                continue;
            }
            assert!(cap["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| !p.as_str().unwrap().starts_with("core:event")));
        }
    }
    fn live_state() -> RelayState {
        RelayState {
            session: "s".into(),
            sink: Some(Box::new(|_| true)),
            ack: 4,
            ..Default::default()
        }
    }
    #[test]
    fn request_close_without_attach_or_with_dead_sink_destroys_and_defers_after() {
        for mut state in [
            RelayState::default(),
            RelayState {
                session: "s".into(),
                sink: Some(Box::new(|_| false)),
                ..Default::default()
            },
        ] {
            assert!(!state.request_close(AfterClose::Main));
            assert!(state.destroying);
            assert_eq!(state.after, AfterClose::Main);
            let (_, _, after) = state.take_destroyed();
            assert_eq!(after, AfterClose::Main);
            assert_eq!(state.take_destroyed().2, AfterClose::None);
            assert!(state.session.is_empty() && state.sink.is_none());
        }
    }
    #[test]
    fn close_checks_ack_and_force_is_an_independent_emergency_exit() {
        let mut state = live_state();
        assert!(state.request_close(AfterClose::Main));
        assert!(state.authorize_close("s", 3, false, false).is_err());
        assert_eq!(state.after, AfterClose::None);
        assert!(!state.destroying);
        assert!(state.authorize_close("old", 4, false, false).is_err());
        assert!(state.authorize_close("s", 4, false, false).unwrap());
        assert!(state.destroying);
        assert_eq!(state.take_destroyed().2, AfterClose::None);
        // Force is still guarded by the command's label check, but needs no live session.
        assert!(state.authorize_close("", 0, true, false).unwrap());
        assert!(state.destroying);
    }
    #[test]
    fn failed_close_cancels_after_and_done_cannot_replay_it() {
        for after in [AfterClose::Main, AfterClose::Hide, AfterClose::Restart] {
            let mut state = live_state();
            assert!(state.request_close(after));
            assert!(!state.authorize_close("s", 4, false, true).unwrap());
            assert_eq!(state.after, AfterClose::None);
            assert!(state.request_close(AfterClose::None));
            assert!(state.authorize_close("s", 4, false, false).unwrap());
            assert_eq!(state.take_destroyed().2, AfterClose::None);
        }
    }
    #[test]
    fn missing_ack_expires_after_without_silent_destruction() {
        let mut state = live_state();
        assert!(state.request_close(AfterClose::Restart));
        let generation = state.close_generation;
        assert!(state.expire_close(generation));
        assert!(!state.destroying);
        assert_eq!(state.after, AfterClose::None);
        // Another explicit main close can escape an unresponsive webview.
        assert!(!state.request_close(AfterClose::Main));
        assert!(state.destroying);
        assert_eq!(state.take_destroyed().2, AfterClose::Main);
        assert!(!state.expire_close(generation));
    }
    #[test]
    fn destroyed_consumes_successful_parent_action_only_once() {
        for after in [AfterClose::Main, AfterClose::Hide, AfterClose::Restart] {
            let mut state = live_state();
            assert!(state.request_close(after));
            assert!(state.authorize_close("s", 4, false, false).unwrap());
            assert!(!state.expire_close(state.close_generation));
            assert_eq!(state.take_destroyed(), ("s".into(), 4, after));
            assert_eq!(state.take_destroyed().2, AfterClose::None);
        }
        let source = include_str!("settings_window.rs");
        let close = source
            .split("pub fn settings_window_close(")
            .nth(1)
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!close.contains("run_after("));
        let destroyed = source
            .split("pub(crate) fn destroyed(")
            .nth(1)
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(destroyed.contains("run_after(app, after)"));
    }
    #[test]
    fn projection_accepts_exact_frontend_keys_and_rejects_extras_and_wrong_types() {
        let full = serde_json::to_value(SettingsDto::from(
            &dualfile_core::settings::Settings::default(),
        ))
        .unwrap();
        let keys = [
            "theme",
            "accent",
            "accentAutomatic",
            "density",
            "fontSize",
            "fontWeight",
            "language",
            "showParentEntry",
            "extensionColumn",
            "paneTintStrength",
            "showPermissionsColumn",
            "showUidColumn",
            "showGidColumn",
            "paneColorLeft",
            "paneColorRight",
            "terminalColors",
            "terminalBg",
            "terminalFg",
            "verifyPolicy",
            "terminalShell",
            "trayEnabled",
            "checkUpdatesOnStart",
            "lastUpdateCheck",
            "sidebarMode",
            "sidebarWidth",
            "sidebarCollapsed",
            "sidebarHidden",
            "sidebarDiscovery",
            "settingsWindowSizeMode",
            "settingsWindowSize",
            "keybindings",
        ];
        let mut projection = json!({});
        for key in keys {
            projection[key] = full[key].clone();
        }
        assert!(validate_projection(&projection).is_ok());
        for key in ["bookmarks", "session", "columnWidths", "other"] {
            let mut extra = projection.clone();
            extra[key] = json!([]);
            assert!(validate_projection(&extra).is_err());
        }
        projection["trayEnabled"] = json!("true");
        assert!(validate_projection(&projection).is_err());
        assert!(std::mem::size_of::<SettingsWindowMessage>() < 100);
    }
    #[test]
    fn remembered_size_is_limited_to_the_screen_and_never_below_the_minimum() {
        let min = MIN_SIZE;
        // Fits: unchanged.
        assert_eq!(
            fit_size((900, 700), min, Some((1920.0, 1080.0))),
            (900.0, 700.0)
        );
        // No monitor known: only the minimum applies.
        assert_eq!(fit_size((900, 700), min, None), (900.0, 700.0));
        // Larger than the screen (smaller monitor now): limited, with room for title bar and panels.
        assert_eq!(
            fit_size((1800, 1000), min, Some((1366.0, 768.0))),
            (1366.0 - SCREEN_MARGIN.0, 768.0 - SCREEN_MARGIN.1)
        );
        // Only one side too large.
        assert_eq!(
            fit_size((1000, 1000), min, Some((1920.0, 800.0))),
            (1000.0, 800.0 - SCREEN_MARGIN.1)
        );
        // A screen smaller than the minimum: the minimum wins.
        assert_eq!(fit_size((900, 700), min, Some((400.0, 300.0))), min);
        // Below the minimum (hand-edited file that got through): raised.
        assert_eq!(fit_size((100, 100), min, Some((1920.0, 1080.0))), min);
        // HiDPI: the area is logical already; fractional results are rounded.
        assert_eq!(fit_size((900, 700), min, Some((1280.5, 720.4))).0, 900.0);
        let (w, h) = fit_size((2000, 2000), min, Some((1280.5, 720.4)));
        assert_eq!((w.fract(), h.fract()), (0.0, 0.0));
    }
    #[test]
    fn holding_keeps_the_close_request_alive_until_released() {
        let mut state = live_state();
        assert!(state.request_close(AfterClose::None));
        let generation = state.close_generation;
        state.holding = true;
        assert!(!state.expire_close(generation));
        assert!(state.closing && !state.destroying && !state.unresponsive);
        state.holding = false;
        assert!(state.expire_close(generation));
        // An abandoned or finished close never keeps a hold.
        state.holding = true;
        state.abandon_close();
        assert!(!state.holding);
        state.holding = true;
        state.take_destroyed();
        assert!(!state.holding);
    }
    #[test]
    fn only_a_close_from_the_window_itself_may_ask_the_user() {
        let delivered = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink = {
            let delivered = Arc::clone(&delivered);
            Box::new(move |value: &Value| {
                delivered.lock().unwrap().push(value.clone());
                true
            }) as Sink
        };
        let mut state = RelayState {
            session: "s".into(),
            sink: Some(sink),
            ..Default::default()
        };
        assert!(state.request_close(AfterClose::None));
        assert_eq!(
            delivered.lock().unwrap().last().unwrap()["interactive"],
            true
        );
        state.abandon_close();
        assert!(state.request_close(AfterClose::Quiet));
        assert_eq!(
            delivered.lock().unwrap().last().unwrap()["interactive"],
            false
        );
        state.abandon_close();
        for after in [AfterClose::Main, AfterClose::Hide, AfterClose::Restart] {
            assert!(state.request_close(after));
            assert_eq!(
                delivered.lock().unwrap().last().unwrap()["interactive"],
                false
            );
            state.abandon_close();
        }
    }
    #[test]
    fn size_fields_are_part_of_the_patch_validation() {
        let patch = |value: Value| -> Result<(), UiError> {
            serde_json::from_value::<SettingsPatch>(value)
                .map_err(|_| invalid())?
                .validate()
        };
        assert!(patch(json!({"settingsWindowSizeMode": "always"})).is_ok());
        assert!(patch(json!({"settingsWindowSizeMode": "sometimes"})).is_err());
        assert!(patch(json!({"settingsWindowSize": {"width": 900, "height": 700}})).is_ok());
        assert!(patch(json!({"settingsWindowSize": {"width": 0, "height": 0}})).is_ok());
        assert!(patch(json!({"settingsWindowSize": {"width": 100, "height": 100}})).is_err());
        assert!(patch(json!({"settingsWindowSize": {"width": 900}})).is_err());
        assert!(
            patch(json!({"settingsWindowSize": {"width": 900, "height": 700, "x": 5}})).is_err()
        );
    }
}
