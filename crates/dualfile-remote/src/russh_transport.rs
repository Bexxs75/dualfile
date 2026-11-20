//! The real SFTP transport: `russh` (SSH, pure Rust, `ring` for the crypto) and `russh-sftp`
//! (the protocol). There is no OpenSSL and no libssh2.
//!
//! What this connection does and does not do:
//! - only the `sftp` subsystem is requested on one session channel; no shell, no exec, no
//!   pty, no port or agent forwarding, no X11 (the code has no path that opens such a channel);
//! - login by password or by a private key file (OpenSSH format, optional passphrase); there is
//!   no SSH agent support in this phase;
//! - the host key is checked in `check_server_key` by the pin policy (`sftp_trust`); the
//!   algorithm list is limited to `ssh-ed25519`, `ecdsa-sha2-nistp{256,384,521}`,
//!   `rsa-sha2-512`, `rsa-sha2-256` (no `ssh-dss`, no `ssh-rsa` with SHA-1); no setting
//!   switches the check off;
//! - key exchange, ciphers and MACs are russh's defaults (curve25519, ML-KEM hybrid, DH group
//!   exchange with SHA-2; ChaCha20-Poly1305, AES-GCM/CTR; HMAC-SHA-2 only), no compression;
//! - every request has a time limit (`Timeouts::idle`), connecting has its own; keep-alive
//!   messages keep an idle session open and detect a dead one.
//!
//! The library is asynchronous; a small shared runtime drives it and every call here blocks
//! the calling (non-async) thread until the answer or the time limit.
//!
//! Not covered by `cargo test` (no network there): this file is exercised by hand against a
//! real server only. `sftp_fake` stands in for it in tests.

use crate::error::{sanitize_text, SftpStatus, TResult, TransportError};
use crate::profile::{SftpAuth, SftpConnectParams};
use crate::secret::Secret;
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::transport::Timeouts;
use crate::trust::{CertVerifier, TrustError};
use russh::client::{self, AuthResult, Handler};
use russh::keys::{
    Algorithm, EcdsaCurve, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{Config as SftpConfig, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const POSIX_RENAME: &str = "posix-rename@openssh.com";
/// Largest private key file read.
const MAX_KEY_FILE: u64 = 1 << 20;
/// Largest piece asked for in one read request.
const MAX_READ: usize = 256 * 1024;
const WRITE_PIECE: usize = 32 * 1024;

pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("dualfile-sftp")
            .enable_time()
            .enable_io()
            .build()
            .expect("the SFTP runtime starts")
    })
}

/// What the host key handler reports.
#[derive(Debug)]
enum HandlerError {
    Ssh(russh::Error),
    Trust(TrustError),
}

impl From<russh::Error> for HandlerError {
    fn from(error: russh::Error) -> Self {
        Self::Ssh(error)
    }
}

struct HostKeyHandler {
    verifier: Arc<dyn CertVerifier>,
    host: String,
    port: u16,
}

impl Handler for HostKeyHandler {
    type Error = HandlerError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // certificates are never offered (see `host_key_certificates`); refuse if one comes
        let PublicKeyOrCertificate::PublicKey { key, hash_alg } = server_public_key else {
            return Err(HandlerError::Trust(TrustError::AlgorithmNotAllowed(
                "Zertifikat".into(),
            )));
        };
        let signature_algorithm = match key.algorithm() {
            Algorithm::Rsa { .. } => Algorithm::Rsa { hash: *hash_alg },
            other => other,
        };
        let blob = key
            .to_bytes()
            .map_err(|_| HandlerError::Trust(TrustError::InvalidChain))?;
        let verifier = self.verifier.clone();
        let (host, port) = (self.host.clone(), self.port);
        let key_type = key.algorithm();
        // the answer may wait for the user: do not stall the runtime's other tasks
        let outcome = tokio::task::block_in_place(|| {
            crate::sftp_trust::check_host_key(
                verifier.as_ref(),
                &host,
                port,
                signature_algorithm.as_str(),
                key_type.as_str(),
                &blob,
            )
        });
        match outcome {
            Ok(_) => Ok(true),
            Err(error) => Err(HandlerError::Trust(error)),
        }
    }
}

