//! Remove AppImage runtime paths before starting external system programs.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::process::Command;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnvPlan {
    pub remove: BTreeSet<OsString>,
    pub replace: BTreeMap<OsString, OsString>,
}

impl EnvPlan {
    pub fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        command.envs(&self.replace);
    }
}

/// Plan changes without reading or mutating the process environment.
/// Empty path elements are preserved: on Unix they can mean the current directory.
pub fn clean_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: Option<&OsStr>,
) -> EnvPlan {
    let mut plan = EnvPlan::default();
    let Some(appdir) = appdir else {
        return plan;
    };
    let root = appdir.as_encoded_bytes();
    let root = root.strip_suffix(b"/").unwrap_or(root);
    // After an in-app update the app restarts with the old process environment, so paths of
    // the previous AppImage (still mounted while its runtime lingers) sit next to the current
    // ones. Its bundled libraries would then be loaded by every program started from here.
    // Every AppImage runtime mounts at `<tmp>/.mount_*`, so the previous one shares the parent
    // directory of the current APPDIR.
    let mount_prefix = root
        .iter()
        .rposition(|b| *b == b'/')
        .filter(|_| {
            root.rsplit(|b| *b == b'/')
                .next()
                .is_some_and(|name| name.starts_with(b".mount_"))
        })
        .map(|slash| [&root[..slash], b"/.mount_".as_slice()].concat());
    let in_any_mount = |entry: &[u8]| {
        mount_prefix
            .as_deref()
            .is_some_and(|p| entry.starts_with(p))
    };
    let under_appdir = |entry: &[u8]| {
        (!appdir.is_empty()
            && (entry == root
                || entry
                    .strip_prefix(root)
                    .is_some_and(|tail| tail.starts_with(b"/"))))
            || in_any_mount(entry)
    };
    for (key, value) in vars {
        if key == "GTK_THEME" {
            continue;
        }
        if matches!(
            key.to_str(),
            Some(
                "APPDIR"
                    | "APPIMAGE"
                    | "ARGV0"
                    | "OWD"
                    | "GDK_PIXBUF_MODULE_FILE"
                    | "GTK_EXE_PREFIX"
                    | "GTK_DATA_PREFIX"
                    | "GTK_PATH"
                    | "GTK_IM_MODULE_FILE"
                    | "GIO_MODULE_DIR"
                    | "GIO_EXTRA_MODULES"
                    | "GST_PLUGIN_SYSTEM_PATH"
                    | "GST_PLUGIN_SYSTEM_PATH_1_0"
                    | "GSETTINGS_SCHEMA_DIR"
                    | "PYTHONPATH"
                    | "QT_PLUGIN_PATH"
                    | "PERLLIB"
            )
        ) {
            plan.remove.insert(key);
            continue;
        }
        let entries: Vec<_> = value.as_encoded_bytes().split(|b| *b == b':').collect();
        let kept: Vec<_> = entries
            .iter()
            .copied()
            .filter(|v| !under_appdir(v))
            .collect();
        if entries.len() == kept.len() {
            continue;
        }
        let cleaned = kept.join(&b':');
        if cleaned.is_empty() {
            plan.remove.insert(key);
        } else {
            // SAFETY: splitting at an ASCII colon preserves encoding boundaries, and
            // every remaining byte comes from the same valid OsStr encoding.
            let cleaned = unsafe { OsStr::from_encoded_bytes_unchecked(&cleaned) };
            plan.replace.insert(key, cleaned.to_owned());
        }
    }
    plan
}

/// Choose a working directory outside the AppImage mount, without reading global state.
pub fn external_current_dir(
    appdir: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<std::path::PathBuf> {
    appdir.map(|_| {
        home.map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| "/".into())
    })
}

pub fn apply_external_current_dir(command: &mut Command) {
    if let Some(path) = external_current_dir(
        std::env::var_os("APPDIR").as_deref(),
        std::env::var_os("HOME").as_deref(),
    ) {
        command.current_dir(path);
    }
}

