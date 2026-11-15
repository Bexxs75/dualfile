//! The connection manager: opens sessions from profiles, hands out connection ids, registers
//! the back ends in the router, closes them again and keeps the number of open connections
//! below a limit.
//!
//! The password is passed in as a `Secret` for the login (and kept inside the reconnect
//! closure of the session so a lost control connection can be replaced); it never appears in
//! an error, a `Debug` output or a returned value.

use crate::error::{TResult, TransportError};
use crate::ftpvfs::{FtpConfig, FtpVfs, Opener};
use crate::profile::{ConnectParams, FtpProfile, SftpConnectParams, SftpProfile, TlsMode};
use crate::secret::Secret;
use crate::sftp_transport::SftpTransport;
use crate::sftpvfs::{SftpConfig, SftpVfs};
use crate::transport::FtpTransport;
use crate::trust::{CertVerifier, ConfirmTrust, PinKey, PinStore, PolicyVerifier, Protocol};
use dualfile_core::vfs::{VPath, Vfs};
use dualfile_core::VfsRouter;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Makes logged-in connections (the real one speaks FTP, the test one talks to a fake).
pub trait Connector: Send + Sync + 'static {
    type Transport: FtpTransport;
    fn connect(
        &self,
        params: &ConnectParams,
        password: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport>;
}

/// Makes logged-in SFTP connections (the real one speaks SSH, the test one talks to a fake).
/// The secret is the password, or the passphrase of the key file (see `SftpAuth`).
pub trait SftpConnector: Send + Sync + 'static {
    type Transport: SftpTransport;
    fn connect(
        &self,
        params: &SftpConnectParams,
        secret: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport>;
}

/// The "no SFTP" connector of a manager built with `ConnectionManager::new`.
pub struct NoSftp;

impl SftpConnector for NoSftp {
    type Transport = crate::russh_transport::RusshTransport;
    fn connect(
        &self,
        _: &SftpConnectParams,
        _: &Secret,
        _: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport> {
        Err(TransportError::Unsupported("SFTP in diesem Aufbau"))
    }
}

#[derive(Clone, Debug)]
pub struct ManagerConfig {
    /// Sessions open at the same time (FTP and SFTP together).
    pub max_connections: usize,
    pub ftp: FtpConfig,
    pub sftp: SftpConfig,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            max_connections: 8,
            ftp: FtpConfig::default(),
            sftp: SftpConfig::default(),
        }
    }
}

/// What the user interface may know about an open connection (never the password).
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub protocol: Protocol,
    /// The TLS mode of an FTP/FTPS connection; `None` for SFTP.
    pub tls: Option<TlsMode>,
    /// Where the connection starts (`ftp:<id>!<start folder>` / `sftp:<id>!<start folder>`).
    pub root: VPath,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Es sind schon zu viele Verbindungen offen")]
    TooManyConnections,
    #[error(transparent)]
    Transport(#[from] TransportError),
}

impl ConnectError {
    /// The trust error behind this failure, if that was the reason (pin changed, certificate
    /// not confirmed, expired, ...).
    pub fn trust_error(&self) -> Option<&crate::trust::TrustError> {
        match self {
            Self::Transport(TransportError::Trust(error)) => Some(error),
            _ => None,
        }
    }
}

/// What the manager needs from a back end to close it again.
trait Closable: Send + Sync {
    fn close(&self);
    fn is_live(&self) -> bool;
}

impl<T: FtpTransport> Closable for FtpVfs<T> {
    fn is_live(&self) -> bool {
        FtpVfs::is_live(self)
    }
    fn close(&self) {
        FtpVfs::close(self)
    }
}

impl<T: SftpTransport> Closable for SftpVfs<T> {
    fn is_live(&self) -> bool {
        SftpVfs::is_live(self)
    }
    fn close(&self) {
        SftpVfs::close(self)
    }
}

/// WebDAV is stateless HTTP, so `close` cannot drop a socket; instead it revokes the transport so no
/// later request (including a queued job or a pending write commit) can still reach the server.
impl Closable for crate::webdavvfs::WebdavVfs {
    fn is_live(&self) -> bool {
        true
    }
    fn close(&self) {
        crate::webdavvfs::WebdavVfs::close(self);
    }
}

struct Session {
    info: ConnectionInfo,
    vfs: Arc<dyn Closable>,
}

struct State {
    sessions: BTreeMap<String, Session>,
    /// Connections being opened right now (they count against the limit).
    pending: usize,
    counter: u64,
}

/// A finished connection and its back end.
type Opened = (ConnectionInfo, Arc<dyn Closable>);