fn map_ssh_error(error: HandlerError) -> TransportError {
    match error {
        HandlerError::Trust(error) => TransportError::Trust(error),
        HandlerError::Ssh(error) => ssh_text(&error),
    }
}

fn ssh_text(error: &russh::Error) -> TransportError {
    TransportError::Connection(sanitize_text(&error.to_string(), 200))
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
            TransportError::status(code, &status.error_message)
        }
        SftpError::Timeout => TransportError::Timeout,
        SftpError::Limited(text) => TransportError::Protocol(sanitize_text(&text, 200)),
        SftpError::IO(text) | SftpError::UnexpectedBehavior(text) => {
            TransportError::Connection(sanitize_text(&text, 200))
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

fn load_key(path: &str, passphrase: &Secret) -> TResult<PrivateKey> {
    let meta = std::fs::metadata(path)
        .map_err(|_| TransportError::Connection("Die Schlüsseldatei ist nicht lesbar".into()))?;
    if !meta.is_file() || meta.len() > MAX_KEY_FILE {
        return Err(TransportError::Connection(
            "Die Schlüsseldatei ist nicht lesbar".into(),
        ));
    }
    let pass = (!passphrase.is_empty()).then(|| passphrase.expose());
    russh::keys::load_secret_key(path, pass).map_err(|_| {
        // no library text: it could be about the key material
        TransportError::Connection(
            "Die Schlüsseldatei konnte nicht gelesen werden (Format oder Passphrase falsch)".into(),
        )
    })
}

/// Runs one request with the time limit; a connection error marks the transport dead.
fn call_on<R>(
    dead: &mut bool,
    closed: bool,
    idle: Duration,
    request: impl Future<Output = Result<R, SftpError>>,
) -> TResult<R> {
    if *dead || closed {
        *dead = true;
        return Err(TransportError::Connection("Verbindung getrennt".into()));
    }
    let limit = idle + Duration::from_secs(2);
    let outcome = runtime().block_on(async {
        tokio::time::timeout(limit, request)
            .await
            .unwrap_or(Err(SftpError::Timeout))
    });
    outcome.map_err(|error| {
        let mapped = map_sftp_error(error);
        if mapped.is_connection_lost() {
            *dead = true;
        }
        mapped
    })
}

/// `call_on` where an end-of-file status means `None`.
fn call_eof_on<R>(
    dead: &mut bool,
    closed: bool,
    idle: Duration,
    request: impl Future<Output = Result<R, SftpError>>,
) -> TResult<Option<R>> {
    call_on(dead, closed, idle, async {
        match request.await {
            Ok(value) => Ok(Some(value)),
            Err(error) if is_eof(&error) => Ok(None),
            Err(error) => Err(error),
        }
    })
}

macro_rules! call {
    ($self:ident, $request:expr) => {
        call_on(
            &mut $self.dead,
            $self.handle.is_closed(),
            $self.idle,
            $request,
        )
    };
}

macro_rules! call_eof {
    ($self:ident, $request:expr) => {
        call_eof_on(
            &mut $self.dead,
            $self.handle.is_closed(),
            $self.idle,
            $request,
        )
    };
}

pub struct RusshTransport {
    handle: client::Handle<HostKeyHandler>,
    sftp: RawSftpSession,
    features: SftpFeatures,
    statvfs: bool,
    handles: HashMap<u64, String>,
    next_handle: u64,
    idle: Duration,
    dead: bool,
}

impl std::fmt::Debug for RusshTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RusshTransport").finish_non_exhaustive()
    }
}

