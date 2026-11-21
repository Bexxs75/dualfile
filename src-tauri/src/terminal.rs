//! Local PTYs. Reader threads own only a weak manager reference so shutdown can release state.
use crate::convert::UiError;
use base64::{engine::general_purpose::STANDARD, Engine};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::fd::{AsRawFd, BorrowedFd},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use tauri::{Emitter, Manager, State};

const MAX_SESSIONS: usize = 8;

#[derive(Clone, Serialize)]
pub struct Output {
    id: u64,
    data: String,
}
#[derive(Clone, Serialize)]
pub struct Exit {
    id: u64,
    code: Option<i32>,
}

trait Sink: Send + Sync + 'static {
    fn output(&self, event: Output);
    fn exit(&self, event: Exit);
}
struct TauriSink(tauri::AppHandle);
impl Sink for TauriSink {
    fn output(&self, event: Output) {
        let _ = self.0.emit("terminal-output", event);
    }
    fn exit(&self, event: Exit) {
        let _ = self.0.emit("terminal-exit", event);
    }
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    stopped: Arc<AtomicBool>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Always reap, including on errors after spawning and during application shutdown.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
pub struct TerminalManager {
    sessions: Mutex<HashMap<u64, Session>>,
    next_id: AtomicU64,
    shutting_down: AtomicBool,
}

fn io_error(error: impl std::fmt::Display) -> UiError {
    UiError::new("io", format!("Terminal: {error}"))
}
fn size(rows: u16, cols: u16) -> Result<PtySize, UiError> {
    if rows == 0 || cols == 0 {
        return Err(UiError::new(
            "invalidInput",
            "Terminal dimensions must be positive",
        ));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
fn local_cwd(cwd: Option<String>) -> PathBuf {
    cwd.map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(crate::home_dir)
}
fn apply_env_plan(command: &mut CommandBuilder, plan: &dualfile_core::appenv::EnvPlan) {
    for key in &plan.remove {
        command.env_remove(key);
    }
    for (key, value) in &plan.replace {
        command.env(key, value);
    }
}

/// Strip the AppImage runtime paths the bundle injects (LD_LIBRARY_PATH, a PATH/GTK/GIO/… overlay
/// into the mount) from the shell's environment, so it loads system libraries and finds system
/// commands instead of the bundled ones. Mirrors the clean environment used for xdg-open; a no-op
/// outside an AppImage (APPDIR unset).
fn apply_clean_env(command: &mut CommandBuilder) {
    let plan = dualfile_core::appenv::clean_env(
        std::env::vars_os(),
        std::env::var_os("APPDIR").as_deref(),
    );
    apply_env_plan(command, &plan);
}

/// The embedded terminal runs a plain shell on purpose: it ignores the user's
/// `$SHELL` and interactive startup files so it stays simple (no greeting, no
/// custom prompt, no login banner). fish with `--no-config` when installed (for
/// inline command suggestions, still without the user's config), otherwise bash
/// with `--norc`, otherwise `/bin/sh`.
fn plain_shell(fish: Option<&str>, bash_exists: bool) -> (String, Vec<&'static str>) {
    if let Some(fish) = fish {
        (fish.to_string(), vec!["--no-config"])
    } else if bash_exists {
        ("/bin/bash".to_string(), vec!["--norc"])
    } else {
        ("/bin/sh".to_string(), vec![])
    }
}

fn find_fish() -> Option<&'static str> {
    ["/usr/bin/fish", "/usr/local/bin/fish", "/bin/fish"]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
}

/// The user's own `$SHELL` (so their config, prompt and greeting run), falling
/// back to the plain shell when `$SHELL` is unset or missing.
fn login_shell() -> (String, Vec<&'static str>) {
    match std::env::var("SHELL") {
        Ok(s) if !s.is_empty() && std::path::Path::new(&s).is_file() => (s, vec![]),
        _ => plain_shell(find_fish(), std::path::Path::new("/bin/bash").is_file()),
    }
}

fn shell_command(cwd: Option<String>, mode: &str) -> CommandBuilder {
    if dualfile_core::packaging::packaging() == dualfile_core::packaging::Packaging::Flatpak {
        return flatpak_shell_command(cwd, mode);
    }
    let bash = std::path::Path::new("/bin/bash").is_file();
    let (program, args) = match mode {
        "login" => login_shell(),
        "bash" => plain_shell(None, bash),
        _ => plain_shell(find_fish(), bash),
    };
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.arg("-i");
    command.env("TERM", "xterm-256color");
    apply_clean_env(&mut command);
    command.cwd(local_cwd(cwd));
    command
}

// The host wrapper returns success after an interactive shell exits (even `exit 1`),
// so only a failed host launch selects the sandbox fallback.
fn flatpak_shell_command(cwd: Option<String>, mode: &str) -> CommandBuilder {
    let mut host = dualfile_core::packaging::host_command_with(
        dualfile_core::packaging::Packaging::Flatpak,
        "sh",
    );
    host.args([
        "-c",
        r#"
case "$1" in
 login) if [ -n "$SHELL" ] && [ -x "$SHELL" ]; then "$SHELL" -i; exit 0; fi ;;
 bash) if [ -x /bin/bash ]; then /bin/bash --norc -i; exit 0; fi ;;
 *) if command -v fish >/dev/null 2>&1; then fish --no-config -i; exit 0; fi ;;
esac
if [ -x /bin/bash ]; then /bin/bash --norc -i; else /bin/sh -i; fi
exit 0
"#,
        "dualfile-host-shell",
        mode,
    ]);
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", r#"
"$@"
result=$?
if [ "$result" -eq 0 ]; then exit 0; fi
printf '\nHost-Shell nicht verfügbar; Sandbox-Shell aktiv. / Host shell unavailable; using sandbox shell.\n' >&2
exec /bin/sh -i
"#, "dualfile-terminal"]);
    command.arg(host.get_program());
    command.args(host.get_args());
    command.env("TERM", "xterm-256color");
    command.cwd(local_cwd(cwd));
    command
}

