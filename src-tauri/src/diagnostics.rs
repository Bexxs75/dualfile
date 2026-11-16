//! Log file for troubleshooting: where it lives, how it is written, and the commands that let
//! the user read, anonymize, save and report it. Nothing here ever leaves the machine by itself.

use crate::anonymize::{anonymize, AnonContext};
use crate::convert::UiError;
use crate::state::AppState;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::plugin::TauriPlugin;
use tauri::{Runtime, State};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

const LOG_NAME: &str = "dualfile";
const MAX_FILE_BYTES: u128 = 1024 * 1024;
/// Newest rotated files kept next to the active one.
const OLD_FILES: usize = 2;
const VIEW_LINES: usize = 500;
const MAX_UI_MESSAGE_CHARS: usize = 2000;
/// Width of `2026-11-16 15:01:31 ERROR `, so wrapped lines stay under their entry.
const CONTINUATION_INDENT: &str = "                          ";
const ISSUES_URL: &str = "https://github.com/Bexxs75/dualfile/issues/new";

type Shared<'a> = State<'a, Arc<AppState>>;

/// `$XDG_STATE_HOME/dualfile`, or `~/.local/state/dualfile`.
pub fn log_dir() -> PathBuf {
    resolve_log_dir(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn resolve_log_dir(state_home: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    let base = state_home
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.filter(|p| p.is_absolute())
                .map(|h| h.join(".local/state"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("dualfile")
}

/// Facts about the running installation, for the first log line and the issue template.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Environment {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub desktop: Option<String>,
    pub session: Option<String>,
    pub app_image: bool,
}

impl Environment {
    pub fn current() -> Self {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            desktop: var("XDG_CURRENT_DESKTOP"),
            session: var("XDG_SESSION_TYPE"),
            app_image: std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()),
        }
    }

    pub fn start_line(&self) -> String {
        format!(
            "Dualfile {} gestartet · {} {} · {} ({}) · {}",
            self.version,
            self.os,
            self.arch,
            self.desktop.as_deref().unwrap_or("unbekannt"),
            self.session.as_deref().unwrap_or("unbekannt"),
            if self.app_image {
                "AppImage"
            } else {
                "installiert"
            }
        )
    }

    fn issue_body(&self) -> String {
        format!(
            "**What happened?**\n\n\n**Environment**\n- Dualfile: {}\n- OS: {} {}\n- Desktop: {} ({})\n- AppImage: {}\n\n**Log**\nPlease paste the anonymized log file here.\n",
            self.version,
            self.os,
            self.arch,
            self.desktop.as_deref().unwrap_or("unknown"),
            self.session.as_deref().unwrap_or("unknown"),
            if self.app_image { "yes" } else { "no" }
        )
    }
}

pub fn issue_url(env: &Environment) -> String {
    format!(
        "{ISSUES_URL}?title=&body={}",
        percent_encode(&env.issue_body())
    )
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Wall-clock time as `YYYY-MM-DD HH:MM:SS`. The `time` crate refuses the local offset in a
/// multi-threaded process, which would silently put UTC into the log.
fn timestamp() -> String {
    let (y, mo, d, h, mi, s) = local_time();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

fn local_time() -> (i32, u32, u32, u32, u32, u32) {
    #[cfg(unix)]
    {
        // SAFETY: `localtime_r` only writes into the zeroed struct we own.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if !libc::localtime_r(&now, &mut tm).is_null() {
                return (
                    tm.tm_year + 1900,
                    (tm.tm_mon + 1) as u32,
                    tm.tm_mday as u32,
                    tm.tm_hour as u32,
                    tm.tm_min as u32,
                    tm.tm_sec as u32,
                );
            }
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (
        year,
        month,
        day,
        (rest / 3600) as u32,
        (rest % 3600 / 60) as u32,
        (rest % 60) as u32,
    )
}

/// The logger: one file, 1 MiB each, two older ones kept. Our own crates log from `Info`,
/// everything else from `Warn`, so an HTTP client cannot flood the file.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let mut targets = vec![Target::new(TargetKind::Folder {
        path: log_dir(),
        file_name: Some(LOG_NAME.to_owned()),
    })];
    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
    }
    let mut builder = tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets(targets)
        .rotation_strategy(RotationStrategy::KeepSome(OLD_FILES))
        .max_file_size(MAX_FILE_BYTES)
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {:<5} {}",
                timestamp(),
                record.level(),
                message
                    .to_string()
                    .replace('\n', &format!("\n{CONTINUATION_INDENT}"))
            ))
        })
        .level(log::LevelFilter::Warn);
    for own in [
        "dualfile_lib",
        "dualfile_core",
        "dualfile_remote",
        "dualfile_plugin",
    ] {
        builder = builder.level_for(own, log::LevelFilter::Info);
    }
    builder.build()
}

