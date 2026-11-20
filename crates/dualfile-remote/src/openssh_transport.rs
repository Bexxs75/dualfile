//! System OpenSSH SFTP transport (Unix pipes). Uses system ssh configuration and
//! known_hosts, not the application's pin store. Optional askpass delegates to a handler.
//! Calls block a non-async caller, like RusshTransport; quit/Drop cancel the process.

use crate::askpass::{AskpassServer, PromptHandler};
use crate::error::{SftpStatus, TResult, TransportError};
use crate::profile::{SftpAuth, SftpConnectParams};
use crate::secret::Secret;
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::trust::CertVerifier;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{Config as SftpConfig, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe::{Receiver, Sender};

const POSIX_RENAME: &str = "posix-rename@openssh.com";
const MAX_READ: usize = 256 * 1024;
const WRITE_PIECE: usize = 32 * 1024;
const STDERR_LIMIT: usize = 8192;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("dualfile-openssh")
            .enable_time()
            .enable_io()
            .build()
            .expect("the SFTP runtime starts")
    })
}

fn map_sftp_error(error: SftpError) -> TransportError {
    match error {
        SftpError::Status(status) => {
            let code = match status.status_code {
                StatusCode::NoSuchFile => SftpStatus::NoSuchFile,
                StatusCode::PermissionDenied => SftpStatus::PermissionDenied,
                StatusCode::BadMessage => SftpStatus::BadMessage,
                StatusCode::NoConnection => SftpStatus::NoConnection,
                StatusCode::ConnectionLost => SftpStatus::ConnectionLost,
                StatusCode::OpUnsupported => SftpStatus::OpUnsupported,
                StatusCode::Failure | StatusCode::Eof | StatusCode::Ok => SftpStatus::Failure,
            };
            TransportError::status(code, "")
        }
        SftpError::Timeout => TransportError::Timeout,
        SftpError::Limited(_) => TransportError::Protocol("SFTP-Antwort zu groß".into()),
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => {
            TransportError::Connection("SFTP-Verbindung getrennt".into())
        }
        SftpError::UnexpectedPacket => TransportError::Protocol("Unerwartete Antwort".into()),
    }
}

fn is_eof(error: &SftpError) -> bool {
    matches!(error, SftpError::Status(status) if status.status_code == StatusCode::Eof)
}

fn to_time(secs: Option<u32>) -> Option<SystemTime> {
    secs.map(|s| UNIX_EPOCH + Duration::from_secs(u64::from(s)))
}

fn to_secs(time: SystemTime) -> u32 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().min(u64::from(u32::MAX)) as u32)
        .unwrap_or(0)
}

fn raw_attrs(attrs: &FileAttributes) -> RawAttrs {
    let kind = match attrs.permissions.map(|p| p & 0o170_000) {
        Some(0o040_000) => RawKind::Dir,
        Some(0o100_000) => RawKind::File,
        Some(0o120_000) => RawKind::Symlink,
        Some(0) | None => RawKind::Unknown,
        Some(_) => RawKind::Other,
    };
    RawAttrs {
        uid: attrs.uid,
        gid: attrs.gid,
        kind,
        size: attrs.size,
        mtime: to_time(attrs.mtime),
        atime: to_time(attrs.atime),
        permissions: attrs.permissions.map(|p| p & 0o7777),
    }
}

fn check_text(text: &str) -> TResult<()> {
    if text.chars().any(char::is_control) {
        Err(TransportError::Rejected)
    } else {
        Ok(())
    }
}

use dualfile_core::connections::valid_ssh_endpoint as valid_endpoint;

pub(crate) fn allowed_env(key: &std::ffi::OsStr) -> bool {
    key.to_str().is_some_and(|s| {
        matches!(s, "PATH" | "HOME" | "USER" | "LANG" | "SSH_AUTH_SOCK") || s.starts_with("LC_")
    })
}

// Keep the SSH allowlist and remove AppImage paths before installing askpass values.
pub(crate) fn configure_external_env(command: &mut Command) {
    dualfile_core::appenv::apply_external_current_dir(command);
    let vars: Vec<_> = std::env::vars_os()
        .filter(|(k, _)| allowed_env(k))
        .collect();
    let appdir = std::env::var_os("APPDIR");
    let plan = dualfile_core::appenv::clean_env(vars.clone(), appdir.as_deref());
    command.env_clear().envs(vars);
    plan.apply(command);
}

pub(crate) fn find_ssh_in(path: &std::ffi::OsStr) -> TResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|dir| dir.join("ssh"))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .ok_or(TransportError::OpensshMissing)
}

