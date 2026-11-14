//! Starting and talking to a plugin process.
//!
//! The process starts from the verified executable descriptor (`execveat` with an empty path),
//! with an empty environment plus a fixed list, a fixed argument list, an empty temporary
//! working folder, its own process group, resource limits, `no_new_privs`, a parent-death
//! signal and the sandbox. Standard input/output carry the protocol; standard error is read
//! with a hard cap and kept only as cleaned diagnostic text. A process that breaks a rule or
//! the time limit is killed with its whole group and always reaped (no zombies).

use crate::error::{Diagnostic, PluginError, PluginResult};
use crate::manifest::Kind;
use crate::package::{refuse_root, ExeCheck, PinnedState, VerifiedPackage};
use crate::runtime::{PluginRuntime, ProcessPermit};
use crate::sandbox::{self, FAIL_CHANGED, FAIL_SANDBOX, FAIL_SETUP};
use serde_json::{Map, Value};
use std::ffi::CString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The only argument besides the program name: the protocol identification.
pub const PROTOCOL_ARG: &str = "--dualfile-plugin-api=1";

/// Resource limits of a plugin process (`setrlimit`).
#[derive(Clone, Copy, Debug)]
pub struct ResourceLimits {
    /// CPU seconds over the life of the process.
    pub cpu_seconds: u64,
    /// Address space in bytes.
    pub address_space: u64,
    /// Open files.
    pub open_files: u64,
    /// Processes and threads the plugin may create (0 = a single-threaded plugin).
    pub threads: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_seconds: 600,
            address_space: 1 << 30,
            open_files: 32,
            threads: 0,
        }
    }
}

/// Everything the child needs between `fork` and `exec`, prepared in the parent. Only plain
/// system calls touch it in the child.
struct ChildSetup {
    exe_fd: RawFd,
    exe_check: ExeCheck,
    argv: Vec<*const libc::c_char>,
    envp: Vec<*const libc::c_char>,
    limits: [(libc::__rlimit_resource_t, u64); 6],
    parent: libc::pid_t,
    plan: Option<sandbox::SandboxPlan>,
}

// SAFETY: the raw pointers point into CStrings that are owned by the same `Setup` wrapper and
// outlive the closure; they are only read.
unsafe impl Send for ChildSetup {}
unsafe impl Sync for ChildSetup {}

impl ChildSetup {
    /// Child side: limits, parent-death signal, no_new_privs, descriptor hygiene, sandbox,
    /// exec. Returns only on failure (an errno value for the spawn error pipe).
    unsafe fn run(&mut self) -> i32 {
        for (resource, value) in self.limits {
            let limit = libc::rlimit {
                rlim_cur: value as libc::rlim_t,
                rlim_max: value as libc::rlim_t,
            };
            if libc::setrlimit(resource, &limit) != 0 {
                return FAIL_SETUP;
            }
        }
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
            return FAIL_SETUP;
        }
        // the parent may have died before the signal was armed
        if libc::getppid() != self.parent {
            return FAIL_SETUP;
        }
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return FAIL_SETUP;
        }
        // everything above stderr closes on exec (the executable descriptor itself, too)
        if libc::syscall(
            libc::SYS_close_range,
            3u32,
            u32::MAX,
            4u32, /* CLOSE_RANGE_CLOEXEC */
        ) != 0
        {
            let mut fd = 3;
            while fd < 4096 {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
                fd += 1;
            }
        }
        // last look at the content right before the exec (before the sandbox closes `pread`)
        if !self.exe_check.matches(self.exe_fd) {
            return FAIL_CHANGED;
        }
        if let Some(plan) = self.plan.as_mut() {
            if let Err(code) = plan.enforce() {
                return code;
            }
        }
        libc::syscall(
            libc::SYS_execveat,
            self.exe_fd as libc::c_long,
            c"".as_ptr(),
            self.argv.as_ptr(),
            self.envp.as_ptr(),
            libc::AT_EMPTY_PATH as libc::c_long,
        );
        *libc::__errno_location()
    }
}

/// A running plugin process with its pipes.
pub(crate) struct Process {
    child: Child,
    stdin: OwnedFd,
    stdout: OwnedFd,
    stderr: OwnedFd,
    stderr_text: Vec<u8>,
    stderr_dropped: u64,
    _permit: ProcessPermit,
}

fn set_nonblocking(fd: RawFd) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