pub struct ConnectionManager<C: Connector, S: SftpConnector = NoSftp> {
    connector: Arc<C>,
    sftp: Arc<S>,
    router: Arc<VfsRouter>,
    pins: Arc<dyn PinStore>,
    cfg: ManagerConfig,
    state: Mutex<State>,
}

impl<C: Connector, S: SftpConnector> std::fmt::Debug for ConnectionManager<C, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionManager")
            .field("max_connections", &self.cfg.max_connections)
            .finish_non_exhaustive()
    }
}

impl<C: Connector> ConnectionManager<C, NoSftp> {
    /// A manager for FTP/FTPS only (`connect_sftp` is refused).
    pub fn new(
        connector: C,
        router: Arc<VfsRouter>,
        pins: Arc<dyn PinStore>,
        cfg: ManagerConfig,
    ) -> Self {
        Self::with_sftp(connector, NoSftp, router, pins, cfg)
    }
}

impl<C: Connector, S: SftpConnector> ConnectionManager<C, S> {
    pub fn with_sftp(
        connector: C,
        sftp: S,
        router: Arc<VfsRouter>,
        pins: Arc<dyn PinStore>,
        cfg: ManagerConfig,
    ) -> Self {
        Self {
            connector: Arc::new(connector),
            sftp: Arc::new(sftp),
            router,
            pins,
            cfg,
            state: Mutex::new(State {
                sessions: BTreeMap::new(),
                pending: 0,
                counter: 0,
            }),
        }
    }