fn command(
    ssh: &Path,
    params: &SftpConnectParams,
    askpass: Option<&AskpassServer>,
) -> TResult<Command> {
    let p = &params.profile;
    if !valid_endpoint(&p.host)
        || (!p.user.is_empty() && !valid_endpoint(&p.user))
        || (p.transport == crate::profile::SshTransport::Builtin
            && (p.user.is_empty() || p.port == 0))
    {
        return Err(TransportError::Rejected);
    }
    if let SftpAuth::KeyFile { path } = &p.auth {
        if !Path::new(path).is_absolute()
            || !Path::new(path).is_file()
            || path.chars().any(char::is_control)
        {
            return Err(TransportError::Rejected);
        }
    }
    let mut c = Command::new(ssh);
    c.args([
        "-T",
        "-s",
        "-o",
        if askpass.is_some() {
            "NumberOfPasswordPrompts=3"
        } else {
            "BatchMode=yes"
        },
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]);
    if p.port != 0 {
        c.arg("-p").arg(p.port.to_string());
    }
    if !p.user.is_empty() {
        c.arg("-l").arg(&p.user);
    }
    if let SftpAuth::KeyFile { path } = &p.auth {
        c.arg("-i").arg(path).args(["-o", "IdentitiesOnly=yes"]);
    }
    c.arg("--").arg(&p.host).arg("sftp");
    configure_external_env(&mut c);
    if let Some(server) = askpass {
        server.configure(&mut c).map_err(|_| {
            TransportError::Connection("Askpass konnte nicht gestartet werden".into())
        })?;
    }
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(c)
}

// Only recognized patterns become fixed messages. No raw stderr, paths or credentials
// reach Display/Debug, even if an arbitrary ssh config helper echoes them.
fn classify_stderr(bytes: &[u8]) -> Option<TransportError> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if text.contains("host key verification failed") {
        Some(TransportError::HostKeyRejected)
    } else if text.contains("permission denied") {
        Some(TransportError::AuthFailed)
    } else if [
        "could not resolve hostname",
        "connection timed out",
        "connection refused",
    ]
    .iter()
    .any(|p| text.contains(p))
    {
        Some(TransportError::Unreachable)
    } else if text.contains("subsystem request failed") {
        Some(TransportError::SubsystemMissing)
    } else {
        None
    }
}

#[derive(Default)]
struct StderrRing(VecDeque<u8>);
impl StderrRing {
    fn append(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.0.len() == STDERR_LIMIT {
                self.0.pop_front();
            }
            self.0.push_back(b);
        }
    }
    fn error(&self) -> Option<TransportError> {
        classify_stderr(&self.0.iter().copied().collect::<Vec<_>>())
    }
}