impl RusshTransport {
    /// Connects, checks the host key, logs in and starts the `sftp` subsystem.
    /// `secret` is the password, or the key file's passphrase (see `SftpAuth`).
    pub fn connect(
        params: &SftpConnectParams,
        secret: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self> {
        let profile = &params.profile;
        profile.validate().map_err(|_| TransportError::Rejected)?;
        // a key file is read before any network use
        let key = match &profile.auth {
            SftpAuth::KeyFile { path } => Some(Arc::new(load_key(path, secret)?)),
            SftpAuth::Password => None,
        };
        let timeouts = params.timeouts;
        // generous outer bound: connect + key exchange + login + subsystem
        let overall = timeouts.connect + timeouts.idle * 3;
        let result = runtime().block_on(async {
            tokio::time::timeout(overall, Self::connect_async(params, secret, key, verifier))
                .await
                .unwrap_or(Err(TransportError::Timeout))
        });
        result.map_err(|error| error.scrubbed(secret))
    }

    async fn connect_async(
        params: &SftpConnectParams,
        secret: &Secret,
        key: Option<Arc<PrivateKey>>,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self> {
        let profile = &params.profile;
        let Timeouts { connect, idle } = params.timeouts;
        let mut config = client::Config::default();
        // host key / signature algorithms: see `sftp_trust::ALLOWED_HOST_KEY_ALGORITHMS`
        config.preferred.key = Cow::Owned(vec![
            Algorithm::Ed25519,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521,
            },
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha512),
            },
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha256),
            },
        ]);
        config.keepalive_interval = Some(Duration::from_secs(30));
        config.keepalive_max = 3;
        config.nodelay = true;
        let handler = HostKeyHandler {
            verifier,
            host: profile.host.clone(),
            port: profile.port,
        };
        let stream = tokio::time::timeout(
            connect,
            tokio::net::TcpStream::connect((profile.host.as_str(), profile.port)),
        )
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::Connection("Der Server ist nicht erreichbar".into()))?;
        let mut handle = tokio::time::timeout(
            connect + idle,
            client::connect_stream(Arc::new(config), stream, handler),
        )
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(map_ssh_error)?;

        let user = profile.user.clone();
        let result = match (&profile.auth, key) {
            (SftpAuth::Password, _) => handle
                .authenticate_password(user, secret.expose())
                .await
                .map_err(|e| ssh_text(&e))?,
            (SftpAuth::KeyFile { .. }, Some(key)) => {
                // RSA keys sign with SHA-2 only: try 512, then 256 (ed25519/ecdsa ignore it)
                let first = handle
                    .authenticate_publickey(
                        user.clone(),
                        PrivateKeyWithHashAlg::new(key.clone(), Some(HashAlg::Sha512)),
                    )
                    .await
                    .map_err(|e| ssh_text(&e))?;
                if first.success() || !key.algorithm().is_rsa() {
                    first
                } else {
                    handle
                        .authenticate_publickey(
                            user,
                            PrivateKeyWithHashAlg::new(key, Some(HashAlg::Sha256)),
                        )
                        .await
                        .map_err(|e| ssh_text(&e))?
                }
            }
            (SftpAuth::KeyFile { .. }, None) => return Err(TransportError::Auth),
        };
        if !matches!(result, AuthResult::Success) {
            return Err(TransportError::Auth);
        }

        // the one channel: a session with the sftp subsystem, nothing else
        let channel = tokio::time::timeout(idle, handle.channel_open_session())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| ssh_text(&e))?;
        tokio::time::timeout(idle, channel.request_subsystem(true, "sftp"))
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| ssh_text(&e))?;
        let sftp = RawSftpSession::new_with_config(
            channel.into_stream(),
            SftpConfig {
                request_timeout_secs: idle.as_secs().max(1),
                ..SftpConfig::default()
            },
        );
        let version = tokio::time::timeout(idle, sftp.init())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(map_sftp_error)?;
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
            handle,
            sftp,
            statvfs: version
                .extensions
                .get("statvfs@openssh.com")
                .is_some_and(|v| v == "2"),
            features,
            handles: HashMap::new(),
            next_handle: 1,
            idle,
            dead: false,
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