pub fn apply_clean_env(command: &mut Command) {
    let appdir = std::env::var_os("APPDIR");
    clean_env(std::env::vars_os(), appdir.as_deref()).apply(command);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(vars: &[(&str, &str)]) -> EnvPlan {
        clean_env(
            vars.iter().map(|(k, v)| ((*k).into(), (*v).into())),
            Some(OsStr::new("/tmp/.mount_x")),
        )
    }

    #[test]
    fn paths_of_a_previous_appimage_mount_are_removed_too() {
        // Environment of the process restarted by an in-app update.
        let result = plan(&[
            ("APPDIR", "/tmp/.mount_x"),
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_x/usr/lib/:/tmp/.mount_old.AbC/usr/lib/:/opt/lib",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_x/usr/share:/usr/share:/tmp/.mount_old.AbC/usr/share:/usr/local/share",
            ),
            (
                "PATH",
                "/tmp/.mount_x/usr/bin/:/tmp/.mount_old.AbC/usr/bin/:/usr/bin",
            ),
            ("HOME_VALUE", "/home/me/.mount_notes/file"),
        ]);
        assert_eq!(
            result.replace.get(OsStr::new("LD_LIBRARY_PATH")).unwrap(),
            "/opt/lib"
        );
        assert_eq!(
            result.replace.get(OsStr::new("XDG_DATA_DIRS")).unwrap(),
            "/usr/share:/usr/local/share"
        );
        assert_eq!(result.replace.get(OsStr::new("PATH")).unwrap(), "/usr/bin");
        assert!(!result.replace.contains_key(OsStr::new("HOME_VALUE")));
        assert!(!result.remove.contains(OsStr::new("HOME_VALUE")));
    }

    #[test]
    fn cleans_runtime_paths_and_preserves_user_values() {
        let result = plan(&[
            ("APPDIR", "/tmp/.mount_x"),
            ("APPIMAGE", "/home/me/Dualfile.AppImage"),
            ("OWD", "/home/me"),
            ("GTK_PATH", "/usr/lib/gtk"),
            ("PATH", "/tmp/.mount_x/usr/bin:/usr/bin:/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_x/lib"),
            (
                "XDG_DATA_DIRS",
                "/usr/share:/tmp/.mount_x/usr/share:/usr/local/share",
            ),
            ("UNKNOWN", "/tmp/.mount_x/other"),
            ("ROOT", "/tmp/.mount_x"),
            ("PREFIX", "/tmp/.mountx/lib"),
            ("GDK_BACKEND", "wayland"),
            ("GTK_THEME", "/tmp/.mount_x/theme"),
            ("USER_VALUE", "hello"),
        ]);
        assert_eq!(
            result.remove,
            [
                "APPDIR",
                "APPIMAGE",
                "OWD",
                "GTK_PATH",
                "LD_LIBRARY_PATH",
                "UNKNOWN",
                "ROOT"
            ]
            .map(OsString::from)
            .into()
        );
        assert_eq!(
            result.replace,
            [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("XDG_DATA_DIRS"),
                    OsString::from("/usr/share:/usr/local/share")
                )
            ]
            .into()
        );
    }

    #[test]
    fn handles_unknown_lists_empty_elements_and_path_boundaries() {
        let result = plan(&[
            ("CUSTOM", ":/tmp/.mount_x/lib::/tmp/.mountx/lib:"),
            ("LD_PRELOAD", "/tmp/.mount_x/a:/usr/lib/b"),
            ("XDG_DATA_DIRS", "/tmp/.mount_x/share:/tmp/.mount_x"),
            ("GDK_BACKEND", "/tmp/.mount_x/backend"),
            ("EMPTY", "::"),
        ]);
        assert_eq!(result.replace[OsStr::new("CUSTOM")], "::/tmp/.mountx/lib:");
        assert_eq!(result.replace[OsStr::new("LD_PRELOAD")], "/usr/lib/b");
        assert_eq!(
            result.remove,
            ["XDG_DATA_DIRS", "GDK_BACKEND"].map(OsString::from).into()
        );
        assert!(!result.replace.contains_key(OsStr::new("EMPTY")));
    }

    #[test]
    fn no_appdir_means_no_changes() {
        assert_eq!(
            clean_env(
                [
                    ("PATH".into(), "/tmp/.mount_x/bin:/bin".into()),
                    ("APPIMAGE".into(), "image".into())
                ],
                None
            ),
            EnvPlan::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let result = clean_env(
            [(
                "PATH".into(),
                OsString::from_vec(b"/tmp/.mount_x/bin:/system/\xff".to_vec()),
            )],
            Some(OsStr::new("/tmp/.mount_x")),
        );
        assert_eq!(
            result.replace[OsStr::new("PATH")],
            OsString::from_vec(b"/system/\xff".to_vec())
        );
    }
}