// Installed immediately after spawn so every early return and unwinding path reaps ssh.
struct Process {
    child: Child,
    stderr: Arc<Mutex<StderrRing>>,
    reader: Option<tokio::task::JoinHandle<()>>,
    stopped: bool,
    askpass: Option<AskpassServer>,
}
impl Process {
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stopped = true;
        self.askpass.take();
        if let Some(mut task) = self.reader.take() {
            // A config helper could inherit stderr. Do not wait indefinitely for its EOF.
            runtime().block_on(async {
                if tokio::time::timeout(Duration::from_millis(100), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            });
        }
    }
    fn failure(&mut self, fallback: TransportError) -> TransportError {
        self.stop();
        self.stderr
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .error()
            .unwrap_or(fallback)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

fn call_on<R>(
    process: &mut Process,
    idle: Duration,
    request: impl Future<Output = Result<R, SftpError>>,
) -> TResult<R> {
    if process.stopped {
        return Err(TransportError::Connection("Verbindung getrennt".into()));
    }
    let result = runtime()
        .block_on(async {
            tokio::time::timeout(idle, request)
                .await
                .unwrap_or(Err(SftpError::Timeout))
        })
        .map_err(map_sftp_error);
    match result {
        Err(e) if e.is_connection_lost() => Err(process.failure(e)),
        other => other,
    }
}
macro_rules! call {
    ($self:ident, $request:expr) => {
        call_on(&mut $self.process, $self.idle, $request)
    };
}
macro_rules! call_eof {
    ($self:ident, $request:expr) => {
        call_on(&mut $self.process, $self.idle, async {
            match $request.await {
                Ok(v) => Ok(Some(v)),
                Err(e) if is_eof(&e) => Ok(None),
                Err(e) => Err(e),
            }
        })
    };
}

pub struct OpensshTransport {
    process: Process,
    sftp: RawSftpSession,
    features: SftpFeatures,
    handles: HashMap<u64, String>,
    next_handle: u64,
    idle: Duration,
}
impl std::fmt::Debug for OpensshTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpensshTransport").finish_non_exhaustive()
    }
}
impl OpensshTransport {
    /// Uses PATH discovery when `ssh` is None. An explicit program is trusted executable
    /// code, intended for embedding/tests; it is never read from a connection profile.
    pub fn connect(params: &SftpConnectParams, ssh: Option<&Path>) -> TResult<Self> {
        Self::connect_with_handler(params, ssh, None)
    }
    pub fn connect_with_handler(
        params: &SftpConnectParams,
        ssh: Option<&Path>,
        handler: Option<Arc<dyn PromptHandler>>,
    ) -> TResult<Self> {
        let askpass = handler.map(AskpassServer::start).transpose().map_err(|_| {
            TransportError::Connection("Askpass konnte nicht gestartet werden".into())
        })?;
        let discovered;
        let ssh = match ssh {
            Some(p) => p,
            None => {
                discovered = find_ssh_in(&std::env::var_os("PATH").unwrap_or_default())?;
                &discovered
            }
        };
        let child = command(ssh, params, askpass.as_ref())?
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    TransportError::OpensshMissing
                } else {
                    TransportError::Connection("OpenSSH konnte nicht gestartet werden".into())
                }
            })?;
        let mut process = Process {
            child,
            stderr: Arc::new(Mutex::new(StderrRing::default())),
            reader: None,
            stopped: false,
            askpass,
        };
        let io_error = |_| TransportError::Connection("OpenSSH-Pipe nicht verfügbar".into());
        let (input, output, mut stderr) = {
            let _enter = runtime().enter();
            (
                Sender::from_owned_fd(process.child.stdin.take().unwrap().into())
                    .map_err(io_error)?,
                Receiver::from_owned_fd(process.child.stdout.take().unwrap().into())
                    .map_err(io_error)?,
                Receiver::from_owned_fd(process.child.stderr.take().unwrap().into())
                    .map_err(io_error)?,
            )
        };
        let ring = process.stderr.clone();
        process.reader = Some(runtime().spawn(async move {
            let mut buf = [0; 1024];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                ring.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .append(&buf[..n]);
            }
        }));
        let idle = params.timeouts.idle;
        let sftp = {
            let _enter = runtime().enter();
            RawSftpSession::new_with_config(
                tokio::io::join(output, input),
                SftpConfig {
                    request_timeout_secs: idle.as_secs().max(1),
                    ..SftpConfig::default()
                },
            )
        };
        let connect_timeout = if process.askpass.is_some() {
            params
                .timeouts
                .connect
                .saturating_add(Duration::from_secs(8 * 120))
        } else {
            params.timeouts.connect
        };
        // The SFTP library also times out INIT internally; allow human interaction there,
        // then restore the usual idle budget for all file operations.
        if process.askpass.is_some() {
            sftp.set_timeout(connect_timeout.as_secs().saturating_add(1));
        }
        let version = call_on(&mut process, connect_timeout, sftp.init())?;
        sftp.set_timeout(idle.as_secs().max(1));
        process.askpass.take();
        if version.version < 3 {
            return Err(TransportError::Protocol(
                "Der Server spricht keine passende SFTP-Version".into(),
            ));
        }
        let features = SftpFeatures {
            posix_rename: version
                .extensions
                .get(POSIX_RENAME)
                .is_some_and(|v| v == "1"),
        };
        Ok(Self {
            process,
            sftp,
            features,
            handles: HashMap::new(),
            next_handle: 1,
            idle,
        })
    }
    fn handle_text(&self, handle: SftpHandle) -> TResult<String> {
        self.handles
            .get(&handle.0)
            .cloned()
            .ok_or_else(|| TransportError::status(SftpStatus::Failure, "Ungültiges Handle"))
    }

    fn register(&mut self, text: String) -> SftpHandle {
        let id = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(id, text);
        SftpHandle(id)
    }
}

impl SftpTransport for OpensshTransport {
    fn features(&self) -> SftpFeatures {
        self.features
    }