impl SftpTransport for RusshTransport {
    fn space_free(&mut self, path: &str) -> TResult<Option<dualfile_core::vfs::SpaceFree>> {
        check_text(path)?;
        if !self.statvfs {
            return Ok(None);
        }
        if self.dead || self.handle.is_closed() {
            return Ok(None);
        }
        // Optional status data must release the transport lock promptly, even on slow servers.
        let stat = runtime().block_on(async {
            tokio::time::timeout(Duration::from_millis(500), self.sftp.statvfs(path)).await
        });
        let Ok(Ok(stat)) = stat else {
            return Ok(None);
        };
        Ok(stat
            .blocks_avail
            .checked_mul(stat.fragment_size)
            .zip(stat.blocks.checked_mul(stat.fragment_size))
            .map(|(available, total)| dualfile_core::vfs::SpaceFree { available, total }))
    }

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
        self.dead = true;
        let _ = self.sftp.close_session();
        let handle = &self.handle;
        let _ = runtime().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                handle.disconnect(russh::Disconnect::ByApplication, "", "en"),
            )
            .await
        });
    }
}

impl Drop for RusshTransport {
    fn drop(&mut self) {
        if !self.dead {
            self.quit();
        }
    }
}

/// The real SFTP connector.
#[derive(Default)]
pub struct RusshConnector;

