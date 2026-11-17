use crate::connections::{validate_connections, ConnectionProfile, MAX_CONNECTIONS};
use crate::error::{from_io, Error, Result};
use crate::plugins::{validate_plugins, PluginEntry, MAX_PLUGINS};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP: AtomicU64 = AtomicU64::new(0);
/// Version 2 added `connections` (server profiles), version 3 `plugins` (confirmed plugins),
/// version 4 the SSH transport (old profiles default to Builtin), version 5 checksum policy, version 6 system mount profiles (optional system_url).
/// Older files load unchanged with empty lists and are rewritten as the current version on
/// the next save.
pub const SETTINGS_VERSION: u32 = 6;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub verify_policy: crate::ops::VerifyPolicy,
    pub theme: String,
    pub accent: String,
    #[serde(default)]
    pub accent_automatic: bool,
    pub show_parent_entry: bool,
    pub terminal_height: u32,
    /// Which shell the embedded terminal launches: "auto" (fish with suggestions
    /// when installed, else bash/sh), "bash" (always plain bash/sh), or "login"
    /// (the user's own `$SHELL` with their config, prompt and greeting).
    pub terminal_shell: String,
    /// Terminal colour scheme: "theme" (from the active design), "classic", "green", "amber",
    /// "light" or "custom" (uses `terminal_bg`/`terminal_fg`).
    pub terminal_colors: String,
    /// Only used when `terminal_colors` is "custom"; `#rrggbb`.
    pub terminal_bg: String,
    pub terminal_fg: String,
    pub check_updates_on_start: bool,
    pub tray_enabled: bool,
    pub start_menu_prompted: bool,
    pub start_menu_prompt_count: u8,
    pub dismissed_update_version: String,
    pub last_update_check: Option<String>,
    pub last_seen_version: Option<String>,

    pub density: String,
    pub font_size: u8,
    pub language: String,
    pub bookmarks: Vec<Bookmark>,
    pub session: Session,
    /// Server connection profiles. They hold no password; secrets live in the keychain or
    /// the session memory (see `dualfile-remote::secret_store`).
    pub connections: Vec<ConnectionProfile>,
    /// Plugins the user confirmed (pinned manifest hash and granted options; no secrets).
    pub plugins: Vec<PluginEntry>,
    /// Show a separate file-extension column in the list (off by default).
    pub extension_column: bool,
    /// Optional tint colour (hex) per window; empty means no tint.
    pub pane_color_left: String,
    pub pane_color_right: String,
    /// User-remapped keyboard shortcuts (command id → key spec); empty means all defaults.
    pub keybindings: BTreeMap<String, String>,
    /// Custom file-list column widths (px); empty fields mean the default width.
    pub column_widths: ColumnWidths,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ColumnWidths {
    /// Widths (px) of the three fixed columns in the normal view: size, type, modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal: Option<[u16; 3]>,
    /// Widths (px) of the three fixed columns in the extension-split view: ext, size, modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<[u16; 3]>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            verify_policy: crate::ops::VerifyPolicy::Auto,
            theme: "schiefer".into(),
            accent: "#4f8cff".into(),
            accent_automatic: true,
            show_parent_entry: true,
            terminal_height: 260,
            terminal_shell: "login".into(),
            terminal_colors: "green".into(),
            terminal_bg: "#102030".into(),
            terminal_fg: "#f0e6d2".into(),
            check_updates_on_start: true,
            tray_enabled: false,
            start_menu_prompted: false,
            start_menu_prompt_count: 0,
            dismissed_update_version: String::new(),
            last_update_check: None,
            last_seen_version: None,

            density: "normal".into(),
            font_size: 14,
            language: "auto".into(),
            bookmarks: vec![],
            session: Session::default(),
            connections: vec![],
            plugins: vec![],
            extension_column: false,
            pane_color_left: String::new(),
            pane_color_right: String::new(),
            keybindings: BTreeMap::new(),
            column_widths: ColumnWidths::default(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Bookmark {
    pub name: String,
    pub path: String,
    pub color: String,
    pub order: u32,
}
impl Default for Bookmark {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            color: "#4f8cff".into(),
            order: 0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Session {
    pub windows: Vec<WindowSession>,
    pub active_window: usize,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            windows: vec![WindowSession::default(), WindowSession::default()],
            active_window: 0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WindowSession {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}
impl Default for WindowSession {
    fn default() -> Self {
        Self {
            tabs: vec![Tab::default()],
            active_tab: 0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Tab {
    pub path: String,
    pub sort: String,
    pub filter: String,
}
impl Default for Tab {
    fn default() -> Self {
        Self {
            path: String::new(),
            sort: "name".into(),
            filter: String::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    Fresh,
    Loaded,
    Repaired(Vec<String>),
    RecoveredFromCorrupt,
    NewerVersion(u32),
    Migrated { from: u32, to: u32 },
}
/// Ordner der Einstellungen: `$XDG_CONFIG_HOME/dualfile`, sonst
/// `~/.config/dualfile`. Leere oder relative `XDG_CONFIG_HOME` gelten laut
/// XDG-Spezifikation als nicht gesetzt.
pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            return p.join("dualfile");
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/dualfile")
}

const DENSITIES: [&str; 3] = ["compact", "normal", "comfortable"];
const FONT_SIZES: std::ops::RangeInclusive<u8> = 8..=32;

/// Version 0 (vor der Einführung von `version`): Akzent hieß `accent_color`
/// und Lesezeichen waren eine bloße Liste von Pfaden. Wird in das v1-Format
/// umgeschrieben; bereits vorhandene v1-Felder haben Vorrang.
fn migrate_v0(obj: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::{json, Value};
    if let Some(old) = obj.remove("accent_color") {
        obj.entry("accent").or_insert(old);
    }
    if let Some(Value::Array(items)) = obj.get_mut("bookmarks") {
        for (i, item) in items.iter_mut().enumerate() {
            if let Value::String(path) = item {
                let name = Path::new(path.as_str())
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                *item = json!({ "name": name, "path": path.clone(), "order": i });
            }
        }
    }
    obj.insert("version".into(), json!(1));
}

/// Liest ein Feld; fehlt es, gilt der Standardwert ohne Meldung, hat es den
/// falschen Typ, gilt der Standardwert mit Meldung.
fn field<T: serde::de::DeserializeOwned>(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    fallback: T,
    fixes: &mut Vec<String>,
) -> T {
    match obj.remove(key) {
        None => fallback,
        Some(v) => serde_json::from_value(v).unwrap_or_else(|_| {
            fixes.push(key.to_string());
            fallback
        }),
    }
}

/// Reads the profile list. A profile that is unusable (wrong type, invalid values, a name or
/// id that is already taken, beyond the limit) is dropped and reported; the others stay.
fn load_connections(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    fixes: &mut Vec<String>,
) -> Vec<ConnectionProfile> {
    let items = match obj.remove("connections") {
        None => return Vec::new(),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => {
            fixes.push("connections".into());
            return Vec::new();
        }
    };
    let mut kept: Vec<ConnectionProfile> = Vec::new();
    for item in items {
        let usable = serde_json::from_value::<ConnectionProfile>(item)
            .ok()
            .filter(|profile| {
                kept.len() < MAX_CONNECTIONS
                    && validate_connections(&[kept.clone(), vec![profile.clone()]].concat()).is_ok()
            });
        match usable {
            Some(profile) => kept.push(profile),
            None => {
                if !fixes.iter().any(|f| f == "connections") {
                    fixes.push("connections".into());
                }
            }
        }
    }
    kept
}

/// Reads the plugin list. An entry that is unusable (wrong type, invalid values, a duplicate
/// id, beyond the limit) is dropped and reported; the others stay.
fn load_plugins(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    fixes: &mut Vec<String>,
) -> Vec<PluginEntry> {
    let items = match obj.remove("plugins") {
        None => return Vec::new(),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => {
            fixes.push("plugins".into());
            return Vec::new();
        }
    };
    let mut kept: Vec<PluginEntry> = Vec::new();
    for item in items {
        let usable = serde_json::from_value::<PluginEntry>(item)
            .ok()
            .filter(|entry| {
                kept.len() < MAX_PLUGINS
                    && validate_plugins(&[kept.clone(), vec![entry.clone()]].concat()).is_ok()
            });
        match usable {
            Some(entry) => kept.push(entry),
            None => {
                if !fixes.iter().any(|f| f == "plugins") {
                    fixes.push("plugins".into());
                }
            }
        }
    }
    kept
}

pub fn load(dir: impl AsRef<Path>) -> (Settings, LoadOutcome) {
    let path = dir.as_ref().join("settings.json");
    let data = match fs::read(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Settings::default(), LoadOutcome::Fresh)
        }
        // Nicht lesbar (z. B. Rechte): nichts sichern, Datei bleibt unberührt.
        Err(_) => return (Settings::default(), LoadOutcome::RecoveredFromCorrupt),
    };
    let mut obj = match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(serde_json::Value::Object(o)) => o,
        _ => {
            backup(&path);
            return (Settings::default(), LoadOutcome::RecoveredFromCorrupt);
        }
    };
    let version = match obj.get("version") {
        None => 0,
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => n,
            None => {
                backup(&path);
                return (Settings::default(), LoadOutcome::RecoveredFromCorrupt);
            }
        },
    };
    if version > SETTINGS_VERSION {
        return (
            Settings {
                version,
                ..Settings::default()
            },
            LoadOutcome::NewerVersion(version),
        );
    }
    let migrated = version < SETTINGS_VERSION;
    if version == 0 {
        migrate_v0(&mut obj);
    }

    let d = Settings::default();
    let mut fixes: Vec<String> = Vec::new();
    let mut s = Settings {
        version: SETTINGS_VERSION,
        verify_policy: field(&mut obj, "verify_policy", d.verify_policy, &mut fixes),
        theme: field(&mut obj, "theme", d.theme.clone(), &mut fixes),
        accent: field(&mut obj, "accent", d.accent.clone(), &mut fixes),
        accent_automatic: field(&mut obj, "accent_automatic", false, &mut fixes),
        show_parent_entry: field(&mut obj, "show_parent_entry", true, &mut fixes),
        terminal_height: field(&mut obj, "terminal_height", 260, &mut fixes),
        terminal_shell: field(
            &mut obj,
            "terminal_shell",
            d.terminal_shell.clone(),
            &mut fixes,
        ),
        terminal_colors: field(
            &mut obj,
            "terminal_colors",
            d.terminal_colors.clone(),
            &mut fixes,
        ),
        terminal_bg: field(&mut obj, "terminal_bg", d.terminal_bg.clone(), &mut fixes),
        terminal_fg: field(&mut obj, "terminal_fg", d.terminal_fg.clone(), &mut fixes),
        check_updates_on_start: field(&mut obj, "check_updates_on_start", true, &mut fixes),
        tray_enabled: field(&mut obj, "tray_enabled", false, &mut fixes),
        start_menu_prompted: field(&mut obj, "start_menu_prompted", false, &mut fixes),
        start_menu_prompt_count: field(&mut obj, "start_menu_prompt_count", 0, &mut fixes),
        dismissed_update_version: field(
            &mut obj,
            "dismissed_update_version",
            String::new(),
            &mut fixes,
        ),
        last_update_check: field(&mut obj, "last_update_check", None, &mut fixes),
        last_seen_version: field(&mut obj, "last_seen_version", None, &mut fixes),

        density: field(&mut obj, "density", d.density.clone(), &mut fixes),
        font_size: field(&mut obj, "font_size", d.font_size, &mut fixes),
        language: field(&mut obj, "language", d.language.clone(), &mut fixes),
        bookmarks: field(&mut obj, "bookmarks", d.bookmarks.clone(), &mut fixes),
        session: field(&mut obj, "session", d.session.clone(), &mut fixes),
        connections: Vec::new(),
        plugins: Vec::new(),
        extension_column: field(&mut obj, "extension_column", d.extension_column, &mut fixes),
        keybindings: field(&mut obj, "keybindings", d.keybindings.clone(), &mut fixes),
        column_widths: field(
            &mut obj,
            "column_widths",
            d.column_widths.clone(),
            &mut fixes,
        ),
        pane_color_left: field(
            &mut obj,
            "pane_color_left",
            d.pane_color_left.clone(),
            &mut fixes,
        ),
        pane_color_right: field(
            &mut obj,
            "pane_color_right",
            d.pane_color_right.clone(),
            &mut fixes,
        ),
    };
    if s.last_seen_version
        .as_deref()
        .is_some_and(|v| !valid_seen_version(v))
    {
        s.last_seen_version = None;
        fixes.push("last_seen_version".into());
    }
    s.connections = load_connections(&mut obj, &mut fixes);
    s.plugins = load_plugins(&mut obj, &mut fixes);
    let bad = |ok: bool, name: &str, fixes: &mut Vec<String>| {
        if !ok && !fixes.iter().any(|f| f == name) {
            fixes.push(name.to_string());
        }
        ok
    };
    if !bad(valid_theme(&s.theme), "theme", &mut fixes) {
        s.theme = d.theme.clone();
    }
    if !bad(valid_color(&s.accent), "accent", &mut fixes) {
        s.accent = d.accent.clone();
    }
    if !bad(
        DENSITIES.contains(&s.density.as_str()),
        "density",
        &mut fixes,
    ) {
        s.density = d.density.clone();
    }
    if !bad(FONT_SIZES.contains(&s.font_size), "font_size", &mut fixes) {
        s.font_size = d.font_size;
    }
    if !bad(
        ["de", "en", "auto"].contains(&s.language.as_str()),
        "language",
        &mut fixes,
    ) {
        s.language = d.language.clone();
    }
    if !bad(
        ["auto", "bash", "login"].contains(&s.terminal_shell.as_str()),
        "terminal_shell",
        &mut fixes,
    ) {
        s.terminal_shell = d.terminal_shell.clone();
    }
    if !bad(
        ["theme", "classic", "green", "amber", "light", "custom"]
            .contains(&s.terminal_colors.as_str()),
        "terminal_colors",
        &mut fixes,
    ) {
        s.terminal_colors = d.terminal_colors.clone();
    }
    if !bad(valid_color(&s.terminal_bg), "terminal_bg", &mut fixes) {
        s.terminal_bg = d.terminal_bg.clone();
    }
    if !bad(valid_color(&s.terminal_fg), "terminal_fg", &mut fixes) {
        s.terminal_fg = d.terminal_fg.clone();
    }
    for b in &mut s.bookmarks {
        if !bad(valid_color(&b.color), "bookmarks.color", &mut fixes) {
            b.color = Bookmark::default().color;
        }
    }
    if s.session.windows.is_empty() {
        s.session = Session::default();
        bad(false, "session", &mut fixes);
    }
    for w in &mut s.session.windows {
        if w.tabs.is_empty() {
            w.tabs.push(Tab::default());
            bad(false, "session", &mut fixes);
        }
        if w.active_tab >= w.tabs.len() {
            w.active_tab = 0;
            bad(false, "session", &mut fixes);
        }
    }
    if s.session.active_window >= s.session.windows.len() {
        s.session.active_window = 0;
        bad(false, "session", &mut fixes);
    }

    if !fixes.is_empty() {
        (s, LoadOutcome::Repaired(fixes))
    } else if migrated {
        (
            s,
            LoadOutcome::Migrated {
                from: version,
                to: SETTINGS_VERSION,
            },
        )
    } else {
        (s, LoadOutcome::Loaded)
    }
}

/// SemVer 2.0 syntax, bounded to the size allowed for the settings marker.
pub fn valid_seen_version(s: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    s.len() <= 32 && PATTERN.get_or_init(|| regex::Regex::new(
        r"\A(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?\z"
    ).expect("valid semver regex")).is_match(s)
}

fn valid_theme(s: &str) -> bool {
    ["schiefer", "papier", "werkstatt"].contains(&s)
}

/// Genau `#rrggbb` (sechs Hexziffern, keine Kurzform, kein Alpha).
fn valid_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Kopie der unlesbaren Datei; vorhandene Sicherungen werden nie überschrieben.
fn backup(path: &Path) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for n in 0..1000u32 {
        let name = if n == 0 {
            format!("settings.json.broken-{stamp}")
        } else {
            format!("settings.json.broken-{stamp}-{n}")
        };
        let target = path.with_file_name(name);
        if target.exists() {
            continue;
        }
        let _ = fs::copy(path, target);
        return;
    }
}

/// Atomar speichern: Hilfsdatei (0600) im selben Ordner, fsync, rename.
/// Verweigert, wenn `settings` oder die vorhandene Datei eine neuere Version
/// haben als bekannt, außer `force_newer`.
pub fn save(dir: impl AsRef<Path>, settings: &Settings, force_newer: bool) -> Result<()> {
    let refuse =
        || Error::Unsupported("Einstellungen einer neueren Version werden nicht überschrieben");
    if settings.version > SETTINGS_VERSION && !force_newer {
        return Err(refuse());
    }
    if !force_newer {
        let on_disk = fs::read(dir.as_ref().join("settings.json"))
            .ok()
            .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
            .and_then(|v| v.get("version").and_then(|v| v.as_u64()));
        if on_disk.is_some_and(|v| v > u64::from(SETTINGS_VERSION)) {
            return Err(refuse());
        }
    }
    fs::create_dir_all(dir.as_ref())
        .map_err(|e| from_io("Einstellungsordner anlegen", dir.as_ref(), e))?;
    let target = dir.as_ref().join("settings.json");
    let tmp = dir.as_ref().join(format!(
        ".settings-{}-{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| Error::Io {
        op: "Einstellungen serialisieren",
        path: target.clone(),
        source: std::io::Error::other(e),
    })?;
    use std::io::Write;
    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            // Von Anfang an 0600, kein Zeitfenster mit weiteren Rechten.
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| from_io("Hilfsdatei anlegen", &tmp, e))?;
        f.write_all(&bytes)
            .map_err(|e| from_io("Einstellungen schreiben", &tmp, e))?;
        f.sync_all()
            .map_err(|e| from_io("Einstellungen sichern", &tmp, e))?;
        fs::rename(&tmp, &target).map_err(|e| from_io("Einstellungen speichern", &target, e))?;
        if let Ok(d) = fs::File::open(dir.as_ref()) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::ENV_LOCK;
    use std::ffi::OsString;

    fn write(dir: &Path, text: &str) {
        fs::write(dir.join("settings.json"), text).unwrap();
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "settings.json")
            .collect()
    }

    #[test]
    fn automatic_accent_is_explicit_and_legacy_colors_stay_custom() {
        assert!(Settings::default().accent_automatic);
        let legacy: Settings = serde_json::from_str(r##"{"accent":"#4f8cff"}"##).unwrap();
        assert!(!legacy.accent_automatic);
        let fresh = Settings::default();
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&fresh).unwrap()).unwrap();
        assert!(restored.accent_automatic);
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r##"{"version":6,"accent":"#4f8cff"}"##);
        assert!(!load(dir.path()).0.accent_automatic);
    }

    #[test]
    fn parent_entry_defaults_and_roundtrips_without_migration() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"version":6}"#);
        let (mut settings, outcome) = load(dir.path());
        assert!(settings.show_parent_entry);
        assert_eq!(outcome, LoadOutcome::Loaded);
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert!(legacy.show_parent_entry);
        settings.show_parent_entry = false;
        save(dir.path(), &settings, false).unwrap();
        assert!(!load(dir.path()).0.show_parent_entry);
    }

    #[test]
    fn terminal_height_defaults_to_260_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"version":6}"#);
        let (mut settings, _) = load(dir.path());
        assert_eq!(settings.terminal_height, 260);
        settings.terminal_height = 400;
        save(dir.path(), &settings, false).unwrap();
        assert_eq!(load(dir.path()).0.terminal_height, 400);
    }

    #[test]
    fn terminal_colors_default_to_green_roundtrip_and_repair() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"version":6}"#);
        let (mut settings, outcome) = load(dir.path());
        assert_eq!(settings.terminal_colors, "green");
        assert!(!matches!(outcome, LoadOutcome::Repaired(_)));
        settings.terminal_colors = "custom".into();
        settings.terminal_bg = "#112233".into();
        save(dir.path(), &settings, false).unwrap();
        let loaded = load(dir.path()).0;
        assert_eq!(
            (loaded.terminal_colors.as_str(), loaded.terminal_bg.as_str()),
            ("custom", "#112233")
        );
        write(
            dir.path(),
            r#"{"version":6,"terminal_colors":"pink","terminal_fg":"red"}"#,
        );
        let (bad, outcome) = load(dir.path());
        assert_eq!(
            (bad.terminal_colors.as_str(), bad.terminal_fg.as_str()),
            ("green", "#f0e6d2")
        );
        assert!(
            matches!(outcome, LoadOutcome::Repaired(ref f) if f.contains(&"terminal_colors".to_string()) && f.contains(&"terminal_fg".to_string()))
        );
    }

    #[test]
    fn terminal_shell_defaults_to_login_roundtrips_and_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"version":6}"#);
        let (mut settings, _) = load(dir.path());
        assert_eq!(settings.terminal_shell, "login");
        settings.terminal_shell = "auto".into();
        save(dir.path(), &settings, false).unwrap();
        assert_eq!(load(dir.path()).0.terminal_shell, "auto");
        write(dir.path(), r#"{"version":6,"terminal_shell":"zsh"}"#);
        let (bad, outcome) = load(dir.path());
        assert_eq!(bad.terminal_shell, "login");
        assert!(
            matches!(outcome, LoadOutcome::Repaired(ref f) if f.contains(&"terminal_shell".to_string()))
        );
    }

    fn sample() -> Settings {
        let mut s = Settings {
            theme: "werkstatt".into(),
            accent: "#12abEF".into(),
            density: "compact".into(),
            font_size: 16,
            language: "en".into(),
            ..Settings::default()
        };
        s.bookmarks = vec![
            Bookmark {
                name: "Zeta".into(),
                path: "/z".into(),
                color: "#ff0000".into(),
                order: 2,
            },
            Bookmark {
                name: "Alpha".into(),
                path: "/a".into(),
                color: "#00ff00".into(),
                order: 0,
            },
            Bookmark {
                name: "Mitte".into(),
                path: "/m".into(),
                color: "#0000ff".into(),
                order: 1,
            },
        ];
        s.session.windows[1].tabs.push(Tab {
            path: "/tmp".into(),
            sort: "size".into(),
            filter: "*.rs".into(),
        });
        s.session.windows[1].active_tab = 1;
        s.session.active_window = 1;
        s
    }

    struct EnvRestore(Vec<(&'static str, Option<OsString>)>);
    impl EnvRestore {
        fn new(keys: &[&'static str]) -> Self {
            Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn verify_policy_migration_validation_and_roundtrip() {
        use crate::ops::VerifyPolicy;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, r#"{"version":4}"#).unwrap();
        let (settings, outcome) = load(dir.path());
        assert_eq!(settings.verify_policy, VerifyPolicy::Auto);
        assert_eq!(
            outcome,
            LoadOutcome::Migrated {
                from: 4,
                to: SETTINGS_VERSION
            }
        );
        fs::write(&file, r#"{"version":6,"verify_policy":"bogus"}"#).unwrap();
        let (mut settings, outcome) = load(dir.path());
        assert_eq!(settings.verify_policy, VerifyPolicy::Auto);
        assert!(matches!(outcome, LoadOutcome::Repaired(_)));
        for policy in [
            VerifyPolicy::Never,
            VerifyPolicy::Auto,
            VerifyPolicy::Always,
        ] {
            settings.verify_policy = policy;
            save(dir.path(), &settings, false).unwrap();
            assert_eq!(load(dir.path()).0.verify_policy, policy);
        }
    }

    #[test]
    fn missing_file_gives_defaults_and_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (s, o) = load(dir.path());
        assert_eq!(s, Settings::default());
        assert_eq!(o, LoadOutcome::Fresh);
    }

    #[test]
    fn roundtrip_preserves_everything() {
        let dir = tempfile::tempdir().unwrap();
        let s = sample();
        save(dir.path(), &s, false).unwrap();
        let (back, outcome) = load(dir.path());
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r##"{"version":6,"theme":"papier","future_flag":{"a":1},
                "bookmarks":[{"name":"x","path":"/x","color":"#112233","order":0,"icon":"star"}],
                "session":{"windows":[{"tabs":[{"path":"/a","pinned":true}],"zoom":2}],"extra":1}}"##,
        );
        let (s, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(s.theme, "papier");
        assert_eq!(s.bookmarks[0].name, "x");
        assert_eq!(s.session.windows[0].tabs[0].path, "/a");
    }

    #[test]
    fn removed_mirror_setting_and_stale_keybinding_load_cleanly_and_are_dropped_on_save() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"version":6,"mirror":true,"keybindings":{"mirror":"Ctrl+M","copy":"F9"}}"#,
        );
        let (s, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::Loaded);
        assert!(leftovers(dir.path()).is_empty());
        // A binding for the removed command is just an unused map entry.
        assert_eq!(s.keybindings.get("copy").map(String::as_str), Some("F9"));
        save(dir.path(), &s, false).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert!(saved.get("mirror").is_none(), "{saved}");
    }

    #[test]
    fn invalid_accent_is_replaced_and_reported() {
        for bad in [
            "red", "#fff", "#12345", "#1234567", "112233", "#gg0000", "#12345é", "",
        ] {
            let dir = tempfile::tempdir().unwrap();
            write(
                dir.path(),
                &format!(r#"{{"version":4,"accent":"{bad}","theme":"papier"}}"#),
            );
            let (s, o) = load(dir.path());
            assert_eq!(s.accent, Settings::default().accent, "accent {bad:?}");
            assert_eq!(s.theme, "papier", "valid fields survive");
            assert_eq!(o, LoadOutcome::Repaired(vec!["accent".into()]), "{bad:?}");
        }
        // Gültige Großschreibung bleibt unverändert.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r##"{"version":6,"accent":"#AbCdEf"}"##);
        let (s, o) = load(dir.path());
        assert_eq!((s.accent.as_str(), o), ("#AbCdEf", LoadOutcome::Loaded));
    }

    #[test]
    fn invalid_theme_is_replaced_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r##"{"version":4,"theme":"neon","accent":"#010203"}"##,
        );
        let (s, o) = load(dir.path());
        assert_eq!(s.theme, "schiefer");
        assert_eq!(s.accent, "#010203");
        assert_eq!(o, LoadOutcome::Repaired(vec!["theme".into()]));
    }

    #[test]
    fn wrong_typed_and_out_of_range_values_are_repaired() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"version":4,"font_size":"huge","language":"fr","density":"tiny",
                "session":{"windows":[{"tabs":[],"active_tab":5}],"active_window":3}}"#,
        );
        let (s, o) = load(dir.path());
        let d = Settings::default();
        assert_eq!(s.font_size, d.font_size);
        assert_eq!(s.language, d.language);
        assert_eq!(s.density, d.density);
        assert_eq!(s.session.windows[0].tabs.len(), 1);
        assert_eq!(s.session.windows[0].active_tab, 0);
        assert_eq!(s.session.active_window, 0);
        let LoadOutcome::Repaired(fixes) = o else {
            panic!("expected Repaired");
        };
        for k in ["font_size", "language", "density", "session"] {
            assert!(fixes.iter().any(|f| f == k), "{k} missing in {fixes:?}");
        }
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_used() {
        let dir = tempfile::tempdir().unwrap();
        let garbage = "{ this is not json";
        write(dir.path(), garbage);
        let (s, o) = load(dir.path());
        assert_eq!(s, Settings::default());
        assert_eq!(o, LoadOutcome::RecoveredFromCorrupt);
        let backups: Vec<_> = leftovers(dir.path());
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert!(backups[0].starts_with("settings.json.broken-"));
        let stamp = backups[0].trim_start_matches("settings.json.broken-");
        assert!(stamp.chars().all(|c| c.is_ascii_digit() || c == '-'));
        assert_eq!(
            fs::read_to_string(dir.path().join(&backups[0])).unwrap(),
            garbage
        );
        // Zweiter Fehlerfall derselben Sekunde überschreibt die Sicherung nicht.
        let (_, o2) = load(dir.path());
        assert_eq!(o2, LoadOutcome::RecoveredFromCorrupt);
        assert_eq!(leftovers(dir.path()).len(), 2);
    }

    #[test]
    fn non_object_json_counts_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "[1,2,3]");
        let (s, o) = load(dir.path());
        assert_eq!(s, Settings::default());
        assert_eq!(o, LoadOutcome::RecoveredFromCorrupt);
        assert_eq!(leftovers(dir.path()).len(), 1);
    }

    #[test]
    fn config_dir_prefers_xdg_config_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new(&["XDG_CONFIG_HOME", "HOME"]);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        std::env::set_var("HOME", "/nonexistent-home");
        assert_eq!(config_dir(), tmp.path().join("dualfile"));
        // Unbrauchbare Werte (leer, relativ) gelten als nicht gesetzt.
        std::env::set_var("XDG_CONFIG_HOME", "");
        assert_eq!(
            config_dir(),
            Path::new("/nonexistent-home/.config/dualfile")
        );
        std::env::set_var("XDG_CONFIG_HOME", "relative/dir");
        assert_eq!(
            config_dir(),
            Path::new("/nonexistent-home/.config/dualfile")
        );
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new(&["XDG_CONFIG_HOME", "HOME"]);
        let tmp = tempfile::tempdir().unwrap();
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", tmp.path());
        let dir = config_dir();
        assert_eq!(dir, tmp.path().join(".config/dualfile"));
        // Und es lässt sich dort wirklich speichern und laden.
        save(&dir, &sample(), false).unwrap();
        assert!(dir.join("settings.json").is_file());
        assert_eq!(load(&dir).0, sample());
    }

    #[test]
    fn save_is_atomic_leaves_no_temp_file_and_uses_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        save(&nested, &sample(), false).unwrap();
        save(&nested, &Settings::default(), false).unwrap(); // Überschreiben
        let target = nested.join("settings.json");
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(leftovers(&nested).is_empty(), "{:?}", leftovers(&nested));
        assert_eq!(load(&nested).0, Settings::default());
        // Fehlschlag (Ziel ist ein Ordner) räumt die Hilfsdatei ebenfalls auf.
        let blocked = dir.path().join("blocked");
        fs::create_dir_all(blocked.join("settings.json")).unwrap();
        assert!(save(&blocked, &sample(), false).is_err());
        assert_eq!(leftovers(&blocked), Vec::<String>::new());
    }

    #[test]
    fn newer_version_is_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"version":99,"theme":"papier","novel":[1,2]}"#;
        write(dir.path(), original);
        let (s, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::NewerVersion(99));
        assert_eq!(s.version, 99);
        // Weder mit dem geladenen Stand ...
        assert!(save(dir.path(), &s, false).is_err());
        // ... noch mit frisch erzeugten Standardwerten (Datei auf der Platte zählt).
        assert!(save(dir.path(), &Settings::default(), false).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            original
        );
        assert!(leftovers(dir.path()).is_empty());
        // Ausdrücklich erzwungen geht es.
        save(dir.path(), &Settings::default(), true).unwrap();
        assert_eq!(load(dir.path()).1, LoadOutcome::Loaded);
    }

    #[test]
    fn migrates_v0_to_current() {
        let dir = tempfile::tempdir().unwrap();
        // v0: ohne `version`, Akzent als `accent_color`, Lesezeichen als Pfadliste.
        write(
            dir.path(),
            r##"{"theme":"papier","accent_color":"#aabbcc","bookmarks":["/home/u/Bilder","/mnt/media"]}"##,
        );
        let (s, o) = load(dir.path());
        assert_eq!(
            o,
            LoadOutcome::Migrated {
                from: 0,
                to: SETTINGS_VERSION
            }
        );
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.theme, "papier");
        assert_eq!(s.accent, "#aabbcc");
        assert_eq!(s.bookmarks.len(), 2);
        assert_eq!(
            (
                s.bookmarks[0].name.as_str(),
                s.bookmarks[0].path.as_str(),
                s.bookmarks[0].order
            ),
            ("Bilder", "/home/u/Bilder", 0)
        );
        assert_eq!(
            (s.bookmarks[1].name.as_str(), s.bookmarks[1].order),
            ("media", 1)
        );
        // Gespeichert ist es danach v1 und lädt ohne Migration.
        save(dir.path(), &s, false).unwrap();
        let (again, o2) = load(dir.path());
        assert_eq!(o2, LoadOutcome::Loaded);
        assert_eq!(again, s);
        // Explizites version:0 migriert ebenso.
        write(dir.path(), r##"{"version":0,"accent_color":"#010101"}"##);
        let (s0, o0) = load(dir.path());
        assert_eq!(
            (s0.accent.as_str(), o0),
            (
                "#010101",
                LoadOutcome::Migrated {
                    from: 0,
                    to: SETTINGS_VERSION
                }
            )
        );
    }

    #[test]
    fn bookmark_order_is_preserved_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let s = sample();
        let names: Vec<_> = s.bookmarks.iter().map(|b| b.name.clone()).collect();
        let orders: Vec<_> = s.bookmarks.iter().map(|b| b.order).collect();
        save(dir.path(), &s, false).unwrap();
        let (back, _) = load(dir.path());
        assert_eq!(
            back.bookmarks
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(
            back.bookmarks.iter().map(|b| b.order).collect::<Vec<_>>(),
            orders
        );
    }

    fn sample_profile(id: &str, name: &str) -> crate::connections::ConnectionProfile {
        crate::connections::ConnectionProfile {
            id: id.into(),
            name: name.into(),
            host: "ftp.example.org".into(),
            user: "u".into(),
            ..Default::default()
        }
    }

    #[test]
    fn version_1_files_without_connections_still_load() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r##"{"version":1,"theme":"papier","accent":"#010203","bookmarks":[{"name":"a","path":"/a","color":"#112233","order":0}]}"##,
        );
        let (s, o) = load(dir.path());
        assert_eq!(
            o,
            LoadOutcome::Migrated {
                from: 1,
                to: SETTINGS_VERSION
            }
        );
        assert_eq!(s.version, SETTINGS_VERSION);
        assert!(s.connections.is_empty() && s.plugins.is_empty());
        assert_eq!(s.theme, "papier");
        assert_eq!(s.bookmarks.len(), 1);
        // saving writes the new version and keeps loading
        save(dir.path(), &s, false).unwrap();
        let (again, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(again, s);
    }

    #[test]
    fn connections_round_trip_and_hold_no_password() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sample();
        let mut p = sample_profile("p1", "Server");
        p.pins.push(crate::connections::PinEntry {
            protocol: crate::connections::PinProtocol::Ftps,
            host: "ftp.example.org".into(),
            port: 21,
            sha256: "ab".repeat(32),
            key_type: None,
        });
        s.connections = vec![p, sample_profile("p2", "Zweiter")];
        s.session.windows[0].tabs[0].path = "conn:p1!/home".into();
        save(dir.path(), &s, false).unwrap();
        let text = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        // "kind": "password" names the login method; no field carries a password itself
        for field in ["\"password\":", "\"passphrase\":", "\"secret\":"] {
            assert!(!text.contains(field), "{field}");
        }
        let (back, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(back, s);
        assert_eq!(back.session.windows[0].tabs[0].path, "conn:p1!/home");
    }

    #[test]
    fn unusable_profiles_are_dropped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"version":2,"connections":[
                {"id":"p1","name":"Gut","host":"h","user":"u","port":21},
                {"id":"p2","name":"gut","host":"h","user":"u","port":21},
                {"id":"p1","name":"Andere","host":"h","user":"u","port":21},
                {"id":"p3","name":"Schlecht","host":"a b","user":"u","port":21},
                {"id":"p4","name":"Port","host":"h","user":"u","port":0},
                "kaputt",
                {"id":"p5","name":"Fünf","host":"h","user":"u","port":22,"protocol":"sftp","tls":"none"}
            ]}"#,
        );
        let (s, o) = load(dir.path());
        let ids: Vec<&str> = s.connections.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p5"]);
        assert_eq!(o, LoadOutcome::Repaired(vec!["connections".into()]));
        // a connections value of the wrong type is replaced as a whole
        write(dir.path(), r#"{"version":2,"connections":{"a":1}}"#);
        let (s, o) = load(dir.path());
        assert!(s.connections.is_empty());
        assert_eq!(o, LoadOutcome::Repaired(vec!["connections".into()]));
    }

    #[test]
    fn at_most_the_limit_of_profiles_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let list: Vec<serde_json::Value> = (0..110)
            .map(|i| {
                serde_json::json!({"id": format!("p{i}"), "name": format!("n{i}"),
                    "host": "h", "user": "u", "port": 21})
            })
            .collect();
        write(
            dir.path(),
            &serde_json::json!({"version": 2, "connections": list}).to_string(),
        );
        let (s, _) = load(dir.path());
        assert_eq!(s.connections.len(), crate::connections::MAX_CONNECTIONS);
    }

    fn plugin(id: &str) -> PluginEntry {
        PluginEntry {
            id: id.into(),
            manifest_sha256: "cd".repeat(32),
            enabled: true,
            ..PluginEntry::default()
        }
    }

    #[test]
    fn version_2_files_load_with_an_empty_plugin_list() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r##"{"version":2,"theme":"papier"}"##);
        let (s, o) = load(dir.path());
        assert_eq!(
            o,
            LoadOutcome::Migrated {
                from: 2,
                to: SETTINGS_VERSION
            }
        );
        assert!(s.plugins.is_empty());
        save(dir.path(), &s, false).unwrap();
        assert_eq!(load(dir.path()).1, LoadOutcome::Loaded);
    }

    #[test]
    fn plugins_round_trip_and_defaults_grant_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sample();
        let mut granted = plugin("beispiel");
        granted.write_granted = true;
        granted.failures = 2;
        granted.disabled_reason = Some(crate::plugins::DisableReason::Failures);
        s.plugins = vec![granted, plugin("zweites")];
        save(dir.path(), &s, false).unwrap();
        let (back, o) = load(dir.path());
        assert_eq!(o, LoadOutcome::Loaded);
        assert_eq!(back, s);
        // an entry with only id and hash grants nothing
        write(
            dir.path(),
            &format!(
                r#"{{"version":4,"plugins":[{{"id":"a","manifest_sha256":"{}"}}]}}"#,
                "ab".repeat(32)
            ),
        );
        let (s, _) = load(dir.path());
        let entry = &s.plugins[0];
        assert!(!entry.enabled && !entry.unsandboxed && !entry.write_granted);
    }

    #[test]
    fn unusable_plugin_entries_are_dropped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "ab".repeat(32);
        write(
            dir.path(),
            &format!(
                r#"{{"version":4,"plugins":[
                {{"id":"gut","manifest_sha256":"{hash}"}},
                {{"id":"gut","manifest_sha256":"{hash}"}},
                {{"id":"BAD/ID","manifest_sha256":"{hash}"}},
                {{"id":"hash","manifest_sha256":"nope"}},
                "kaputt",
                {{"id":"zwei","manifest_sha256":"{hash}","enabled":true}}
            ]}}"#
            ),
        );
        let (s, o) = load(dir.path());
        let ids: Vec<&str> = s.plugins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["gut", "zwei"]);
        assert_eq!(o, LoadOutcome::Repaired(vec!["plugins".into()]));
        write(dir.path(), r#"{"version":4,"plugins":{"a":1}}"#);
        let (s, o) = load(dir.path());
        assert!(s.plugins.is_empty());
        assert_eq!(o, LoadOutcome::Repaired(vec!["plugins".into()]));
    }

    #[test]
    fn at_most_the_limit_of_plugins_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "ab".repeat(32);
        let list: Vec<serde_json::Value> = (0..60)
            .map(|i| serde_json::json!({"id": format!("p{i}"), "manifest_sha256": hash}))
            .collect();
        write(
            dir.path(),
            &serde_json::json!({"version": 3, "plugins": list}).to_string(),
        );
        let (s, _) = load(dir.path());
        assert_eq!(s.plugins.len(), MAX_PLUGINS);
    }
}