    fn realpath(&mut self, path: &str) -> TResult<String> {
        check_text(path)?;
        let name = call!(self, self.sftp.realpath(path))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| TransportError::Protocol("Leere Antwort".into()))
    }

    fn lstat(&mut self, path: &str) -> TResult<RawAttrs> {
        check_text(path)?;
        let attrs = call!(self, self.sftp.lstat(path))?;
        Ok(raw_attrs(&attrs.attrs))
    }

    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle> {
        check_text(path)?;
        let handle = call!(self, self.sftp.opendir(path))?;
        Ok(self.register(handle.handle))
    }

    fn read_dir(&mut self, handle: SftpHandle) -> TResult<Vec<RawEntry>> {
        let text = self.handle_text(handle)?;
        match call_eof!(self, self.sftp.readdir(text))? {
            None => Ok(Vec::new()),
            Some(name) => Ok(name
                .files
                .into_iter()
                .map(|file| RawEntry {
                    name: file.filename,
                    attrs: raw_attrs(&file.attrs),
                })
                .collect()),
        }
    }

    fn open_read(&mut self, path: &str) -> TResult<SftpHandle> {
        check_text(path)?;
        let handle = call!(
            self,
            self.sftp
                .open(path, OpenFlags::READ, FileAttributes::empty(),)
        )?;
        Ok(self.register(handle.handle))
    }

    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle> {
        check_text(path)?;
        let (flags, attrs) = match how {
            OpenWrite::CreateNew => (
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o644),
                    ..FileAttributes::empty()
                },
            ),
            OpenWrite::CreateTruncate => (
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                FileAttributes {
                    permissions: Some(0o644),
                    ..FileAttributes::empty()
                },
            ),
            OpenWrite::Existing => (OpenFlags::WRITE, FileAttributes::empty()),
        };
        let handle = call!(self, self.sftp.open(path, flags, attrs))?;
        Ok(self.register(handle.handle))
    }

    fn read(&mut self, handle: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize> {
        let text = self.handle_text(handle)?;
        let want = buf.len().min(MAX_READ) as u32;
        match call_eof!(self, self.sftp.read(text, offset, want))? {
            None => Ok(0),
            Some(data) => {
                let n = data.data.len().min(buf.len());
                buf[..n].copy_from_slice(&data.data[..n]);
                Ok(n)
            }
        }
    }

    fn write(&mut self, handle: SftpHandle, offset: u64, data: &[u8]) -> TResult<()> {
        let text = self.handle_text(handle)?;
        let mut at = offset;
        for piece in data.chunks(WRITE_PIECE) {
            call!(self, self.sftp.write(text.clone(), at, piece.to_vec()))?;
            at += piece.len() as u64;
        }
        Ok(())
    }

    fn fstat(&mut self, handle: SftpHandle) -> TResult<RawAttrs> {
        let text = self.handle_text(handle)?;
        let attrs = call!(self, self.sftp.fstat(text))?;
        Ok(raw_attrs(&attrs.attrs))
    }

    fn close(&mut self, handle: SftpHandle) -> TResult<()> {
        let text = self
            .handles
            .remove(&handle.0)
            .ok_or_else(|| TransportError::status(SftpStatus::Failure, "Ungültiges Handle"))?;
        call!(self, self.sftp.close(text)).map(|_| ())
    }

    fn mkdir(&mut self, path: &str) -> TResult<()> {
        check_text(path)?;
        call!(self, self.sftp.mkdir(path, FileAttributes::empty())).map(|_| ())
    }

    fn rmdir(&mut self, path: &str) -> TResult<()> {
        check_text(path)?;
        call!(self, self.sftp.rmdir(path)).map(|_| ())
    }

    fn remove(&mut self, path: &str) -> TResult<()> {
        check_text(path)?;
        call!(self, self.sftp.remove(path)).map(|_| ())
    }

    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        check_text(from)?;
        check_text(to)?;
        call!(self, self.sftp.rename(from, to)).map(|_| ())
    }

    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()> {
        check_text(from)?;
        check_text(to)?;
        if !self.features.posix_rename {
            return Err(TransportError::Unsupported("posix-rename"));
        }
        // the extension's data: two SSH strings (u32 length + bytes)
        let mut data = Vec::with_capacity(8 + from.len() + to.len());
        for text in [from, to] {
            data.extend_from_slice(&(text.len() as u32).to_be_bytes());
            data.extend_from_slice(text.as_bytes());
        }
        let reply = call!(self, self.sftp.extended(POSIX_RENAME, data))?;
        match reply {
            russh_sftp::protocol::Packet::Status(status)
                if status.status_code == StatusCode::Ok =>
            {
                Ok(())
            }
            russh_sftp::protocol::Packet::Status(status) => {
                Err(map_sftp_error(SftpError::Status(status)))
            }
            _ => Err(TransportError::Protocol("Unerwartete Antwort".into())),
        }
    }

    fn readlink(&mut self, path: &str) -> TResult<String> {
        check_text(path)?;
        let name = call!(self, self.sftp.readlink(path))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| TransportError::Protocol("Leere Antwort".into()))
    }

    fn symlink(&mut self, target: &str, at: &str) -> TResult<()> {
        check_text(target)?;
        check_text(at)?;
        // OpenSSH's server takes the link's content first and its location second (the
        // reverse of the protocol text); the library sends its first argument first
        call!(self, self.sftp.symlink(target, at)).map(|_| ())
    }

    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        atime: Option<SystemTime>,
    ) -> TResult<()> {
        check_text(path)?;
        let attrs = FileAttributes {
            permissions,
            mtime: mtime.map(to_secs),
            atime: atime.or(mtime).map(to_secs),
            ..FileAttributes::empty()
        };
        call!(self, self.sftp.setstat(path, attrs)).map(|_| ())
    }

    fn quit(&mut self) {
        self.process.stop();
        let _ = self.sftp.close_session();
    }
}
impl Drop for OpensshTransport {
    fn drop(&mut self) {
        self.quit();
    }
}