#[cfg(test)]
mod cwd_tests {
    use super::*;
    #[test]
    fn appimage_external_current_dir_uses_home_or_root() {
        for (appdir, home, expected) in [
            (
                Some("/tmp/.mount_x"),
                Some("/home/demo"),
                Some("/home/demo"),
            ),
            (Some("/tmp/.mount_x"), None, Some("/")),
            (Some("/tmp/.mount_x"), Some(""), Some("/")),
            (Some("/tmp/.mount_x"), Some("relative"), Some("/")),
            (None, Some("/home/demo"), None),
        ] {
            let mut command = Command::new("xdg-open");
            if let Some(path) = external_current_dir(appdir.map(OsStr::new), home.map(OsStr::new)) {
                command.current_dir(path);
            }
            assert_eq!(
                command.get_current_dir(),
                expected.map(std::path::Path::new)
            );
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HelperError {
    Unavailable,
    Failed(String),
    Timeout,
}

/// The response deadline includes process creation and environment/CWD preparation, which
/// can themselves block on a filesystem. Such a syscall retains a fixed helper worker.
pub fn run_isolated(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(), HelperError> {
    crate::places_pool::helpers()
        .run(timeout + std::time::Duration::from_millis(200), move || {
            run_bounded(&mut command, timeout)
        })
        .unwrap_or(Err(HelperError::Timeout))
}

/// Only the short-lived intermediary is tracked/reaped. The desktop application runs
/// in a separate session and never consumes a helper slot for its lifetime.
pub fn launch_isolated(
    command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(), HelperError> {
    launch_in_pool(crate::places_pool::helpers(), command, timeout)
}
fn launch_in_pool(
    pool: &crate::places_pool::Pool,
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(), HelperError> {
    pool.run(timeout + std::time::Duration::from_millis(200), move || {
        use std::os::unix::process::CommandExt;
        // SAFETY: after fork only async-signal-safe libc calls are made. The grandchild
        // inherits Command's exec-error pipe, so spawn still reports an exec failure.
        unsafe {
            command.pre_exec(|| {
                let pid = libc::fork();
                if pid < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if pid > 0 {
                    libc::_exit(0);
                }
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // stdout is /dev/null. Do not leave the application attached to the
                // intermediary's diagnostic pipe, whose reader closes on return.
                if libc::dup2(libc::STDOUT_FILENO, libc::STDERR_FILENO) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        run_bounded(&mut command, timeout)
    })
    .unwrap_or(Err(HelperError::Timeout))
}
/// Bound concurrent helpers, diagnostics and cleanup. Unreaped children retain their slot.
pub fn run_bounded(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Result<(), HelperError> {
    use std::{
        io::Read,
        process::{Child, Stdio},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
        time::{Duration, Instant},
    };
    static ACTIVE: AtomicUsize = AtomicUsize::new(0);
    static REAP: Mutex<Vec<Child>> = Mutex::new(Vec::new());
    {
        let mut pending = REAP.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain_mut(|child| {
            if matches!(child.try_wait(), Ok(Some(_))) {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
                false
            } else {
                true
            }
        });
    }
    static REAPER: std::sync::Once = std::sync::Once::new();
    REAPER.call_once(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(200));
            REAP.lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain_mut(|child| {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        ACTIVE.fetch_sub(1, Ordering::SeqCst);
                        false
                    } else {
                        true
                    }
                });
        });
    });
    loop {
        let active = ACTIVE.load(Ordering::SeqCst);
        if active >= 4 {
            return Err(HelperError::Timeout);
        }
        if ACTIVE
            .compare_exchange(active, active + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            break;
        }
    }
    struct Slot(bool);
    impl Drop for Slot {
        fn drop(&mut self) {
            if self.0 {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
    let mut slot = Slot(true);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    apply_clean_env(command);
    apply_external_current_dir(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            HelperError::Unavailable
        } else {
            HelperError::Failed(String::new())
        }
    })?;
    let mut stderr = child.stderr.take();
    let nonblocking = stderr.as_ref().is_none_or(|stderr| {
        rustix::fs::fcntl_getfl(stderr)
            .and_then(|flags| rustix::fs::fcntl_setfl(stderr, flags | rustix::fs::OFlags::NONBLOCK))
            .is_ok()
    });
    fn drain(stderr: &mut Option<std::process::ChildStderr>, output: &mut Vec<u8>) {
        let Some(stderr) = stderr else {
            return;
        };
        let mut buffer = [0; 4096];
        for _ in 0..16 {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => output.extend_from_slice(&buffer[..n.min(8192 - output.len())]),
            }
        }
    }
    let start = Instant::now();
    let mut output = Vec::new();
    let result = loop {
        if !nonblocking {
            break Err(HelperError::Failed(String::new()));
        }
        drain(&mut stderr, &mut output);
        match child.try_wait() {
            Ok(Some(status)) => {
                // The process can write and exit between the preceding read and try_wait.
                drain(&mut stderr, &mut output);
                break if status.success() {
                    Ok(())
                } else {
                    Err(HelperError::Failed(safe_helper_error(&output)))
                };
            }
            Err(_) => break Err(HelperError::Failed(String::new())),
            _ => {}
        }
        if start.elapsed() >= timeout {
            break Err(HelperError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if result.is_err() {
        #[cfg(unix)]
        {
            // SAFETY: the child starts a dedicated process group; a negative pid addresses it.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
        }
        let _ = child.kill();
    }
    // No blocking wait, including after SIGKILL: uninterruptible I/O can outlive it.
    let cleanup = Instant::now();
    while cleanup.elapsed() < Duration::from_millis(100) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return result;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    slot.0 = false;
    REAP.lock().unwrap_or_else(|e| e.into_inner()).push(child);
    result
}
fn safe_helper_error(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).to_lowercase();
    // Only fixed diagnostic codes/text survive; never interpolate sources, usernames or paths.
    for (needle, message) in [
        ("busy", "busy"),
        ("permission denied", "not permitted"),
        ("not permitted", "not permitted"),
        ("not found", "not found"),
        ("host unreachable", "Host unreachable"),
    ] {
        if text.contains(needle) {
            return message.into();
        }
    }
    String::new()
}
#[cfg(test)]
mod bounded_helper_tests {
    use super::*;
    #[test]
    fn open_desktop_applications_do_not_consume_mount_helper_slots() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..4 {
            let mut command = std::process::Command::new("sh");
            command
                .args([
                    "-c",
                    "printf started > \"$1\"; sleep 1; printf done > \"$1\"",
                    "launcher",
                ])
                .arg(tmp.path().join(i.to_string()));
            assert_eq!(
                launch_isolated(command, std::time::Duration::from_secs(1)),
                Ok(())
            );
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (0..4).any(|i| !tmp.path().join(i.to_string()).exists()) {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!((0..4).all(
            |i| std::fs::read_to_string(tmp.path().join(i.to_string())).unwrap() == "started"
        ));
        assert_eq!(
            run_isolated(
                std::process::Command::new("true"),
                std::time::Duration::from_secs(1)
            ),
            Ok(())
        );
        while (0..4)
            .any(|i| std::fs::read_to_string(tmp.path().join(i.to_string())).unwrap() != "done")
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            launch_isolated(
                std::process::Command::new(tmp.path().join("missing")),
                std::time::Duration::from_secs(1)
            ),
            Err(HelperError::Unavailable)
        );
    }
    #[test]
    fn busy_launch_pool_reports_failure_and_never_starts_expired_work() {
        let pool = crate::places_pool::Pool::new(1);
        let (release, blocked) = std::sync::mpsc::channel();
        pool.run(std::time::Duration::from_millis(20), move || {
            blocked.recv().unwrap()
        });
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("must-not-start");
        let mut command = std::process::Command::new("touch");
        command.arg(&marker);
        assert_eq!(
            launch_in_pool(&pool, command, std::time::Duration::from_millis(10)),
            Err(HelperError::Timeout)
        );
        release.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        // The expired queued job may still occupy the queue until the released worker
        // receives it. Wait for recovery instead of racing that receive.
        while pool
            .run(std::time::Duration::from_millis(20), || 7)
            .is_none()
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!marker.exists());
    }
    #[test]
    fn desktop_launcher_timeout_does_not_kill_foreground_application() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("finished");
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "sleep 0.35; printf done > \"$1\"", "launcher"])
            .arg(&marker);
        assert_eq!(
            launch_isolated(command, std::time::Duration::from_millis(20)),
            Ok(())
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !marker.exists() {
            assert!(std::time::Instant::now() < until, "application was killed");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[test]
    fn final_stderr_before_exit_is_always_drained() {
        for _ in 0..20 {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "printf 'permission denied' >&2; exit 1"]);
            assert_eq!(
                run_bounded(&mut command, std::time::Duration::from_secs(1)),
                Err(HelperError::Failed("not permitted".into()))
            );
        }
    }
    #[test]
    fn credentials_never_survive_errors() {
        let text =
            b"credentials=/root/secret username=alice //alice:secret@host/share permission denied";
        assert_eq!(safe_helper_error(text), "not permitted");
        assert_eq!(
            safe_helper_error(b"credentials=/root/secret username=alice"),
            ""
        );
    }
    #[test]
    fn helper_timeout_has_a_response_deadline() {
        let start = std::time::Instant::now();
        assert_eq!(
            run_bounded(
                std::process::Command::new("sh").args(["-c", "exec sleep 10"]),
                std::time::Duration::from_millis(30)
            ),
            Err(HelperError::Timeout)
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