    /// Opens a session. `confirm` is asked about a certificate that needs the user's decision
    /// (once per connect, and again for a reconnect of a lost control connection).
    pub fn connect(
        &self,
        profile: FtpProfile,
        password: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError> {
        profile.validate().map_err(ConnectError::Invalid)?;
        self.reserve()?;
        let outcome = self.open(profile, password, confirm);
        self.finish(outcome)
    }

    /// Opens an SFTP session. `secret` is the password, or the passphrase of the key file
    /// (empty = none). The host key is judged by the same pin policy as FTPS certificates:
    /// the first key must be confirmed (`confirm`), a changed key is refused.
    pub fn connect_sftp(
        &self,
        profile: SftpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError> {
        profile.validate().map_err(ConnectError::Invalid)?;
        self.reserve()?;
        let outcome = self.open_sftp(profile, secret, confirm);
        self.finish(outcome)
    }

    pub fn connect_smb(
        &self,
        profile: crate::smb::SmbProfile,
        secret: Secret,
    ) -> Result<ConnectionInfo, ConnectError> {
        profile.validate().map_err(ConnectError::Invalid)?;
        self.reserve()?;
        let outcome = (|| {
            let cfg = self.cfg.sftp.clone();
            let first = crate::smb::SmbTransport::connect(&profile, &secret, cfg.timeouts)?;
            let id = self.next_id();
            let p = profile.clone();
            let password = secret.clone();
            let timeouts = cfg.timeouts;
            let opener =
                Arc::new(move || crate::smb::SmbTransport::connect(&p, &password, timeouts));
            let vfs = crate::smb::SmbVfs::with_scheme(
                &id,
                first,
                opener,
                cfg,
                Some(secret),
                dualfile_core::vfs::Scheme::Smb,
            )
            .map_err(|_| ConnectError::Invalid("Ungültige Verbindungskennung"))?;
            let root = VPath::smb(&id, profile.start_folder.as_deref().unwrap_or("/"))
                .map_err(|_| ConnectError::Invalid("Ungültiger Startordner"))?;
            self.router.register_remote(&id, vfs.clone()).map_err(|_| {
                ConnectError::Invalid("Die Verbindung konnte nicht angemeldet werden")
            })?;
            let info = ConnectionInfo {
                id,
                name: profile.name,
                host: profile.host,
                port: profile.port,
                user: profile.user,
                protocol: Protocol::Smb,
                tls: None,
                root,
            };
            Ok((info, vfs as Arc<dyn Closable>))
        })();
        self.finish(outcome)
    }

    pub fn connect_webdav(
        &self,
        profile: crate::profile::WebdavProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError> {
        profile.validate().map_err(ConnectError::Invalid)?;
        self.reserve()?;
        let outcome = (|| {
            let scheme_str = if profile.tls { "https" } else { "http" };
            let origin = format!("{scheme_str}://{}:{}", profile.host, profile.port);
            let user = (!profile.user.is_empty()).then_some(profile.user.as_str());
            let password = user.map(|_| secret.expose());
            // For https, pin and confirm the certificate through the same verifier the FTPS path
            // uses (system roots + fingerprint pin + user confirmation); http carries no TLS.
            let pinning = profile.tls.then(|| {
                let verifier: Arc<dyn CertVerifier> =
                    Arc::new(PolicyVerifier::new(self.pins.clone(), confirm));
                crate::tls::PinningServerVerifier::new(
                    verifier,
                    PinKey::new(Protocol::Webdav, &profile.host, profile.port),
                    crate::tls::system_roots(),
                )
            });
            let tls_config = pinning
                .as_ref()
                .map(|p| crate::tls::client_config(p.clone()));
            let transport = Arc::new(crate::webdav_transport::UreqTransport::new_tls(
                &origin, user, password, tls_config,
            ));
            let id = self.next_id();
            let vfs = Arc::new(
                crate::webdavvfs::WebdavVfs::new(transport, &profile.base_path)
                    .with_connection(&id),
            );
            let root = VPath::webdav(&id, profile.start_folder.as_deref().unwrap_or("/"))
                .map_err(|_| ConnectError::Invalid("Ungültiger Startordner"))?;
            // Verify reachability and credentials with one PROPFIND on the base before reporting
            // success, so a wrong password is not accepted and later stored, and a reconnect with
            // corrected credentials builds a fresh transport instead of reusing a failed one.
            let base = VPath::webdav(&id, "/")
                .map_err(|_| ConnectError::Invalid("Ungültiger WebDAV-Basispfad"))?;
            vfs.stat(&base).map_err(|e| {
                // A refused certificate surfaces here as a generic transport failure (rustls only
                // reports a general error); recover the real trust reason so the connect flow can
                // prompt the user to confirm the fingerprint, exactly like FTPS/SFTP.
                if let Some(trust) = pinning.as_ref().and_then(|p| p.take_error()) {
                    return ConnectError::Transport(TransportError::Trust(trust));
                }
                match e {
                    dualfile_core::error::Error::PermissionDenied(_) => {
                        ConnectError::Invalid("WebDAV-Anmeldung fehlgeschlagen")
                    }
                    _ => ConnectError::Invalid("WebDAV-Server nicht erreichbar"),
                }
            })?;
            self.router.register_remote(&id, vfs.clone()).map_err(|_| {
                ConnectError::Invalid("Die Verbindung konnte nicht angemeldet werden")
            })?;
            let info = ConnectionInfo {
                id,
                name: profile.name,
                host: profile.host,
                port: profile.port,
                user: profile.user,
                protocol: Protocol::Webdav,
                tls: None,
                root,
            };
            Ok((info, vfs as Arc<dyn Closable>))
        })();
        self.finish(outcome)
    }

    fn reserve(&self) -> Result<(), ConnectError> {
        let mut state = self.state.lock().unwrap();
        if state.sessions.len() + state.pending >= self.cfg.max_connections {
            return Err(ConnectError::TooManyConnections);
        }
        state.pending += 1;
        Ok(())
    }

    fn finish(
        &self,
        outcome: Result<Opened, ConnectError>,
    ) -> Result<ConnectionInfo, ConnectError> {
        let mut state = self.state.lock().unwrap();
        state.pending -= 1;
        let (info, vfs) = outcome?;
        state.sessions.insert(
            info.id.clone(),
            Session {
                info: info.clone(),
                vfs,
            },
        );
        Ok(info)
    }

    fn open_sftp(
        &self,
        profile: SftpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<Opened, ConnectError> {
        let params = SftpConnectParams::new(profile.clone());
        let verifier: Arc<dyn CertVerifier> =
            Arc::new(PolicyVerifier::new(self.pins.clone(), confirm));
        let first = self
            .sftp
            .connect(&params, &secret, verifier.clone())
            .map_err(|error| error.scrubbed(&secret))?;
        let id = self.next_id();
        let connector = self.sftp.clone();
        let opener_params = params.clone();
        let opener_secret = secret.clone();
        let opener_verifier = verifier;
        let opener: crate::ftpvfs::Opener<S::Transport> = Arc::new(move || {
            connector
                .connect(&opener_params, &opener_secret, opener_verifier.clone())
                .map_err(|error| error.scrubbed(&opener_secret))
        });
        let mut cfg = self.cfg.sftp.clone();
        cfg.timeouts = params.timeouts;
        cfg.limits = params.limits;
        let vfs = SftpVfs::new(&id, first, opener, cfg, Some(secret))
            .map_err(|_| ConnectError::Invalid("Ungültige Verbindungskennung"))?;
        let registered: Arc<dyn Vfs> = vfs.clone();
        self.router
            .register_remote(&id, registered)
            .map_err(|_| ConnectError::Invalid("Die Verbindung konnte nicht angemeldet werden"))?;
        let start = match &profile.start_folder {
            Some(folder) => folder.clone(),
            None => vfs.home_dir().unwrap_or_else(|_| "/".to_owned()),
        };
        let root = VPath::sftp(&id, &start)
            .or_else(|_| VPath::sftp(&id, "/"))
            .map_err(|_| ConnectError::Invalid("Ungültiger Startordner"))?;
        let info = ConnectionInfo {
            id,
            name: profile.name,
            host: profile.host,
            port: profile.port,
            user: profile.user,
            protocol: Protocol::Sftp,
            tls: None,
            root,
        };
        let closable: Arc<dyn Closable> = vfs;
        Ok((info, closable))
    }

    fn open(
        &self,
        profile: FtpProfile,
        password: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<Opened, ConnectError> {
        let params = ConnectParams::new(profile.clone());
        let verifier: Arc<dyn CertVerifier> =
            Arc::new(PolicyVerifier::new(self.pins.clone(), confirm));
        let first = self
            .connector
            .connect(&params, &password, verifier.clone())
            .map_err(|error| error.scrubbed(&password))?;
        let id = self.next_id();
        let connector = self.connector.clone();
        let opener_params = params.clone();
        let opener_secret = password.clone();
        let opener_verifier = verifier;
        let opener: Opener<C::Transport> = Arc::new(move || {
            connector
                .connect(&opener_params, &opener_secret, opener_verifier.clone())
                .map_err(|error| error.scrubbed(&opener_secret))
        });
        let mut ftp_cfg = self.cfg.ftp.clone();
        ftp_cfg.timeouts = params.timeouts;
        ftp_cfg.limits = params.limits;
        let vfs = FtpVfs::new(&id, first, opener, ftp_cfg, Some(password))
            .map_err(|_| ConnectError::Invalid("Ungültige Verbindungskennung"))?;
        let registered: Arc<dyn Vfs> = vfs.clone();
        self.router
            .register_remote(&id, registered)
            .map_err(|_| ConnectError::Invalid("Die Verbindung konnte nicht angemeldet werden"))?;
        let start = profile
            .start_folder
            .clone()
            .unwrap_or_else(|| "/".to_owned());
        let root = VPath::ftp(&id, &start)
            .or_else(|_| VPath::ftp(&id, "/"))
            .map_err(|_| ConnectError::Invalid("Ungültiger Startordner"))?;
        let info = ConnectionInfo {
            id,
            name: profile.name,
            host: profile.host,
            port: profile.port,
            user: profile.user,
            protocol: Protocol::Ftps,
            tls: Some(profile.tls),
            root,
        };
        let closable: Arc<dyn Closable> = vfs;
        Ok((info, closable))
    }

    fn next_id(&self) -> String {
        let mut state = self.state.lock().unwrap();
        state.counter += 1;
        // the time part keeps an id from a previous run from being reused by accident
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() & 0xfffff)
            .unwrap_or(0);
        format!("c{}-{:05x}", state.counter, stamp)
    }

    /// Closes a session: its locations are disconnected at once.
    pub fn disconnect(&self, id: &str) -> bool {
        let session = self.state.lock().unwrap().sessions.remove(id);
        // the router entry goes first: nothing new can reach the back end afterwards
        self.router.unregister(id);
        match session {
            Some(session) => {
                session.vfs.close();
                true
            }
            None => false,
        }
    }

    pub fn disconnect_all(&self) {
        let ids: Vec<String> = self
            .state
            .lock()
            .unwrap()
            .sessions
            .keys()
            .cloned()
            .collect();
        for id in ids {
            self.disconnect(&id);
        }
    }

    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.state
            .lock()
            .unwrap()
            .sessions
            .values()
            .map(|s| s.info.clone())
            .collect()
    }

    pub fn is_live(&self, id: &str) -> bool {
        let vfs = self
            .state
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .map(|s| s.vfs.clone());
        vfs.is_some_and(|vfs| vfs.is_live())
    }

    pub fn is_connected(&self, id: &str) -> bool {
        self.state.lock().unwrap().sessions.contains_key(id)
    }

    pub fn router(&self) -> &Arc<VfsRouter> {
        &self.router
    }
}

/// The real connector: FTP/FTPS over `suppaftp` with rustls and the system roots.
#[derive(Default)]
pub struct SuppaConnector {
    /// Trust anchors; `None` = the system's.
    pub roots: Option<Vec<rustls::pki_types::TrustAnchor<'static>>>,
}

impl Connector for SuppaConnector {
    type Transport = crate::suppa::SuppaTransport;
    fn connect(
        &self,
        params: &ConnectParams,
        password: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport> {
        crate::suppa::SuppaTransport::connect(params, password, verifier, self.roots.clone())
    }
}