impl Process {
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// True while the process has not exited (and is not yet reaped by someone else).
    pub(crate) fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kills the whole process group and reaps the process. Safe to call repeatedly.
    pub(crate) fn kill_and_reap(&mut self) {
        let pgid = self.child.id() as libc::pid_t;
        // SAFETY: signalling the group we created; the process is not reaped yet, so the id
        // cannot belong to anyone else.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Waits up to `grace` for the process to end by itself, then kills it. Always reaps.
    pub(crate) fn shutdown(&mut self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                _ => break,
            }
        }
        self.kill_and_reap();
    }

    pub(crate) fn diagnostic(&self) -> Diagnostic {
        Diagnostic::new(&String::from_utf8_lossy(&self.stderr_text))
    }

    pub(crate) fn stderr_dropped(&self) -> u64 {
        self.stderr_dropped
    }

    fn drain_stderr(&mut self, cap: usize) {
        let mut buf = [0u8; 4096];
        // bounded work per call: a flood cannot keep the host in this loop
        for _ in 0..16 {
            // SAFETY: reading into a local buffer from a descriptor we own.
            let n =
                unsafe { libc::read(self.stderr.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let n = n as usize;
            let room = cap.saturating_sub(self.stderr_text.len());
            self.stderr_text.extend_from_slice(&buf[..n.min(room)]);
            self.stderr_dropped += n.saturating_sub(room) as u64;
        }
    }

    /// Writes `data` to the plugin before `deadline`.
    fn write_all(&mut self, data: &[u8], deadline: Instant, stderr_cap: usize) -> PluginResult<()> {
        let mut written = 0;
        while written < data.len() {
            let wait = poll_two(
                self.stdin.as_raw_fd(),
                libc::POLLOUT,
                self.stderr.as_raw_fd(),
                deadline,
            )?;
            if wait.second_ready {
                self.drain_stderr(stderr_cap);
            }
            if wait.hangup {
                return Err(PluginError::Crashed);
            }
            if !wait.first_ready {
                continue;
            }
            // SAFETY: writing from a valid slice to a descriptor we own.
            let n = unsafe {
                libc::write(
                    self.stdin.as_raw_fd(),
                    data[written..].as_ptr().cast(),
                    data.len() - written,
                )
            };
            if n < 0 {
                let error = std::io::Error::last_os_error();
                match error.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => continue,
                    _ => return Err(PluginError::Crashed),
                }
            }
            written += n as usize;
        }
        Ok(())
    }

    /// Reads one line (without the newline) from the plugin before `deadline`, at most
    /// `max_line` bytes. Anything behind the newline is a protocol violation (the plugin may
    /// only answer).
    fn read_line(
        &mut self,
        deadline: Instant,
        max_line: usize,
        stderr_cap: usize,
    ) -> PluginResult<Vec<u8>> {
        let mut line: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let wait = poll_two(
                self.stdout.as_raw_fd(),
                libc::POLLIN,
                self.stderr.as_raw_fd(),
                deadline,
            )?;
            if wait.second_ready {
                self.drain_stderr(stderr_cap);
            }
            if !wait.first_ready && !wait.hangup {
                continue;
            }
            // SAFETY: reading into a local buffer from a descriptor we own.
            let n = unsafe {
                libc::read(
                    self.stdout.as_raw_fd(),
                    chunk.as_mut_ptr().cast(),
                    chunk.len(),
                )
            };
            if n < 0 {
                let error = std::io::Error::last_os_error();
                match error.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => continue,
                    _ => return Err(PluginError::Crashed),
                }
            }
            if n == 0 {
                return Err(PluginError::Crashed);
            }
            let got = &chunk[..n as usize];
            match got.iter().position(|&b| b == b'\n') {
                Some(end) => {
                    if line.len() + end > max_line {
                        return Err(PluginError::Protocol("Die Antwort ist zu groß"));
                    }
                    if end + 1 != got.len() {
                        return Err(PluginError::Protocol("Unaufgeforderte Daten"));
                    }
                    line.extend_from_slice(&got[..end]);
                    return Ok(line);
                }
                None => {
                    if line.len() + got.len() > max_line {
                        return Err(PluginError::Protocol("Die Antwort ist zu groß"));
                    }
                    line.extend_from_slice(got);
                }
            }
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

struct PollResult {
    first_ready: bool,
    second_ready: bool,
    hangup: bool,
}

/// Waits until `first` has `events`, or `second` can be read, or `deadline` passes.
fn poll_two(
    first: RawFd,
    events: i16,
    second: RawFd,
    deadline: Instant,
) -> PluginResult<PollResult> {
    let now = Instant::now();
    if now >= deadline {
        return Err(PluginError::Timeout);
    }
    let millis = (deadline - now).as_millis().min(1000) as libc::c_int;
    let mut fds = [
        libc::pollfd {
            fd: first,
            events,
            revents: 0,
        },
        libc::pollfd {
            fd: second,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: two valid pollfd structs.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(PollResult {
                first_ready: false,
                second_ready: false,
                hangup: false,
            });
        }
        return Err(PluginError::Crashed);
    }
    Ok(PollResult {
        first_ready: fds[0].revents & events != 0,
        second_ready: fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0,
        hangup: fds[0].revents & (libc::POLLERR | libc::POLLNVAL) != 0
            || (events == libc::POLLOUT && fds[0].revents & libc::POLLHUP != 0)
            || (events == libc::POLLIN
                && fds[0].revents & libc::POLLHUP != 0
                && fds[0].revents & libc::POLLIN == 0),
    })
}

fn cstring(text: &str) -> PluginResult<CString> {
    CString::new(text).map_err(|_| PluginError::Spawn)
}

/// A running plugin: the process plus the request counter.
pub struct Session {
    process: Process,
    // dropped after the process: the working folder is deleted once the process is gone
    _workdir: tempfile::TempDir,
    next_id: u64,
    max_line: usize,
    max_request: usize,
    stderr_cap: usize,
    pub(crate) kinds: Vec<Kind>,
    pub(crate) sandboxed: bool,
}

impl Session {
    /// Starts the verified package. `pinned` is what the user confirmed (the package must
    /// match it exactly, else `Locked`); its `unsandboxed` flag is the only way to run
    /// without the sandbox, and that start is logged.
    pub fn start(
        runtime: &Arc<PluginRuntime>,
        package: &VerifiedPackage,
        pinned: &PinnedState,
    ) -> PluginResult<Session> {
        // SAFETY: both calls have no preconditions.
        let (euid, uid) = unsafe { (libc::geteuid(), libc::getuid()) };
        refuse_root(euid, uid)?;
        pinned.check(package)?;
        let config = runtime.config();
        let permit = runtime.acquire()?;

        let sandboxed = !pinned.unsandboxed;
        let plan = if sandboxed {
            if config.simulate_sandbox_unavailable {
                return Err(PluginError::SandboxUnavailable);
            }
            Some(sandbox::prepare(
                std::os::fd::AsFd::as_fd(&package.dir),
                package.exe.as_raw_fd(),
            )?)
        } else {
            runtime.log(&format!(
                "Plugin {} wird ohne Sandbox gestartet (ausdrücklich bestätigt)",
                package.manifest.id
            ));
            None
        };

        let workdir = tempfile::Builder::new()
            .prefix("dualfile-plugin-")
            .tempdir()
            .map_err(|_| PluginError::Spawn)?;
        let work = workdir
            .path()
            .to_str()
            .ok_or(PluginError::Spawn)?
            .to_owned();
        let exe_text = package.dir_path.join(&package.manifest.exec);
        let exe_text = exe_text.to_str().ok_or(PluginError::Spawn)?;

        let argv_c = [cstring(exe_text)?, cstring(PROTOCOL_ARG)?];
        let env_c = [
            cstring("LANG=C.UTF-8")?,
            cstring("PATH=/nonexistent")?,
            cstring(&format!("HOME={work}"))?,
            cstring(&format!("TMPDIR={work}"))?,
        ];
        let null = std::ptr::null();
        let argv: Vec<*const libc::c_char> =
            argv_c.iter().map(|c| c.as_ptr()).chain([null]).collect();
        let envp: Vec<*const libc::c_char> =
            env_c.iter().map(|c| c.as_ptr()).chain([null]).collect();
        let limits = config.limits;
        let mut setup = ChildSetup {
            exe_fd: package.exe.as_raw_fd(),
            exe_check: package.exe_check,
            argv,
            envp,
            limits: [
                (libc::RLIMIT_CPU, limits.cpu_seconds),
                (libc::RLIMIT_AS, limits.address_space),
                (libc::RLIMIT_FSIZE, 0),
                (libc::RLIMIT_NPROC, limits.threads),
                (libc::RLIMIT_NOFILE, limits.open_files),
                (libc::RLIMIT_CORE, 0),
            ],
            // SAFETY: getpid has no preconditions.
            parent: unsafe { libc::getpid() },
            plan,
        };

        let mut command = Command::new(exe_text);
        dualfile_core::appenv::apply_clean_env(&mut command);
        // execveat below uses its own fixed envp; never add inherited values to it.
        command
            .current_dir(workdir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: the closure only makes plain system calls on data prepared above and never
        // allocates; it replaces the process image or returns an error.
        unsafe {
            command.pre_exec(move || {
                let code = setup.run();
                Err(std::io::Error::from_raw_os_error(code))
            });
        }
        // Started from the runtime's long-lived thread (see `PluginRuntime`); the strings the
        // argument and environment pointers point to travel with the command.
        let spawned = runtime.run_on_spawner(move || {
            let result = command.spawn();
            drop((argv_c, env_c));
            result
        })?;
        let mut child = spawned.map_err(|error| match error.raw_os_error() {
            Some(FAIL_SANDBOX) => PluginError::SandboxUnavailable,
            Some(FAIL_CHANGED) => PluginError::Locked,
            _ => PluginError::Spawn,
        })?;
        let stdin = OwnedFd::from(child.stdin.take().ok_or(PluginError::Spawn)?);
        let stdout = OwnedFd::from(child.stdout.take().ok_or(PluginError::Spawn)?);
        let stderr = OwnedFd::from(child.stderr.take().ok_or(PluginError::Spawn)?);
        for fd in [&stdin, &stdout, &stderr] {
            set_nonblocking(fd.as_raw_fd());
        }
        let process = Process {
            child,
            stdin,
            stdout,
            stderr,
            stderr_text: Vec::new(),
            stderr_dropped: 0,
            _permit: permit,
        };
        let mut session = Session {
            process,
            _workdir: workdir,
            next_id: 1,
            max_line: config.max_line_bytes,
            max_request: config.max_request_bytes,
            stderr_cap: config.stderr_cap,
            kinds: Vec::new(),
            sandboxed,
        };
        session.hello(config.hello_timeout, &package.manifest.kinds)?;
        Ok(session)
    }

    fn hello(&mut self, timeout: Duration, allowed: &[Kind]) -> PluginResult<()> {
        let mut params = Map::new();
        params.insert("api".into(), Value::from(crate::manifest::API_VERSION));
        let result = self.call("hello", params, timeout)?;
        let hello = crate::protocol::parse_hello(&result).inspect_err(|_| self.kill())?;
        if hello.api != crate::manifest::API_VERSION {
            self.kill();
            return Err(PluginError::Protocol("Die Protokollversion weicht ab"));
        }
        if hello.kinds.is_empty() || hello.kinds.iter().any(|kind| !allowed.contains(kind)) {
            self.kill();
            return Err(PluginError::Protocol(
                "Das Plugin meldet Arten, die das Manifest nicht nennt",
            ));
        }
        self.kinds = hello.kinds;
        Ok(())
    }

    pub fn pid(&self) -> u32 {
        self.process.pid()
    }

    pub fn is_alive(&mut self) -> bool {
        self.process.is_alive()
    }

    /// True if the session runs inside the sandbox.
    pub fn is_sandboxed(&self) -> bool {
        self.sandboxed
    }

    /// The cleaned text the plugin wrote to standard error so far (capped).
    pub fn diagnostic(&self) -> Diagnostic {
        self.process.diagnostic()
    }

    /// Bytes of standard error that were read but dropped because of the cap.
    pub fn stderr_dropped(&self) -> u64 {
        self.process.stderr_dropped()
    }

    pub fn kill(&mut self) {
        self.process.kill_and_reap();
    }

    /// Ends the plugin: closes its input, gives it a moment, then kills and reaps it.
    pub fn close(mut self) {
        self.process.shutdown(Duration::from_millis(200));
    }

    /// Sends one request and returns the validated `result` object of the answer. Any
    /// transport or protocol failure kills the process; an error answer of the plugin is
    /// returned as `PluginError::Reported` and keeps the session.
    #[doc(hidden)]
    pub fn call(
        &mut self,
        op: &str,
        params: Map<String, Value>,
        timeout: Duration,
    ) -> PluginResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = Map::new();
        request.insert("id".into(), Value::from(id));
        request.insert("op".into(), Value::from(op));
        for (key, value) in params {
            request.insert(key, value);
        }
        let mut line = serde_json::to_vec(&Value::Object(request))
            .map_err(|_| PluginError::Protocol("Anfrage nicht darstellbar"))?;
        if line.len() > self.max_request {
            return Err(PluginError::Unsupported(
                "Die Anfrage ist zu groß für das Plugin",
            ));
        }
        line.push(b'\n');
        let deadline = Instant::now() + timeout;
        let outcome = self
            .process
            .write_all(&line, deadline, self.stderr_cap)
            .and_then(|()| {
                self.process
                    .read_line(deadline, self.max_line, self.stderr_cap)
            })
            .and_then(|answer| crate::protocol::parse_response(&answer, id));
        match outcome {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(reported)) => Err(reported),
            Err(error) => {
                self.process.kill_and_reap();
                Err(error)
            }
        }
    }
}