pub fn log_start() {
    log::info!("{}", Environment::current().start_line());
}

/// Panics end up in the log with their location; the previous hook (message on stderr) still runs.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "unbekannter Stelle".to_owned(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(keine Meldung)".to_owned());
        log::error!("Panic bei {location}: {message}");
        log::logger().flush();
        previous(info);
    }));
}

/// Log files oldest first: rotated ones carry a sortable date in the name, the active one is last.
fn log_files(dir: &Path) -> Vec<PathBuf> {
    let active = format!("{LOG_NAME}.log");
    let rotated_prefix = format!("{LOG_NAME}_");
    let mut rotated: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&rotated_prefix) && n.ends_with(".log"))
        })
        .collect();
    rotated.sort();
    if dir.join(&active).is_file() {
        rotated.push(dir.join(active));
    }
    rotated
}

/// The last `max_lines` lines over all log files, oldest first.
pub fn read_tail(dir: &Path, max_lines: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for file in log_files(dir).iter().rev() {
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut older: Vec<String> = text.lines().map(str::to_owned).collect();
        older.append(&mut lines);
        lines = older;
        if lines.len() >= max_lines {
            break;
        }
    }
    let skip = lines.len().saturating_sub(max_lines);
    lines[skip..].join("\n")
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home.as_os_str().len() > 1 => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[derive(Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogInfoDto {
    pub path: String,
    pub bytes: u64,
}

fn info_for(dir: &Path, home: &Path) -> LogInfoDto {
    let file = dir.join(format!("{LOG_NAME}.log"));
    LogInfoDto {
        path: tilde(&file, home),
        bytes: std::fs::metadata(&file).map_or(0, |m| m.len()),
    }
}

fn context(state: &AppState) -> AnonContext {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let home = state.home_dir();
    let user = var("USER")
        .or_else(|| var("LOGNAME"))
        .or_else(|| {
            Path::new(&home)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    AnonContext {
        home,
        user,
        hostname: hostname(),
        hosts: state.connection_hosts(),
    }
}

fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length and `gethostname` NUL-terminates within it.
        let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) == 0 };
        if ok {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            return String::from_utf8_lossy(&buffer[..end]).into_owned();
        }
    }
    String::new()
}

fn text_for(state: &AppState, anonymize_it: bool) -> String {
    let text = read_tail(&log_dir(), VIEW_LINES);
    if anonymize_it {
        anonymize(&text, &context(state))
    } else {
        text
    }
}

/// `dualfile-log-YYYYMMDD-HHMM.txt`, or with `-2`, `-3` … when the name is taken. The file is
/// created exclusively, so nothing is ever overwritten.
fn save_exclusive(dir: &Path, stamp: &str, text: &str) -> Result<PathBuf, UiError> {
    for attempt in 1..=1000 {
        let name = if attempt == 1 {
            format!("dualfile-log-{stamp}.txt")
        } else {
            format!("dualfile-log-{stamp}-{attempt}.txt")
        };
        let target = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .and_then(|()| file.write_all(b"\n"))
                    .map_err(|_| {
                        UiError::new("io", "Die Logdatei konnte nicht gespeichert werden")
                    })?;
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(UiError::new("permissionDenied", "Zugriff verweigert"))
            }
            Err(_) => {
                return Err(UiError::new(
                    "io",
                    "Die Logdatei konnte nicht gespeichert werden",
                ))
            }
        }
    }
    Err(UiError::new(
        "exists",
        "Es gibt schon zu viele Dateien mit diesem Namen",
    ))
}

/// The folder for a saved log: an existing local folder given as a normal path.
fn local_folder(raw: &str, home: &Path) -> Result<PathBuf, UiError> {
    let parsed = crate::validate::parse_path(raw, home)?;
    if parsed.scheme() != dualfile_core::vfs::Scheme::Local {
        return Err(UiError::new(
            "invalidPath",
            "Die Logdatei kann nur in einen lokalen Ordner gespeichert werden",
        ));
    }
    let path = parsed.path().to_path_buf();
    if !path.is_dir() {
        return Err(UiError::new("notFound", "Der Ordner existiert nicht"));
    }
    Ok(path)
}