/// OpenSSH owns known_hosts. The trait's stored secret and pin verifier are unused;
/// interactive authentication goes exclusively through the optional prompt handler.
#[derive(Default)]
pub struct OpensshConnector {
    ssh: Option<PathBuf>,
    handler: Option<Arc<dyn PromptHandler>>,
}
impl OpensshConnector {
    pub fn with_prompt_handler(mut self, handler: Arc<dyn PromptHandler>) -> Self {
        self.handler = Some(handler);
        self
    }
    pub fn with_program(ssh: impl Into<PathBuf>) -> Self {
        Self {
            ssh: Some(ssh.into()),
            handler: None,
        }
    }
}
impl crate::manager::SftpConnector for OpensshConnector {
    type Transport = OpensshTransport;
    fn connect(
        &self,
        params: &SftpConnectParams,
        _secret: &Secret,
        _verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport> {
        OpensshTransport::connect_with_handler(params, self.ssh.as_deref(), self.handler.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SftpProfile;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn inherited_user_port_key_are_not_passed() {
        let p = SftpConnectParams::new(crate::profile::SftpProfile {
            transport: crate::profile::SshTransport::Openssh,
            name: "alias".into(),
            host: "nas".into(),
            user: String::new(),
            port: 0,
            auth: SftpAuth::Password,
            start_folder: None,
        });
        let command = command(Path::new("ssh"), &p, None).unwrap();
        let args: Vec<_> = command.get_args().map(|v| v.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "-T",
                "-s",
                "-o",
                "BatchMode=yes",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                "nas",
                "sftp"
            ]
        );
    }

    struct CancelPrompt;
    impl PromptHandler for CancelPrompt {
        fn prompt(&self, _: crate::askpass::Prompt) -> crate::askpass::PromptAnswer {
            crate::askpass::PromptAnswer::Cancel
        }
    }
    #[test]
    fn askpass_command_has_only_explicit_extra_environment() {
        let server = AskpassServer::start(Arc::new(CancelPrompt)).unwrap();
        let c = command(Path::new("ssh"), &params(), Some(&server)).unwrap();
        let args: Vec<_> = c.get_args().collect();
        assert!(!args.contains(&std::ffi::OsStr::new("BatchMode=yes")));
        assert!(args.contains(&std::ffi::OsStr::new("NumberOfPasswordPrompts=3")));
        assert!(!args
            .iter()
            .any(|a| a.to_string_lossy().contains("StrictHostKeyChecking")));
        let env: HashMap<_, _> = c.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("SSH_ASKPASS_REQUIRE")),
            Some(&Some(std::ffi::OsStr::new("force")))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("SSH_ASKPASS")),
            Some(&Some(std::env::current_exe().unwrap().as_os_str()))
        );
        for key in env.keys() {
            assert!(
                allowed_env(key)
                    || [
                        "SSH_ASKPASS",
                        "SSH_ASKPASS_REQUIRE",
                        "DUALFILE_ASKPASS_SOCKET",
                        "DUALFILE_ASKPASS_TOKEN"
                    ]
                    .iter()
                    .any(|s| key == &std::ffi::OsStr::new(s))
            );
        }
        assert!(!env.contains_key(std::ffi::OsStr::new("DISPLAY")));
        let batch = command(Path::new("ssh"), &params(), None).unwrap();
        assert!(batch.get_args().any(|a| a == "BatchMode=yes"));
        assert!(!batch.get_envs().any(|(k, _)| k == "SSH_ASKPASS"));
    }
    fn params() -> SftpConnectParams {
        let mut p = SftpConnectParams::new(SftpProfile {
            transport: crate::profile::SshTransport::Builtin,
            name: "test".into(),
            host: "example.invalid".into(),
            port: 2222,
            user: "test-user".into(),
            auth: SftpAuth::Password,
            start_folder: None,
        });
        p.timeouts.connect = Duration::from_secs(2);
        p.timeouts.idle = Duration::from_secs(2);
        p
    }
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("ssh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn quoted(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    fn server() -> Option<PathBuf> {
        [
            "/usr/lib/openssh/sftp-server",
            "/usr/lib/ssh/sftp-server",
            "/usr/libexec/sftp-server",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    }
    fn server_script(dir: &Path, prefix: &str) -> PathBuf {
        let server = server().expect("install OpenSSH sftp-server to run this test");
        script(dir, &format!("{prefix}\nexec {}", quoted(&server)))
    }

    #[test]
    fn rejects_injection_invalid_endpoints_ports_and_keys() {
        for bad in [
            "-oProxyCommand=x",
            "",
            "host name",
            "a\nb",
            "a/b",
            "x;id",
            "ü",
            &"a".repeat(256),
        ] {
            for user in [false, true] {
                let mut p = params();
                if user {
                    p.profile.user = bad.into();
                } else {
                    p.profile.host = bad.into();
                }
                assert!(matches!(
                    command(Path::new("unused"), &p, None),
                    Err(TransportError::Rejected)
                ));
            }
        }
        for good in [
            "[::1]",
            "host.example",
            "user@host",
            "fe80::1%eth0",
            &"a".repeat(255),
        ] {
            assert!(valid_endpoint(good));
        }
        let mut p = params();
        p.profile.port = 0;
        assert!(command(Path::new("unused"), &p, None).is_err());
        p.profile.port = u16::MAX;
        assert!(command(Path::new("unused"), &p, None).is_ok());
        let dir = tempfile::tempdir().unwrap();
        for key in [
            "relative".to_owned(),
            dir.path().join("missing").to_str().unwrap().into(),
            dir.path().to_str().unwrap().into(),
        ] {
            p.profile.auth = SftpAuth::KeyFile { path: key };
            assert!(matches!(
                command(Path::new("unused"), &p, None),
                Err(TransportError::Rejected)
            ));
        }
    }

    #[test]
    fn path_discovery_requires_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_ssh_in(dir.path().as_os_str()),
            Err(TransportError::OpensshMissing)
        ));
        let path = dir.path().join("ssh");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(find_ssh_in(dir.path().as_os_str()).is_err());
        let path = script(dir.path(), "exit 0");
        assert_eq!(find_ssh_in(dir.path().as_os_str()).unwrap(), path);
        assert!(matches!(
            OpensshTransport::connect(&params(), Some(&dir.path().join("missing"))),
            Err(TransportError::OpensshMissing)
        ));
    }

    #[test]
    fn fake_ssh_receives_exact_arguments_and_clean_environment() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let env = dir.path().join("env");
        let ssh = script(dir.path(), &format!("printf '%s\\n' \"$@\" > {}\n/usr/bin/env > {}\nprintf 'Permission denied\\n' >&2\nexit 1", quoted(&args), quoted(&env)));
        let mut p = params();
        let key = dir.path().join("private key");
        std::fs::write(&key, "test-only").unwrap();
        for with_key in [false, true] {
            p.profile.auth = if with_key {
                SftpAuth::KeyFile {
                    path: key.to_str().unwrap().into(),
                }
            } else {
                SftpAuth::Password
            };
            // Inspect explicit overrides as well as the actual child's environment.
            let c = command(&ssh, &p, None).unwrap();
            if std::env::var_os("APPDIR").is_some() {
                assert_eq!(c.get_current_dir(), Some(Path::new("/")));
            }
            for (k, _) in c.get_envs() {
                assert!(allowed_env(k));
            }
            let e = OpensshTransport::connect(&p, Some(&ssh)).unwrap_err();
            assert!(matches!(e, TransportError::AuthFailed), "{e:?}");
            let mut expected = vec![
                "-T",
                "-s",
                "-o",
                "BatchMode=yes",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-p",
                "2222",
                "-l",
                "test-user",
            ];
            if with_key {
                expected.extend(["-i", key.to_str().unwrap(), "-o", "IdentitiesOnly=yes"]);
            }
            expected.extend(["--", "example.invalid", "sftp"]);
            assert_eq!(
                std::fs::read_to_string(&args).unwrap(),
                format!("{}\n", expected.join("\n"))
            );
            for line in std::fs::read_to_string(&env).unwrap().lines() {
                let name = line.split_once('=').unwrap().0;
                // /bin/sh itself adds PWD after exec; it was not passed by the transport.
                assert!(allowed_env(name.as_ref()) || name == "PWD", "{name}");
            }
        }
        for name in [
            "SSH_ASKPASS",
            "SSH_ASKPASS_REQUIRE",
            "DISPLAY",
            "LD_PRELOAD",
            "BASH_ENV",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(!allowed_env(name.as_ref()));
        }
    }

    #[test]
    fn inherited_environment_is_actually_removed() {
        // Isolate environment mutation from parallel tests; the nested test checks
        // every variable received by fake ssh, so even one leaked marker fails it.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "openssh_transport::tests::fake_ssh_receives_exact_arguments_and_clean_environment",
            ])
            .env("APPDIR", "/tmp/.mount_x")
            .env("HOME", "/")
            .env("DUALFILE_FORBIDDEN_SECRET", "secret-marker")
            .env("SSH_ASKPASS", "/nonexistent/askpass-marker")
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", ":12345")
            .env("SSH_AUTH_SOCK", "/nonexistent/agent-marker")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn stderr_patterns_are_classified_without_echoing_secrets() {
        let dir = tempfile::tempdir().unwrap();
        for (pattern, expected) in [
            (
                "Host key verification failed",
                TransportError::HostKeyRejected,
            ),
            ("Permission denied", TransportError::AuthFailed),
            ("Could not resolve hostname", TransportError::Unreachable),
            ("Connection timed out", TransportError::Unreachable),
            ("Connection refused", TransportError::Unreachable),
            ("subsystem request failed", TransportError::SubsystemMissing),
        ] {
            let ssh = script(
                dir.path(),
                &format!(
                    "printf '{pattern}: /private/SECRET password-MARKER\\033\\r\\n' >&2\nexit 1"
                ),
            );
            let error = OpensshTransport::connect(&params(), Some(&ssh)).unwrap_err();
            assert_eq!(
                std::mem::discriminant(&error),
                std::mem::discriminant(&expected)
            );
            let text = format!("{error} {error:?}");
            assert!(!text.contains("SECRET") && !text.contains("MARKER") && !text.contains('/'));
            assert!(text.len() < 200 && !text.chars().any(char::is_control));
        }
        assert!(TransportError::AuthFailed
            .to_string()
            .contains("Schlüssel oder ssh-agent nötig"));
        let ssh = script(
            dir.path(),
            "printf '/private/SECRET password-MARKER' >&2\nexit 1",
        );
        let error = OpensshTransport::connect(&params(), Some(&ssh)).unwrap_err();
        assert!(!format!("{error:?}").contains("MARKER"));
    }

    #[test]
    fn stderr_ring_is_bounded_and_keeps_tail() {
        let mut ring = StderrRing::default();
        ring.append(&vec![b'x'; 100_000]);
        ring.append(b"Permission denied");
        assert_eq!(ring.0.len(), STDERR_LIMIT);
        assert!(matches!(ring.error(), Some(TransportError::AuthFailed)));
    }

    #[cfg(target_os = "linux")]
    fn assert_reaped(pid: u32) {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "process {pid} still alive or zombie"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn connect_timeout_kills_and_reaps_child() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let ssh = script(
            dir.path(),
            &format!("echo $$ > {}\nexec /bin/sleep 30", quoted(&pidfile)),
        );
        let mut p = params();
        p.timeouts.connect = Duration::from_millis(150);
        let error = OpensshTransport::connect(&p, Some(&ssh)).unwrap_err();
        assert!(matches!(error, TransportError::Timeout));
        let pid = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_reaped(pid);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn guard_reaps_on_unwind() {
        // Test the guard independently of script creation/exec and its ETXTBSY race.
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let outcome = std::panic::catch_unwind(|| {
            let _guard = Process {
                child,
                stderr: Arc::new(Mutex::new(StderrRing::default())),
                reader: None,
                stopped: false,
                askpass: None,
            };
            panic!("test cancellation");
        });
        assert!(outcome.is_err());
        // waitpid identifies our child; /proc could already belong to a reused PID.
        assert_eq!(
            rustix::process::waitpid(
                rustix::process::Pid::from_raw(pid as i32),
                rustix::process::WaitOptions::NOHANG
            )
            .unwrap_err(),
            rustix::io::Errno::CHILD,
        );
    }

    // Uses only the local OpenSSH subsystem executable, never sshd or a socket.
    #[test]
    fn local_server_round_trip_all_operations() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = server_script(dir.path(), "");
        let mut t = OpensshTransport::connect(&params(), Some(&ssh)).unwrap();
        let root = dir.path().join("files").to_str().unwrap().to_owned();
        t.mkdir(&root).unwrap();
        assert_eq!(t.realpath(&root).unwrap(), root);
        assert_eq!(t.lstat(&root).unwrap().kind, RawKind::Dir);
        let file = format!("{root}/file");
        let h = t.open_write(&file, OpenWrite::CreateNew).unwrap();
        t.write(h, 0, b"abcdef").unwrap();
        t.write(h, 3, b"XYZ").unwrap();
        assert_eq!(t.fstat(h).unwrap().size, Some(6));
        t.close(h).unwrap();
        assert!(t.open_write(&file, OpenWrite::CreateNew).is_err());
        let h = t.open_read(&file).unwrap();
        let mut buf = [0; 10];
        assert_eq!(t.read(h, 2, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"cXYZ");
        assert_eq!(t.read(h, 6, &mut buf).unwrap(), 0);
        t.close(h).unwrap();
        let h = t.open_write(&file, OpenWrite::Existing).unwrap();
        t.write(h, 6, b"!").unwrap();
        t.close(h).unwrap();
        let when = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        t.setstat(&file, Some(0o600), Some(when), Some(when))
            .unwrap();
        let attrs = t.lstat(&file).unwrap();
        assert_eq!(attrs.permissions, Some(0o600));
        assert_eq!(attrs.mtime, Some(when));
        let link = format!("{root}/link");
        t.symlink("file", &link).unwrap();
        assert_eq!(t.lstat(&link).unwrap().kind, RawKind::Symlink);
        assert_eq!(t.readlink(&link).unwrap(), "file");
        let h = t.open_dir(&root).unwrap();
        let mut names = Vec::new();
        loop {
            let entries = t.read_dir(h).unwrap();
            if entries.is_empty() {
                break;
            }
            names.extend(entries.into_iter().map(|e| e.name));
        }
        t.close(h).unwrap();
        assert!(names.iter().any(|n| n == "file"));
        let renamed = format!("{root}/renamed");
        t.rename(&file, &renamed).unwrap();
        let h = t.open_write(&file, OpenWrite::CreateTruncate).unwrap();
        t.write(h, 0, b"replacement").unwrap();
        t.close(h).unwrap();
        assert!(t.rename(&file, &renamed).is_err());
        assert!(t.features().posix_rename);
        t.posix_rename(&file, &renamed).unwrap();
        assert_eq!(t.lstat(&renamed).unwrap().size, Some(11));
        assert!(matches!(
            t.lstat(&file),
            Err(TransportError::Status {
                code: SftpStatus::NoSuchFile,
                ..
            })
        ));
        assert!(matches!(
            t.realpath("/bad\npath"),
            Err(TransportError::Rejected)
        ));
        t.remove(&link).unwrap();
        t.remove(&renamed).unwrap();
        t.rmdir(&root).unwrap();
        let pid = t.process.child.id();
        drop(t);
        #[cfg(target_os = "linux")]
        assert_reaped(pid);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn quit_and_request_timeout_reap_connected_child() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = server_script(dir.path(), "");
        let mut t = OpensshTransport::connect(&params(), Some(&ssh)).unwrap();
        let pid = t.process.child.id();
        t.quit();
        t.quit();
        assert_reaped(pid);
        assert!(t.realpath("/").is_err());

        // A script sends only the binary SFTP v3 greeting, then never answers requests.
        let ssh = script(
            dir.path(),
            "printf '\\000\\000\\000\\005\\002\\000\\000\\000\\003'\nexec /bin/sleep 30",
        );
        let mut p = params();
        p.timeouts.idle = Duration::from_millis(100);
        let mut t = OpensshTransport::connect(&p, Some(&ssh)).unwrap();
        let pid = t.process.child.id();
        assert!(matches!(t.realpath("/"), Err(TransportError::Timeout)));
        assert_reaped(pid);
    }
}