#[cfg(test)]
mod ssh_migration_tests {
    use super::*;
    use crate::connections::{ConnectionProtocol, ConnectionTls, SshTransport};

    #[test]
    fn version_three_profiles_keep_builtin_and_version_four_inherits_ssh_values() {
        let dir = tempfile::tempdir().unwrap();
        let old = r#"{"version":3,"connections":[{"id":"p1","name":"old","protocol":"sftp","tls":"none","host":"nas","user":"u","port":22}]}"#;
        fs::write(dir.path().join("settings.json"), old).unwrap();
        let (mut settings, outcome) = load(dir.path());
        assert_eq!(
            outcome,
            LoadOutcome::Migrated {
                from: 3,
                to: SETTINGS_VERSION
            }
        );
        assert_eq!(settings.connections[0].transport, SshTransport::Builtin);
        let p = &mut settings.connections[0];
        p.transport = SshTransport::Openssh;
        p.protocol = ConnectionProtocol::Sftp;
        p.tls = ConnectionTls::None;
        p.port = 0;
        p.user.clear();
        assert!(p.validate().is_ok());
        save(dir.path(), &settings, false).unwrap();
        let (back, outcome) = load(dir.path());
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, settings);
        let mut invalid = back.connections[0].clone();
        invalid.save_secret = true;
        assert!(invalid.validate().is_err());
        invalid.save_secret = false;
        invalid.host = "-oProxyCommand=x".into();
        assert!(invalid.validate().is_err());
    }
}