fn file_stamp() -> String {
    let (y, mo, d, h, mi, _) = local_time();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}")
}

/// One log line about a connection attempt. Only protocol, host, port and user name go in; the
/// error is the already sanitized one the UI gets, never the raw transport text, and no secret is
/// ever passed to this function.
pub fn connection_line(
    profile: &dualfile_core::connections::ConnectionProfile,
    failure: Option<&UiError>,
) -> String {
    // URL form, so the anonymizer replaces host and user even after the profile is gone.
    let scheme = format!("{:?}", profile.protocol).to_ascii_lowercase();
    let user = if profile.user.is_empty() {
        String::new()
    } else {
        format!("{}@", profile.user)
    };
    let target = format!("{scheme}://{user}{}:{}", profile.host, profile.port);
    match failure {
        None => format!("Verbindung {target} aufgebaut"),
        Some(error) => format!(
            "Verbindung {target} fehlgeschlagen: {} ({})",
            error.message, error.kind
        ),
    }
}

pub fn sanitize_ui_message(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_UI_MESSAGE_CHARS)
        .collect()
}

#[tauri::command]
pub async fn log_info(state: Shared<'_>) -> Result<LogInfoDto, UiError> {
    let home = PathBuf::from(state.home_dir());
    Ok(info_for(&log_dir(), &home))
}

#[tauri::command]
pub async fn log_read(state: Shared<'_>, anonymize: bool) -> Result<String, UiError> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || text_for(&state, anonymize))
        .await
        .map_err(|_| UiError::internal())
}

#[tauri::command]
pub async fn log_save(state: Shared<'_>, dir: String, anonymize: bool) -> Result<String, UiError> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let folder = local_folder(&dir, Path::new(&state.home_dir()))?;
        let text = text_for(&state, anonymize);
        let saved = save_exclusive(&folder, &file_stamp(), &text)?;
        Ok(saved.display().to_string())
    })
    .await
    .map_err(|_| UiError::internal())?
}

#[tauri::command]
pub async fn log_open_folder() -> Result<(), UiError> {
    tauri::async_runtime::spawn_blocking(|| {
        let dir = log_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|_| UiError::new("io", "Der Log-Ordner konnte nicht angelegt werden"))?;
        crate::state::open_with_system(&dir)
    })
    .await
    .map_err(|_| UiError::internal())?
}

#[tauri::command]
pub async fn report_issue_url() -> Result<String, UiError> {
    Ok(issue_url(&Environment::current()))
}

