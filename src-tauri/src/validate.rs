//! Input validation for every command. The UI is not trusted: paths must be absolute,
//! normalized and free of NUL bytes and `..`; names go through the core's name check.

use crate::convert::{SettingsDto, UiError};
use dualfile_core::archive::MSG_READ_ONLY;
use dualfile_core::connections::{parse_profile_path, PROFILE_PATH_PREFIX};
use dualfile_core::names::validate_name;
use dualfile_core::vfs::{parse_ipc_path, Scheme, VPath};
use std::path::{Component, Path, PathBuf};

/// Start of every serialized archive location (local paths start with `/`).
const ARCHIVE_PREFIX: &str = "archive:";
/// Starts of serialized server locations (`ftp:<connection id>!<path>`).
const REMOTE_PREFIXES: [&str; 3] = ["ftp:", "sftp:", "smb:"];
/// Start of a serialized plugin location (`plugin:<plugin id>!<path>`).
const PLUGIN_PREFIX: &str = "plugin:";

/// Shown when something would be written to a plugin location where only local folders are
/// allowed, or when trash is asked for there.
pub const MSG_NO_PLUGIN_TRASH: &str =
    "Ein Plugin-Ort hat keinen Papierkorb; dort hilft nur „Endgültig löschen“";
pub const MSG_NO_PLUGIN_TARGET: &str =
    "Hier wird nur in lokale Ordner geschrieben, nicht an einen Plugin-Ort";

/// Shown when trash is asked for on a server (which has none; deleting for good needs the
/// delete plan and its confirmation).
pub const MSG_NO_REMOTE_TRASH: &str =
    "Auf einem Server gibt es keinen Papierkorb; dort hilft nur „Endgültig löschen“";
/// Shown when something would be written to a server where only local folders are allowed
/// (the synchronisation targets).
pub const MSG_NO_REMOTE_SYNC_TARGET: &str =
    "Abgleichen schreibt nur in lokale Ordner, nicht auf einen Server";

/// Linux `PATH_MAX`.
pub const MAX_PATH_BYTES: usize = 4096;
/// Upper bound for one operation's item list.
pub const MAX_ITEMS: usize = 100_000;
/// Folders the UI may ask to watch at once (two windows, a little headroom).
pub const MAX_WATCHED: usize = 8;

fn invalid_path(message: &str) -> UiError {
    UiError::new("invalidPath", message)
}

/// Parses a path from the UI: a local path, an archive location in the IPC format
/// (`archive:<outer>!<inner>`) or a server location (`ftp:<connection id>!<path>`,
/// `sftp:…`; whether the connection exists is the router's question). For local paths `~` and
/// `~/…` expand to the home folder; the result is absolute and normalized (no `.`, no
/// doubled or trailing slashes). `..` is rejected.
pub fn parse_path(raw: &str, home: &Path) -> Result<VPath, UiError> {
    if raw.contains('\0') {
        return Err(invalid_path("Der Pfad enthält ein ungültiges Zeichen"));
    }
    if raw.is_empty() {
        return Err(invalid_path("Der Pfad ist leer"));
    }
    if raw.len() > MAX_PATH_BYTES {
        return Err(invalid_path("Der Pfad ist zu lang"));
    }
    if raw.starts_with(ARCHIVE_PREFIX) {
        // `archive:<outer>!<inner>`: the core parser checks nesting depth, encoding, `..`
        // and the outer location; nothing is expanded or normalized here.
        return Ok(parse_ipc_path(raw)?);
    }
    if raw.starts_with(PLUGIN_PREFIX) {
        // Checked strictly (a forged one is an invalid path); whether the plugin is switched
        // on is the router's question (`disconnected` otherwise).
        return Ok(parse_ipc_path(raw)?);
    }
    if REMOTE_PREFIXES.iter().any(|prefix| raw.starts_with(prefix)) {
        // forged ones (bad id, `..`, control characters, relative) fail as invalid paths
        return Ok(parse_ipc_path(raw)?);
    }
    let expanded: PathBuf = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    };
    if expanded.as_os_str().len() > MAX_PATH_BYTES {
        return Err(invalid_path("Der Pfad ist zu lang"));
    }
    if !expanded.is_absolute() {
        return Err(invalid_path("Der Pfad ist nicht absolut"));
    }
    if expanded
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid_path("Der Pfad darf kein „..“ enthalten"));
    }
    let normalized: PathBuf = expanded.components().collect();
    Ok(VPath::local(normalized)?)
}