#[cfg(test)]
mod updater_migration_tests {
    use super::*;
    #[test]
    fn legacy_defaults_and_new_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), r#"{"version":6}"#).unwrap();
        let (mut s, _) = load(dir.path());
        assert!(s.check_updates_on_start);
        assert!(!s.tray_enabled && !s.start_menu_prompted);
        assert_eq!(s.start_menu_prompt_count, 0);
        assert!(s.dismissed_update_version.is_empty() && s.last_update_check.is_none());
        s.check_updates_on_start = false;
        s.tray_enabled = true;
        s.start_menu_prompted = true;
        s.start_menu_prompt_count = 3;
        s.dismissed_update_version = "1.2.3".into();
        s.last_update_check = Some("2026-01-01T00:00:00Z".into());
        save(dir.path(), &s, false).unwrap();
        assert_eq!(load(dir.path()).0, s);
    }
}

#[cfg(test)]
mod release_notes_tests {
    use super::*;

    #[test]
    fn missing_marker_loads_without_repair_and_marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("last_seen_version");
        fs::write(dir.path().join("settings.json"), value.to_string()).unwrap();
        let (mut settings, outcome) = load(dir.path());
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(settings.last_seen_version, None);
        settings.last_seen_version = Some("0.3.5-rc.1+build.2".into());
        save(dir.path(), &settings, false).unwrap();
        assert_eq!(load(dir.path()), (settings, LoadOutcome::Loaded));
    }

    #[test]
    fn invalid_marker_is_repaired_and_semver_is_bounded() {
        for invalid in [
            "01.2.3",
            "1.2",
            "1.2.3-01",
            "1.2.3\n",
            "",
            "1.2.3+abcdefghijklmnopqrstuvwxyz123",
        ] {
            assert!(!valid_seen_version(invalid), "{invalid}");
            let dir = tempfile::tempdir().unwrap();
            let mut value = serde_json::to_value(Settings::default()).unwrap();
            value["last_seen_version"] = serde_json::json!(invalid);
            fs::write(dir.path().join("settings.json"), value.to_string()).unwrap();
            let (settings, outcome) = load(dir.path());
            assert_eq!(settings.last_seen_version, None);
            assert_eq!(
                outcome,
                LoadOutcome::Repaired(vec!["last_seen_version".into()])
            );
        }
        for valid in ["0.3.10", "0.3.5-rc.1", "1.2.3+build.01", "1.2.3-0.a-1"] {
            assert!(valid_seen_version(valid), "{valid}");
        }
    }
}