#[tauri::command]
pub async fn log_frontend(level: String, message: String) -> Result<(), UiError> {
    let message = sanitize_ui_message(&message);
    match level.as_str() {
        "error" => log::error!(target: "dualfile_lib::ui", "Oberfläche: {message}"),
        "warn" => log::warn!(target: "dualfile_lib::ui", "Oberfläche: {message}"),
        "info" => log::info!(target: "dualfile_lib::ui", "Oberfläche: {message}"),
        _ => return Err(UiError::new("invalidInput", "Ungültige Protokollstufe")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_prefers_an_absolute_state_home() {
        let dir = resolve_log_dir(Some("/s".into()), Some("/home/a".into()));
        assert_eq!(dir, PathBuf::from("/s/dualfile"));
        let dir = resolve_log_dir(Some("rel".into()), Some("/home/a".into()));
        assert_eq!(dir, PathBuf::from("/home/a/.local/state/dualfile"));
        let dir = resolve_log_dir(None, Some("/home/a".into()));
        assert_eq!(dir, PathBuf::from("/home/a/.local/state/dualfile"));
    }

    #[test]
    fn start_line_names_version_system_desktop_and_package() {
        let env = Environment {
            version: "0.3.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            desktop: Some("KDE".into()),
            session: Some("wayland".into()),
            app_image: true,
        };
        assert_eq!(
            env.start_line(),
            "Dualfile 0.3.0 gestartet · linux x86_64 · KDE (wayland) · AppImage"
        );
        let bare = Environment {
            app_image: false,
            desktop: None,
            session: None,
            ..env
        };
        assert_eq!(
            bare.start_line(),
            "Dualfile 0.3.0 gestartet · linux x86_64 · unbekannt (unbekannt) · installiert"
        );
    }

    #[test]
    fn issue_url_is_a_short_https_link_with_the_environment_in_the_body() {
        let env = Environment {
            version: "0.3.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            desktop: Some("GNOME".into()),
            session: Some("x11".into()),
            app_image: false,
        };
        let url = issue_url(&env);
        assert!(url.starts_with("https://github.com/Bexxs75/dualfile/issues/new?title=&body="));
        assert!(crate::commands::is_safe_external_url(&url));
        assert!(url.contains("Dualfile%3A%200.3.0"));
        assert!(url.contains("GNOME%20%28x11%29"));
        assert!(url.contains("anonymized%20log%20file"));
        assert!(!url.contains(' ') && !url.contains('\n'));
    }

    #[test]
    fn read_tail_joins_rotated_files_oldest_first_and_cuts_to_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let w = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        w("dualfile_2026-01-02_00-00-00.log", "b1\nb2\n");
        w("dualfile_2026-01-01_00-00-00.log", "a1\na2\n");
        w("dualfile.log", "c1\nc2\n");
        w("other.log", "x\n");
        assert_eq!(read_tail(dir.path(), 500), "a1\na2\nb1\nb2\nc1\nc2");
        assert_eq!(read_tail(dir.path(), 3), "b2\nc1\nc2");
        assert_eq!(read_tail(&dir.path().join("missing"), 5), "");
    }

    #[test]
    fn info_shows_a_tilde_path_and_the_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dualfile.log"), "12345").unwrap();
        let home = dir.path().parent().unwrap().to_path_buf();
        let info = info_for(dir.path(), &home);
        assert!(info.path.starts_with("~/") && info.path.ends_with("/dualfile.log"));
        assert_eq!(info.bytes, 5);
        assert_eq!(info_for(&dir.path().join("none"), &home).bytes, 0);
    }

    #[test]
    fn saving_never_overwrites_and_numbers_clashes() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_exclusive(dir.path(), "20261116-1500", "one").unwrap();
        let second = save_exclusive(dir.path(), "20261116-1500", "two").unwrap();
        let third = save_exclusive(dir.path(), "20261116-1500", "three").unwrap();
        let names: Vec<_> = [&first, &second, &third]
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "dualfile-log-20261116-1500.txt",
                "dualfile-log-20261116-1500-2.txt",
                "dualfile-log-20261116-1500-3.txt"
            ]
        );
        assert_eq!(std::fs::read_to_string(first).unwrap(), "one\n");
    }

    #[test]
    fn the_target_folder_must_be_an_existing_local_absolute_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let kind = |raw: &str| local_folder(raw, home).unwrap_err().kind;
        assert!(local_folder(dir.path().to_str().unwrap(), home).is_ok());
        assert_eq!(kind("relative/dir"), "invalidPath");
        assert_eq!(
            kind(&format!("{}/../x", dir.path().display())),
            "invalidPath"
        );
        assert_eq!(kind("ftp:abc!/x"), "invalidPath");
        assert_eq!(kind("archive:/a.zip!/x"), "invalidPath");
        assert_eq!(kind(&format!("{}/nope", dir.path().display())), "notFound");
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(kind(file.to_str().unwrap()), "notFound");
    }

    #[test]
    fn ui_messages_lose_control_characters_and_are_cut() {
        assert_eq!(sanitize_ui_message("a\nb\r\tc\u{7}d"), "a b  c d");
        assert_eq!(sanitize_ui_message(&"x".repeat(5000)).chars().count(), 2000);
    }

    #[test]
    fn connection_lines_carry_no_secret() {
        use dualfile_remote::error::TransportError;
        use dualfile_remote::manager::ConnectError;
        let profile = dualfile_core::connections::ConnectionProfile {
            host: "nas.example".into(),
            port: 445,
            user: "otto".into(),
            ..Default::default()
        };
        let password = "hunter2-Pa55";
        // A server reply that echoes the password, as some servers do.
        let raw = ConnectError::Transport(TransportError::reply(
            550,
            &format!("login failed for otto:{password}"),
        ));
        assert!(format!("{raw:?}").contains(password));
        let line = connection_line(&profile, Some(&crate::convert::connect_error(&raw)));
        assert!(!line.contains(password), "{line}");
        assert!(line.contains("ftps://otto@nas.example:445"), "{line}");
        let raw = ConnectError::Transport(TransportError::Connection(format!(
            "host nas.example {password}"
        )));
        let line = connection_line(&profile, Some(&crate::convert::connect_error(&raw)));
        assert!(!line.contains(password), "{line}");
        assert!(!connection_line(&profile, None).contains(password));
    }

    #[test]
    fn timestamp_has_the_log_format() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 19);
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], " ");
        assert_eq!(file_stamp().len(), 13);
    }
}