impl TerminalManager {
    fn lock(&self) -> Result<MutexGuard<'_, HashMap<u64, Session>>, UiError> {
        self.sessions.lock().map_err(|_| UiError::internal())
    }

    fn open(
        self: &Arc<Self>,
        command: CommandBuilder,
        rows: u16,
        cols: u16,
        sink: Arc<dyn Sink>,
    ) -> Result<u64, UiError> {
        let dimensions = size(rows, cols)?;
        let mut sessions = self.lock()?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(UiError::new("io", "Terminal manager is shutting down"));
        }
        if sessions.len() >= MAX_SESSIONS {
            return Err(UiError::new(
                "invalidInput",
                "At most 8 terminal sessions may be open",
            ));
        }
        let pair = native_pty_system().openpty(dimensions).map_err(io_error)?;
        let writer = pair.master.take_writer().map_err(io_error)?;
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io_error("PTY has no file descriptor"))?;
        // Keep the descriptor used by poll alive independently of Session's master.
        // SAFETY: the master owns fd and remains alive while it is borrowed and cloned.
        let owned = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .map_err(io_error)?;
        let reader = std::fs::File::from(owned);
        let fd = reader.as_raw_fd();
        // All master duplicates share file status flags. Nonblocking writes let shutdown
        // acquire the session lock even when a shell has stopped consuming pasted input.
        // SAFETY: fd is owned by reader; these fcntl operations take integer arguments.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let child = pair.slave.spawn_command(command).map_err(io_error)?;
        drop(pair.slave);
        let stopped = Arc::new(AtomicBool::new(false));
        let session = Session {
            master: pair.master,
            writer,
            child,
            stopped: stopped.clone(),
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        sessions.insert(id, session);
        let manager = Arc::downgrade(self);
        let started = std::thread::Builder::new()
            .name(format!("terminal-{id}"))
            .spawn(move || {
                let mut reader = reader;
                let mut bytes = [0; 8192];
                let mut code = None;
                loop {
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    // A finite poll lets close stop the reader even when a descendant retains the
                    // slave. The cloned reader keeps this descriptor's underlying PTY alive.
                    let mut poll = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // SAFETY: poll points to one initialized pollfd, valid for this call.
                    let ready = unsafe { libc::poll(&mut poll, 1, 50) };
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if ready < 0 {
                        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        break;
                    }
                    if ready > 0 {
                        match reader.read(&mut bytes) {
                            Ok(0) => break,
                            Ok(n) => sink.output(Output {
                                id,
                                data: STANDARD.encode(&bytes[..n]),
                            }),
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::Interrupted
                                        | std::io::ErrorKind::WouldBlock
                                ) =>
                            {
                                continue
                            }
                            Err(_) => break, // Linux PTYs report EIO when the slave closes.
                        }
                    } else if let Some(manager) = manager.upgrade() {
                        if let Ok(mut sessions) = manager.lock() {
                            if let Some(session) = sessions.get_mut(&id) {
                                if let Ok(Some(status)) = session.child.try_wait() {
                                    code = i32::try_from(status.exit_code()).ok();
                                    break;
                                }
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if let Some(manager) = manager.upgrade() {
                    let session = manager
                        .lock()
                        .ok()
                        .and_then(|mut sessions| sessions.remove(&id));
                    if let Some(mut session) = session {
                        // EOF can precede waitpid readiness. Allow a bounded grace period for
                        // the exit status, then Drop kills a child that only closed its terminal.
                        for _ in 0..20 {
                            match session.child.try_wait() {
                                Ok(Some(status)) => {
                                    code = i32::try_from(status.exit_code()).ok();
                                    break;
                                }
                                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
                                Err(_) => break,
                            }
                        }
                        drop(session);
                    }
                }
                sink.exit(Exit { id, code });
            });
        if let Err(error) = started {
            sessions.remove(&id);
            return Err(io_error(error));
        }
        Ok(id)
    }
    fn write(&self, id: u64, data: &str) -> Result<(), UiError> {
        let mut remaining = data.as_bytes();
        loop {
            let result = {
                let mut sessions = self.lock()?;
                let session = sessions
                    .get_mut(&id)
                    .ok_or_else(|| UiError::new("notFound", "Unknown terminal session"))?;
                if remaining.is_empty() {
                    return Ok(());
                }
                session.writer.write(remaining)
            };
            match result {
                Ok(0) => return Err(io_error("PTY writer closed")),
                Ok(n) => remaining = &remaining[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(e) => return Err(io_error(e)),
            }
        }
    }

    fn resize(&self, id: u64, rows: u16, cols: u16) -> Result<(), UiError> {
        let sessions = self.lock()?;
        let session = sessions
            .get(&id)
            .ok_or_else(|| UiError::new("notFound", "Unknown terminal session"))?;
        session.master.resize(size(rows, cols)?).map_err(io_error)
    }
    fn close(&self, id: u64) -> Result<(), UiError> {
        let session = self.lock()?.remove(&id);
        drop(session);
        Ok(())
    }
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        // Poison must not prevent child cleanup during shutdown.
        let sessions = match self.sessions.lock() {
            Ok(mut s) => std::mem::take(&mut *s),
            Err(e) => std::mem::take(&mut *e.into_inner()),
        };
        drop(sessions);
    }
}
impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type Shared<'a> = State<'a, Arc<TerminalManager>>;
async fn run<T: Send + 'static>(
    manager: Shared<'_>,
    work: impl FnOnce(Arc<TerminalManager>) -> Result<T, UiError> + Send + 'static,
) -> Result<T, UiError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || work(manager))
        .await
        .map_err(io_error)?
}
#[tauri::command]
pub async fn terminal_open(
    app: tauri::AppHandle,
    manager: Shared<'_>,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<u64, UiError> {
    let shell = app
        .state::<Arc<crate::state::AppState>>()
        .load_settings()
        .settings
        .terminal_shell;
    run(manager, move |manager| {
        manager.open(
            shell_command(cwd, &shell),
            rows,
            cols,
            Arc::new(TauriSink(app)),
        )
    })
    .await
}
#[tauri::command]
pub async fn terminal_write(manager: Shared<'_>, id: u64, data: String) -> Result<(), UiError> {
    run(manager, move |manager| manager.write(id, &data)).await
}
#[tauri::command]
pub async fn terminal_resize(
    manager: Shared<'_>,
    id: u64,
    rows: u16,
    cols: u16,
) -> Result<(), UiError> {
    run(manager, move |manager| manager.resize(id, rows, cols)).await
}
#[tauri::command]
pub async fn terminal_close(manager: Shared<'_>, id: u64) -> Result<(), UiError> {
    run(manager, move |manager| manager.close(id)).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn flatpak_terminal_falls_back_with_visible_bilingual_notice() {
        let bin = tempfile::tempdir().unwrap();
        let command = super::flatpak_shell_command(None, "bash");
        let argv = command.get_argv();
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env("PATH", bin.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains("Sandbox-Shell aktiv"));
        assert!(text.contains("Host shell unavailable"));
    }
    #[test]
    fn flatpak_terminal_passes_mode_to_the_host_without_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let spawn = bin.path().join("flatpak-spawn");
        std::fs::write(
            &spawn,
            "#!/bin/sh\n[ \"$1\" = --host ] || exit 1\nprintf 'host shell reached'\n",
        )
        .unwrap();
        std::fs::set_permissions(&spawn, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command = super::flatpak_shell_command(None, "login");
        let argv = command.get_argv();
        assert_eq!(argv.last().unwrap(), "login");
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env("PATH", bin.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "host shell reached"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Sandbox-Shell"));
    }

    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    enum Event {
        Output(Output),
        Exit(Exit),
    }
    struct TestSink(mpsc::Sender<Event>);
    impl Sink for TestSink {
        fn output(&self, event: Output) {
            let _ = self.0.send(Event::Output(event));
        }
        fn exit(&self, event: Exit) {
            let _ = self.0.send(Event::Exit(event));
        }
    }
    fn setup() -> (Arc<TerminalManager>, Arc<dyn Sink>, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        (
            Arc::new(TerminalManager::default()),
            Arc::new(TestSink(tx)),
            rx,
        )
    }
    fn sh(script: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", script]);
        command.cwd("/");
        command
    }
    #[test]
    fn plain_shell_prefers_fish_then_bash_then_sh() {
        assert_eq!(
            plain_shell(Some("/usr/bin/fish"), true),
            ("/usr/bin/fish".to_string(), vec!["--no-config"])
        );
        assert_eq!(
            plain_shell(None, true),
            ("/bin/bash".to_string(), vec!["--norc"])
        );
        assert_eq!(plain_shell(None, false), ("/bin/sh".to_string(), vec![]));
    }
    #[test]
    fn shell_env_drops_appimage_runtime_paths() {
        use std::ffi::OsStr;
        let mut command = CommandBuilder::new("/bin/sh");
        command.env("LD_LIBRARY_PATH", "/tmp/.mount_x/lib");
        command.env("PATH", "/tmp/.mount_x/usr/bin:/usr/bin:/bin");
        command.env("KEEP", "value");
        let plan = dualfile_core::appenv::clean_env(
            [
                ("LD_LIBRARY_PATH".into(), "/tmp/.mount_x/lib".into()),
                ("PATH".into(), "/tmp/.mount_x/usr/bin:/usr/bin:/bin".into()),
                ("KEEP".into(), "value".into()),
            ],
            Some(OsStr::new("/tmp/.mount_x")),
        );
        apply_env_plan(&mut command, &plan);
        assert!(command.get_env("LD_LIBRARY_PATH").is_none());
        assert_eq!(command.get_env("PATH"), Some(OsStr::new("/usr/bin:/bin")));
        assert_eq!(command.get_env("KEEP"), Some(OsStr::new("value")));
    }

    fn collect(rx: &mpsc::Receiver<Event>, id: u64) -> (Vec<u8>, Option<i32>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        loop {
            match rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("PTY event before deadline")
            {
                Event::Output(output) => {
                    assert_eq!(output.id, id);
                    bytes.extend(STANDARD.decode(output.data).unwrap());
                }
                Event::Exit(exit) => {
                    assert_eq!(exit.id, id);
                    return (bytes, exit.code);
                }
            }
        }
    }
    #[test]
    fn output_and_exit_remove_session() {
        let (manager, sink, rx) = setup();
        let id = manager
            .open(sh("printf 'hi\\377'; exit 7"), 24, 80, sink)
            .unwrap();
        assert!(id > 0);
        let (output, code) = collect(&rx, id);
        assert_eq!(output, b"hi\xff");
        assert_eq!(code, Some(7));
        assert!(manager.lock().unwrap().is_empty());
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
    #[test]
    fn echo_round_trip_and_resize() {
        let (manager, sink, rx) = setup();
        let id = manager
            .open(
                sh("read -r line; printf 'reply:%s' \"$line\""),
                24,
                80,
                sink,
            )
            .unwrap();
        manager.resize(id, 30, 100).unwrap();
        let dimensions = manager.lock().unwrap()[&id].master.get_size().unwrap();
        assert_eq!((dimensions.rows, dimensions.cols), (30, 100));
        manager.write(id, "hello\n").unwrap();
        let (bytes, _) = collect(&rx, id);
        assert!(String::from_utf8_lossy(&bytes).contains("reply:hello"));
    }
    #[test]
    fn close_is_idempotent_and_stops_reader() {
        let (manager, sink, rx) = setup();
        let id = manager.open(sh("read -r line"), 24, 80, sink).unwrap();
        manager.close(id).unwrap();
        manager.close(id).unwrap();
        collect(&rx, id);
        assert!(manager.lock().unwrap().is_empty());
        assert!(manager.write(id, "x").is_err());
        assert!(manager.resize(id, 24, 80).is_err());
    }
    #[test]
    fn cap_unknown_ids_and_shutdown() {
        let (manager, sink, rx) = setup();
        assert!(manager.write(123, "x").is_err());
        assert!(manager.resize(123, 24, 80).is_err());
        assert!(manager.open(sh("true"), 0, 80, sink.clone()).is_err());
        for _ in 0..MAX_SESSIONS {
            manager
                .open(sh("read -r line"), 24, 80, sink.clone())
                .unwrap();
        }
        let error = manager.open(sh("true"), 24, 80, sink.clone()).unwrap_err();
        assert!(error.message.contains('8'));
        manager.shutdown();
        assert!(manager.lock().unwrap().is_empty());
        assert!(manager.open(sh("true"), 24, 80, sink).is_err());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exits = 0;
        while exits < MAX_SESSIONS {
            if matches!(
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap(),
                Event::Exit(_)
            ) {
                exits += 1;
            }
        }
    }
    #[test]
    fn cwd_rejects_virtual_relative_missing_and_file_paths() {
        for path in [
            "archive:/tmp/a.zip!/",
            "sftp:one!/",
            "webdav:one!/",
            "plugin:one!/",
            "relative",
            "/definitely-missing-dualfile-pty",
            "/bin/sh",
        ] {
            assert_eq!(local_cwd(Some(path.into())), crate::home_dir());
        }
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            local_cwd(Some(dir.path().display().to_string())),
            dir.path()
        );
    }
}