/// The scan API never resolves archive, connection or plugin locations.
pub fn scan_path(raw: &str, home: &Path) -> Result<PathBuf, UiError> {
    if ["archive:", "ftp:", "sftp:", "smb:", "conn:", "plugin:"]
        .iter()
        .any(|prefix| raw.starts_with(prefix))
    {
        return Err(UiError::new("invalidInput", "Nur lokale Ordner"));
    }
    let path = parse_path(raw, home)?;
    if path.scheme() != Scheme::Local {
        return Err(UiError::new("invalidInput", "Nur lokale Ordner"));
    }
    Ok(path.path().to_path_buf())
}

#[cfg(test)]
pub fn parse_paths(raw: &[String], home: &Path) -> Result<Vec<VPath>, UiError> {
    parse_paths_with(raw, |path| parse_path(path, home))
}

/// A list of paths (not empty, at most `MAX_ITEMS`), each parsed by `parse`.
pub fn parse_paths_with(
    raw: &[String],
    mut parse: impl FnMut(&str) -> Result<VPath, UiError>,
) -> Result<Vec<VPath>, UiError> {
    if raw.is_empty() {
        return Err(invalid_path("Keine Einträge angegeben"));
    }
    if raw.len() > MAX_ITEMS {
        return Err(invalid_path("Zu viele Einträge"));
    }
    raw.iter().map(|path| parse(path)).collect()
}

/// Operations on the file system root itself make no sense and are refused.
pub fn reject_root(path: &VPath) -> Result<(), UiError> {
    if path.path().parent().is_none() {
        return Err(invalid_path("Die Wurzel kann nicht verändert werden"));
    }
    Ok(())
}

pub fn reject_roots(paths: &[VPath]) -> Result<(), UiError> {
    paths.iter().try_for_each(reject_root)
}

/// Archives are read-only: every command that changes a location refuses archive paths with
/// the kind `readOnly` (the core refuses them, too; this gives the answer before a job
/// is even queued).
pub fn reject_archive_writes(paths: &[VPath]) -> Result<(), UiError> {
    if paths.iter().any(|path| path.scheme() == Scheme::Archive) {
        return Err(UiError::new("readOnly", MSG_READ_ONLY));
    }
    Ok(())
}

/// Server locations are refused (`unsupported`, with `message`) by commands that cannot work
/// there.
pub fn reject_remote(paths: &[VPath], message: &'static str) -> Result<(), UiError> {
    if let Some(path) = paths.iter().find(|path| path.scheme().is_remote()) {
        let message = if path.scheme() == Scheme::Plugin {
            if message == MSG_NO_REMOTE_TRASH {
                MSG_NO_PLUGIN_TRASH
            } else {
                MSG_NO_PLUGIN_TARGET
            }
        } else {
            message
        };
        return Err(UiError::new("unsupported", message));
    }
    Ok(())
}

pub fn check_name(name: &str) -> Result<(), UiError> {
    Ok(validate_name(name)?)
}

fn valid_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn bad_settings(what: &str) -> UiError {
    UiError::new(
        "invalidSettings",
        format!("Ungültige Einstellungen: {what}"),
    )
}

fn plain_text(text: &str, max_bytes: usize) -> bool {
    text.len() <= max_bytes && !text.contains('\0')
}