impl crate::manager::SftpConnector for RusshConnector {
    type Transport = RusshTransport;
    fn connect(
        &self,
        params: &SftpConnectParams,
        secret: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport> {
        RusshTransport::connect(params, secret, verifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::Protocol;

    fn attrs(permissions: Option<u32>) -> FileAttributes {
        FileAttributes {
            size: Some(7),
            permissions,
            mtime: Some(1_600_000_000),
            ..FileAttributes::empty()
        }
    }

    #[test]
    fn attributes_map_onto_kinds_permissions_and_times() {
        for (mode, kind) in [
            (0o040_755, RawKind::Dir),
            (0o100_644, RawKind::File),
            (0o120_777, RawKind::Symlink),
            (0o010_644, RawKind::Other),
            (0o020_644, RawKind::Other),
            (0o060_644, RawKind::Other),
            (0o140_644, RawKind::Other),
        ] {
            assert_eq!(raw_attrs(&attrs(Some(mode))).kind, kind, "{mode:o}");
        }
        assert_eq!(raw_attrs(&attrs(None)).kind, RawKind::Unknown);
        let a = raw_attrs(&attrs(Some(0o104_755)));
        assert_eq!(a.permissions, Some(0o4755));
        assert_eq!(a.size, Some(7));
        assert_eq!(to_secs(a.mtime.unwrap()), 1_600_000_000);
    }

    #[test]
    fn statuses_map_onto_transport_errors_and_stay_bounded() {
        let status = |code, text: &str| {
            SftpError::Status(russh_sftp::protocol::Status {
                id: 1,
                status_code: code,
                error_message: text.into(),
                language_tag: String::new(),
            })
        };
        assert!(matches!(
            map_sftp_error(status(StatusCode::NoSuchFile, "x")),
            TransportError::Status {
                code: SftpStatus::NoSuchFile,
                ..
            }
        ));
        let long = format!("a\r\nb{}", "x".repeat(1000));
        let text = map_sftp_error(status(StatusCode::Failure, &long)).to_string();
        assert!(!text.contains('\n') && text.len() < 400, "{text}");
        assert!(map_sftp_error(status(StatusCode::ConnectionLost, "")).is_connection_lost());
        assert!(!map_sftp_error(status(StatusCode::Failure, "")).is_connection_lost());
        assert!(matches!(
            map_sftp_error(SftpError::Timeout),
            TransportError::Timeout
        ));
        assert!(is_eof(&status(StatusCode::Eof, "")));
        assert!(!is_eof(&status(StatusCode::Failure, "")));
    }

    #[test]
    fn key_files_that_are_missing_or_huge_are_refused_without_leaking() {
        let dir = std::env::temp_dir();
        let missing = dir.join("dualfile-no-such-key-file");
        let e = load_key(missing.to_str().unwrap(), &Secret::new("pp-MARKER")).unwrap_err();
        assert!(!e.to_string().contains("MARKER"));
        let bogus = tempfile_in(&dir, b"not a key");
        let e = load_key(bogus.to_str().unwrap(), &Secret::new("pp-MARKER")).unwrap_err();
        assert!(!e.to_string().contains("MARKER") && !format!("{e:?}").contains("MARKER"));
        std::fs::remove_file(bogus).ok();
        assert!(check_text("a\nb").is_err() && check_text("/ok").is_ok());
    }

    fn tempfile_in(dir: &std::path::Path, data: &[u8]) -> std::path::PathBuf {
        let path = dir.join(format!("dualfile-key-test-{}", std::process::id()));
        std::fs::write(&path, data).unwrap();
        path
    }

    /// Manual integration test against a real SSH server (never run by plain `cargo test`).
    /// `DUALFILE_SFTP_TEST=host:port:user:keyfile:passphrase:workdir` (the work directory must
    /// exist on the server and is only used below). Example: an unprivileged `sshd` on
    /// 127.0.0.1. Run with `cargo test --offline -p dualfile-remote -- --ignored real_server`.
    #[test]
    #[ignore = "needs a real SSH server, see DUALFILE_SFTP_TEST"]
    fn real_server_round_trip() {
        use crate::manager::SftpConnector;
        use crate::profile::SftpProfile;
        use crate::sftpvfs::{SftpConfig, SftpVfs};
        use crate::trust::{
            ConfirmTrust, MemoryPins, PinStore, PolicyVerifier, TrustDecision, TrustRequest,
        };
        use dualfile_core::ops::{self, JobHandle};
        use dualfile_core::vfs::{ReplaceMode, VPath, Vfs, WriteMode};
        use std::io::{Read, Write};

        struct Yes;
        impl ConfirmTrust for Yes {
            fn confirm(&self, _: &TrustRequest) -> TrustDecision {
                TrustDecision::TrustAlways
            }
        }
        let spec = std::env::var("DUALFILE_SFTP_TEST").expect("DUALFILE_SFTP_TEST");
        let parts: Vec<&str> = spec.splitn(6, ':').collect();
        let [host, port, user, key, phrase, work] = parts[..] else {
            panic!("host:port:user:keyfile:passphrase:workdir")
        };
        let profile = SftpProfile {
            transport: crate::profile::SshTransport::Builtin,
            name: "t".into(),
            host: host.into(),
            port: port.parse().unwrap(),
            user: user.into(),
            auth: SftpAuth::KeyFile { path: key.into() },
            start_folder: None,
        };
        let params = SftpConnectParams::new(profile.clone());
        let pins = Arc::new(MemoryPins::new());
        let verifier: Arc<dyn CertVerifier> =
            Arc::new(PolicyVerifier::new(pins.clone(), Arc::new(Yes)));
        let connector = crate::russh_transport::RusshConnector;
        // wrong passphrase: refused, the passphrase is in no text
        let bad = connector
            .connect(&params, &Secret::new("wrong-MARKER"), verifier.clone())
            .unwrap_err();
        assert!(!format!("{bad} {bad:?}").contains("MARKER"), "{bad}");
        let secret = Secret::new(phrase);
        let first = connector
            .connect(&params, &secret, verifier.clone())
            .unwrap();
        assert_eq!(
            pins.get(&crate::trust::PinKey::new(
                Protocol::Sftp,
                host,
                params.profile.port
            ))
            .map(|p| p.key_type),
            Some(Some("ssh-ed25519".into()))
        );
        let (p2, s2, v2) = (params.clone(), secret.clone(), verifier.clone());
        let opener: crate::ftpvfs::Opener<RusshTransport> =
            Arc::new(move || crate::russh_transport::RusshConnector.connect(&p2, &s2, v2.clone()));
        let vfs = SftpVfs::new("r1", first, opener, SftpConfig::default(), Some(secret)).unwrap();
        println!("posix-rename: {}", vfs.home_dir().is_ok());
        let at = |name: &str| VPath::sftp("r1", &format!("{work}/{name}")).unwrap();
        let _ = vfs.remove_permanent(&at("t.bin"));
        vfs.mkdir(&at("dir")).unwrap();
        assert!(matches!(
            vfs.mkdir(&at("dir")),
            Err(dualfile_core::Error::Exists(_))
        ));
        // upload (resumable path), download, compare
        let tmp = std::env::temp_dir().join(format!("dualfile-real-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let data: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(tmp.join("t.bin"), &data).unwrap();
        let report = ops::copy(
            &dualfile_core::LocalVfs,
            &[VPath::local(tmp.join("t.bin")).unwrap()],
            &*vfs,
            &VPath::sftp("r1", work).unwrap(),
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let mut got = Vec::new();
        vfs.open_read(&at("t.bin"))
            .unwrap()
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, data);
        assert_eq!(
            vfs.read_range(&at("t.bin"), 1000, 10).unwrap(),
            &data[1000..1010]
        );
        // listing, links, rename, set_metadata
        let listed = vfs.list(&VPath::sftp("r1", work).unwrap()).unwrap();
        assert!(listed
            .iter()
            .any(|e| e.name == "t.bin" && e.size == data.len() as u64));
        vfs.create_symlink("t.bin", &at("lnk")).unwrap();
        assert_eq!(vfs.read_link(&at("lnk")).unwrap(), "t.bin");
        assert_eq!(
            vfs.stat(&at("lnk")).unwrap().kind,
            dualfile_core::vfs::EntryKind::Symlink
        );
        let when = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        vfs.set_metadata(&at("t.bin"), Some(0o640), Some(when))
            .unwrap();
        let st = vfs.stat(&at("t.bin")).unwrap();
        assert_eq!(st.mode.map(|m| m & 0o777), Some(0o640));
        assert_eq!(st.modified, Some(when));
        vfs.rename(&at("t.bin"), &at("u.bin"), ReplaceMode::NoReplace)
            .unwrap();
        std::fs::write(tmp.join("x"), b"x").unwrap();
        let mut w = vfs
            .create_write(&at("u.bin"), WriteMode::NoReplace)
            .unwrap();
        w.write_all(b"z").unwrap();
        assert!(matches!(w.commit(), Err(dualfile_core::Error::Exists(_))));
        drop(w);
        let mut w = vfs.create_write(&at("u.bin"), WriteMode::Replace).unwrap();
        w.write_all(b"replaced").unwrap();
        w.commit().unwrap();
        assert_eq!(vfs.stat(&at("u.bin")).unwrap().size, 8);
        vfs.remove_permanent(&at("lnk")).unwrap();
        vfs.remove_permanent(&at("u.bin")).unwrap();
        vfs.remove_permanent(&at("dir")).unwrap();
        let left: Vec<_> = vfs
            .list(&VPath::sftp("r1", work).unwrap())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::remove_dir_all(&tmp).ok();
        // a pinned key that differs is refused
        pins.set(crate::trust::Pin {
            protocol: Protocol::Sftp,
            host: host.to_ascii_lowercase(),
            port: params.profile.port,
            sha256: "0".repeat(64),
            key_type: None,
        });
        let e = connector
            .connect(&params, &Secret::new(phrase), verifier)
            .unwrap_err();
        assert!(
            matches!(e, TransportError::Trust(TrustError::PinMismatch { .. })),
            "{e}"
        );
        vfs.close();
    }
}