/// A path the settings may keep: local, in an archive, or the persistent server form
/// `conn:<profile id>!<path>`. The location of a running connection (`ftp:<id>!…`) would be
/// stale at the next start and is refused.
fn settings_path(text: &str) -> bool {
    if !plain_text(text, MAX_PATH_BYTES) {
        return false;
    }
    if REMOTE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
        || text.starts_with(PLUGIN_PREFIX)
    {
        return false;
    }
    if text.starts_with(PROFILE_PATH_PREFIX) {
        return parse_profile_path(text).is_ok();
    }
    true
}

/// Checks settings coming from the UI before they are written (the core only repairs on load).
pub fn validate_settings(s: &SettingsDto) -> Result<(), UiError> {
    if !["schiefer", "papier", "werkstatt"].contains(&s.theme.as_str()) {
        return Err(bad_settings("Theme"));
    }
    if !["compact", "normal", "comfortable"].contains(&s.density.as_str()) {
        return Err(bad_settings("Dichte"));
    }
    if !["de", "en", "auto"].contains(&s.language.as_str()) {
        return Err(bad_settings("Sprache"));
    }
    if !valid_color(&s.accent) {
        return Err(bad_settings("Akzentfarbe"));
    }
    if !(8..=32).contains(&s.font_size) {
        return Err(bad_settings("Schriftgröße"));
    }
    if s.bookmarks.len() > 200 {
        return Err(bad_settings("zu viele Lesezeichen"));
    }
    for bookmark in &s.bookmarks {
        if !plain_text(&bookmark.name, 255)
            || !settings_path(&bookmark.path)
            || !valid_color(&bookmark.color)
        {
            return Err(bad_settings("Lesezeichen"));
        }
    }
    for c in [&s.pane_color_left, &s.pane_color_right] {
        if !c.is_empty() && !valid_color(c) {
            return Err(bad_settings("Fensterfarbe"));
        }
    }
    if s.keybindings.len() > 200 {
        return Err(bad_settings("zu viele Tastenkürzel"));
    }
    for (command, spec) in &s.keybindings {
        if !plain_text(command, 64) || !plain_text(spec, 64) {
            return Err(bad_settings("Tastenkürzel"));
        }
    }
    let windows = &s.session.windows;
    if windows.len() > 8 || (!windows.is_empty() && s.session.active_window >= windows.len()) {
        return Err(bad_settings("Fenster"));
    }
    for window in windows {
        if window.tabs.len() > 200
            || (!window.tabs.is_empty() && window.active_tab >= window.tabs.len())
        {
            return Err(bad_settings("Reiter"));
        }
        for tab in &window.tabs {
            if !settings_path(&tab.path)
                || !plain_text(&tab.sort, 64)
                || !plain_text(&tab.filter, 1024)
            {
                return Err(bad_settings("Reiter"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{BookmarkDto, SessionDto, TabDto, WindowSessionDto};

    const HOME: &str = "/home/demo";

    fn parse(raw: &str) -> Result<PathBuf, UiError> {
        parse_path(raw, Path::new(HOME)).map(|v| v.path().to_path_buf())
    }

    #[test]
    fn verify_policy_is_validated_on_deserialization() {
        let mut value = serde_json::to_value(settings()).unwrap();
        for policy in ["never", "auto", "always"] {
            value["verifyPolicy"] = serde_json::json!(policy);
            let dto: SettingsDto = serde_json::from_value(value.clone()).unwrap();
            assert!(validate_settings(&dto).is_ok());
        }
        value["verifyPolicy"] = serde_json::json!("invalid");
        assert!(serde_json::from_value::<SettingsDto>(value).is_err());
    }

    #[test]
    fn accepts_and_normalizes_absolute_paths() {
        assert_eq!(parse("/a//b/./c/").unwrap(), PathBuf::from("/a/b/c"));
        assert_eq!(parse("/").unwrap(), PathBuf::from("/"));
    }

    #[test]
    fn expands_the_home_shortcut_only_at_the_start() {
        assert_eq!(parse("~").unwrap(), PathBuf::from(HOME));
        assert_eq!(parse("~/Doku").unwrap(), PathBuf::from("/home/demo/Doku"));
        assert!(parse("~user").is_err());
        assert!(parse("/a/~").is_ok());
    }

    #[test]
    fn rejects_relative_empty_nul_parent_and_overlong_paths() {
        for bad in ["", "a/b", "./a", "/a/../b", "/..", "/a\0b", "~/../x"] {
            let error = parse(bad).unwrap_err();
            assert_eq!(error.kind, "invalidPath", "{bad:?}");
        }
        let long = format!("/{}", "x".repeat(MAX_PATH_BYTES));
        assert!(parse(&long).is_err());
    }

    #[test]
    fn nul_error_message_does_not_echo_the_path() {
        let error = parse("/secret\0x").unwrap_err();
        assert!(!error.message.contains("secret"));
    }

    #[test]
    fn server_locations_are_parsed_strictly() {
        let home = Path::new(HOME);
        let ok = parse_path("ftp:abc!/x", home).unwrap();
        assert_eq!(ok.scheme(), Scheme::Ftp);
        assert_eq!(ok.conn_id(), Some("abc"));
        assert_eq!(ok.path(), Path::new("/x"));
        assert_eq!(
            parse_path("sftp:abc!/", home).unwrap().scheme(),
            Scheme::Sftp
        );
        // the path after the first `!` is taken literally
        let odd = parse_path("ftp:a!/we!rd%21", home).unwrap();
        assert_eq!(odd.path(), Path::new("/we!rd%21"));
        assert_eq!(dualfile_core::format_ipc_path(&odd), "ftp:a!/we!rd%21");
        // doubled slashes are normalized like everywhere
        assert_eq!(
            parse_path("ftp:a!//x///y/", home).unwrap().path(),
            Path::new("/x/y")
        );
        // forged ones fail as invalid paths
        for raw in [
            "ftp:ABC!/x",
            "ftp:a!/../x",
            "ftp:a",
            "sftp:a!/a\nb",
            "ftp:a!relative",
            "ftp:!/x",
            "ftp:a b!/x",
            "sftp:a!/a\\b",
        ] {
            assert_eq!(
                parse_path(raw, home).unwrap_err().kind,
                "invalidPath",
                "{raw}"
            );
        }
        let long = format!("ftp:a!/{}", "x".repeat(MAX_PATH_BYTES));
        assert_eq!(parse_path(&long, home).unwrap_err().kind, "invalidPath");
        // a list may mix places; each entry is parsed on its own
        let list = parse_paths(&["/a".into(), "ftp:abc!/x".into()], home).unwrap();
        assert_eq!(list[1].scheme(), Scheme::Ftp);
        // a local path that merely looks similar is still local
        assert_eq!(parse("/ftp:abc!/x").unwrap(), PathBuf::from("/ftp:abc!/x"));
        assert!(parse_path("ftp:", home).is_err());
    }

    #[test]
    fn plugin_locations_parse_strictly_and_never_end_up_in_the_settings() {
        let home = Path::new(HOME);
        for raw in ["plugin:example!/x", "plugin:a-1!/", "plugin:a!/we!rd"] {
            let path = parse_path(raw, home).unwrap();
            assert_eq!(path.scheme(), Scheme::Plugin, "{raw}");
        }
        assert!(!settings_path("plugin:a!/x"));
        // forged ones are invalid paths, not "unsupported"
        for raw in [
            "plugin:",
            "plugin:A!/x",
            "plugin:a!/../x",
            "plugin:a!x",
            "plugin:a b!/x",
        ] {
            assert_eq!(
                parse_path(raw, home).unwrap_err().kind,
                "invalidPath",
                "{raw}"
            );
        }
        // a list may mix places; a forged plugin entry refuses the whole list
        let list = parse_paths(&["/a".into(), "plugin:p!/x".into()], home).unwrap();
        assert_eq!(list[1].scheme(), Scheme::Plugin);
        assert!(parse_paths(&["/a".into(), "plugin:P!/x".into()], home).is_err());
        // a local path that only looks similar is still local
        assert_eq!(
            parse("/plugin:p!/x").unwrap(),
            PathBuf::from("/plugin:p!/x")
        );
        // plugin locations count as remote for the commands that cannot work there
        let plugin = VPath::plugin("p", "/a").unwrap();
        assert!(reject_remote(&[plugin], MSG_NO_REMOTE_TRASH).is_err());
    }

    #[test]
    fn remote_writes_are_refused_with_the_given_message() {
        let local = VPath::local("/a").unwrap();
        let remote = VPath::ftp("c1", "/a").unwrap();
        assert!(reject_remote(std::slice::from_ref(&local), MSG_NO_REMOTE_TRASH).is_ok());
        let error = reject_remote(&[local, remote], MSG_NO_REMOTE_TRASH).unwrap_err();
        assert_eq!(error.kind, "unsupported");
        assert_eq!(error.message, MSG_NO_REMOTE_TRASH);
    }

    #[test]
    fn path_lists_must_not_be_empty_or_huge() {
        assert!(parse_paths(&[], Path::new(HOME)).is_err());
        let ok = parse_paths(&["/a".into(), "~/b".into()], Path::new(HOME)).unwrap();
        assert_eq!(ok.len(), 2);
        assert!(parse_paths(&["/a".into(), "x".into()], Path::new(HOME)).is_err());
        let many = vec!["/a".to_owned(); MAX_ITEMS + 1];
        assert!(parse_paths(&many, Path::new(HOME)).is_err());
    }

    #[test]
    fn the_root_is_refused_for_changing_operations() {
        assert!(reject_root(&VPath::local("/").unwrap()).is_err());
        assert!(reject_root(&VPath::local("/a").unwrap()).is_ok());
        assert!(reject_roots(&[VPath::local("/a").unwrap(), VPath::local("/").unwrap()]).is_err());
    }

    #[test]
    fn archive_paths_use_the_ipc_format_and_keep_exclamation_and_percent_marks() {
        let home = Path::new(HOME);
        let plain = parse_path("archive:/home/u/a.zip!/dir/x.txt", home).unwrap();
        assert_eq!(plain.scheme(), Scheme::Archive);
        assert_eq!(plain.path(), Path::new("/dir/x.txt"));
        assert_eq!(plain.outer().unwrap().path(), Path::new("/home/u/a.zip"));
        let odd = parse_path("archive:/home/u/a%21b%25c.zip!/x", home).unwrap();
        assert_eq!(odd.outer().unwrap().path(), Path::new("/home/u/a!b%c.zip"));
        assert_eq!(
            dualfile_core::format_ipc_path(&odd),
            "archive:/home/u/a%21b%25c.zip!/x"
        );
        let nested = parse_path("archive:archive:/a.zip%21/in.tar!/x", home).unwrap();
        assert_eq!(nested.archive_depth(), 2);
        // local paths are not confused with archive paths and still get normalized
        assert_eq!(parse("/archive:/x").unwrap(), PathBuf::from("/archive:/x"));
    }

    #[test]
    fn forged_archive_paths_are_refused() {
        for bad in [
            "archive:/x.zip!/../../etc/passwd",
            "archive:/x.zip!/a/../b",
            "archive:relative.zip!/x",
            "archive:/x.zip",
            "archive:/x.zip!relative",
            "archive:/x.zip!/a\0b",
            "archive:/a%zz.zip!/x",
            "archive:~/x.zip!/x",
            "archive:archive:archive:archive:/a.zip%2521!/b!/c!/d!/e",
        ] {
            assert!(parse_path(bad, Path::new(HOME)).is_err(), "{bad:?}");
        }
        let long = format!("archive:/x.zip!/{}", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(
            parse_path(&long, Path::new(HOME)).unwrap_err().kind,
            "invalidPath"
        );
    }

    #[test]
    fn writes_to_archive_paths_are_refused_as_read_only() {
        let local = VPath::local("/a").unwrap();
        let inside = parse_path("archive:/a.zip!/x", Path::new(HOME)).unwrap();
        assert!(reject_archive_writes(std::slice::from_ref(&local)).is_ok());
        let error = reject_archive_writes(&[local, inside]).unwrap_err();
        assert_eq!(error.kind, "readOnly");
        assert_eq!(error.message, MSG_READ_ONLY);
    }

    #[test]
    fn names_use_the_core_rules() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert_eq!(check_name(bad).unwrap_err().kind, "invalidName", "{bad:?}");
        }
        assert!(check_name("ok name.txt").is_ok());
    }

    fn settings() -> SettingsDto {
        SettingsDto {
            version: 1,
            verify_policy: Default::default(),
            theme: "schiefer".into(),
            accent: "#4f8cff".into(),
            accent_automatic: false,
            show_parent_entry: true,
            check_updates_on_start: true,
            tray_enabled: false,
            start_menu_prompted: false,
            start_menu_prompt_count: 0,
            dismissed_update_version: String::new(),
            last_update_check: None,
            density: "normal".into(),
            font_size: 13,
            language: "auto".into(),
            bookmarks: vec![BookmarkDto {
                name: "x".into(),
                path: "/x".into(),
                color: "#112233".into(),
                order: 0,
            }],
            mirror: false,
            extension_column: false,
            pane_color_left: String::new(),
            pane_color_right: String::new(),
            keybindings: Default::default(),
            session: SessionDto {
                windows: vec![WindowSessionDto {
                    tabs: vec![TabDto {
                        path: "/x".into(),
                        sort: "name".into(),
                        filter: String::new(),
                    }],
                    active_tab: 0,
                }],
                active_window: 0,
            },
        }
    }

    #[test]
    fn valid_settings_pass_and_bad_values_fail() {
        assert!(validate_settings(&settings()).is_ok());
        let mut s = settings();
        s.theme = "neon".into();
        assert!(validate_settings(&s).is_err());
        let mut s = settings();
        s.accent = "red".into();
        assert!(validate_settings(&s).is_err());
        let mut s = settings();
        s.font_size = 80;
        assert!(validate_settings(&s).is_err());
        let mut s = settings();
        s.bookmarks[0].path = "/a\0b".into();
        assert!(validate_settings(&s).is_err());
        let mut s = settings();
        s.session.windows[0].active_tab = 3;
        assert!(validate_settings(&s).is_err());
        let mut s = settings();
        s.session.active_window = 5;
        assert_eq!(validate_settings(&s).unwrap_err().kind, "invalidSettings");
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    #[test]
    fn local_only_with_fixed_error_and_existing_path_validation() {
        for path in [
            "archive:/tmp/a.zip!/",
            "ftp:id!/",
            "sftp:id!/",
            "smb:id!/",
            "conn:id!/",
            "plugin:id!/",
        ] {
            assert_eq!(
                scan_path(path, Path::new("/home/demo")).unwrap_err(),
                UiError::new("invalidInput", "Nur lokale Ordner")
            );
        }
        assert_eq!(
            scan_path("~/data", Path::new("/home/demo")).unwrap(),
            PathBuf::from("/home/demo/data")
        );
        assert!(scan_path("/tmp/../secret", Path::new("/home/demo")).is_err());
    }
}

#[cfg(test)]
mod smb_tests {
    use super::parse_path;
    use dualfile_core::vfs::Scheme;
    use std::path::Path;
    #[test]
    fn smb_paths_are_remote_and_traversal_is_rejected() {
        let home = Path::new("/home/demo");
        assert_eq!(
            parse_path("smb:id!/folder", home).unwrap().scheme(),
            Scheme::Smb
        );
        assert!(parse_path("smb:id!/../folder", home).is_err());
    }
}
