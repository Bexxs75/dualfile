//! Server connections on the Tauri side: the DTOs of the connection commands, the pin store
//! that follows the profiles, and the seam (`ConnectionBackend`) between the app state and the
//! connection manager of `dualfile-remote` (so tests can plug in fake servers).
//!
//! No password appears in any type here that is serialized, formatted or returned: the
//! secret only travels as `Secret` (`***` when formatted, overwritten when dropped).

use dualfile_core::connections::{
    ConnectionAuth, ConnectionProfile, ConnectionProtocol, ConnectionTls, PinEntry, PinProtocol,
    SshTransport,
};
use dualfile_core::VfsRouter;
use dualfile_remote::manager::{
    ConnectError, ConnectionInfo, ConnectionManager, Connector, ManagerConfig, SftpConnector,
};
use dualfile_remote::profile::{FtpProfile, SftpAuth, SftpProfile, TlsMode};
use dualfile_remote::secret::Secret;
use dualfile_remote::secret_store::{SecretVault, StoreKind};
use dualfile_remote::trust::{
    ConfirmTrust, Pin, PinKey, PinStore, Protocol, TrustDecision, TrustReason, TrustRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an open "trust this fingerprint?" question stays answerable.
pub const TRUST_QUESTION_TTL: Duration = Duration::from_secs(300);
/// Longest password / passphrase accepted.
pub const MAX_SECRET_BYTES: usize = 4096;

// ---------------------------------------------------------------- DTOs

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthDto {
    Password,
    KeyFile { path: String },
}

/// A profile as the UI edits it. There is deliberately no password field; the pins are not
/// part of it either (only the connection code writes them).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfileDto {
    pub id: String,
    pub name: String,
    pub protocol: ConnectionProtocol,
    #[serde(default)]
    pub transport: SshTransport,
    #[serde(default)]
    pub system_url: Option<String>,
    #[serde(default)]
    pub share: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub guest: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub start_folder: Option<String>,
    pub tls: ConnectionTls,
    pub allow_plaintext: bool,
    pub auth: AuthDto,
    pub save_secret: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinInfoDto {
    pub protocol: PinProtocol,
    pub host: String,
    pub port: u16,
    pub sha256: String,
    pub key_type: Option<String>,
}

/// Where a stored secret is, as the UI hears it (`none` = nothing stored).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretStoreDto {
    Keychain,
    Session,
    None,
}

impl From<Option<StoreKind>> for SecretStoreDto {
    fn from(kind: Option<StoreKind>) -> Self {
        match kind {
            Some(StoreKind::Keychain) => Self::Keychain,
            Some(StoreKind::Session) => Self::Session,
            None => Self::None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionEntryDto {
    pub profile: ConnectionProfileDto,
    pub pins: Vec<PinInfoDto>,
    pub connected: bool,
    pub connection_id: Option<String>,
    pub root: Option<String>,
    /// Only "set / not set", never the secret.
    pub secret_set: bool,
    pub secret_store: SecretStoreDto,
}

/// What a connect attempt came to. Failures other than these are errors of the command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConnectOutcomeDto {
    Connected {
        connection_id: String,
        root: String,
        secret_store: SecretStoreDto,
    },
    /// The user must decide about this fingerprint (`connection_trust`); nothing is connected.
    NeedsTrust {
        fingerprint: String,
        key_type: Option<String>,
        kind: &'static str,
        changed: bool,
    },
    /// The server presents another key than the pinned one: shown only, never connected.
    PinMismatch { old: String, new: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TrustDecisionDto {
    Once,
    Always,
}

impl From<ConnectionProfile> for ConnectionProfileDto {
    fn from(p: ConnectionProfile) -> Self {
        Self {
            id: p.id,
            name: p.name,
            protocol: p.protocol,
            transport: p.transport,
            system_url: p.system_url,
            share: p.share,
            domain: p.domain,
            guest: p.guest,
            host: p.host,
            port: p.port,
            user: p.user,
            start_folder: p.start_folder,
            tls: p.tls,
            allow_plaintext: p.allow_plaintext,
            auth: match p.auth {
                ConnectionAuth::Password => AuthDto::Password,
                ConnectionAuth::KeyFile { path } => AuthDto::KeyFile { path },
            },
            save_secret: p.save_secret,
        }
    }
}

impl ConnectionProfileDto {
    /// The profile as the settings keep it, without pins (the caller adds the ones that still
    /// apply).
    pub fn into_profile(self) -> ConnectionProfile {
        ConnectionProfile {
            id: self.id,
            name: self.name,
            protocol: self.protocol,
            transport: self.transport,
            system_url: self.system_url,
            share: self.share,
            domain: self.domain,
            guest: self.guest,
            host: self.host,
            port: self.port,
            user: self.user,
            start_folder: self.start_folder,
            tls: self.tls,
            allow_plaintext: self.allow_plaintext,
            auth: match self.auth {
                AuthDto::Password => ConnectionAuth::Password,
                AuthDto::KeyFile { path } => ConnectionAuth::KeyFile { path },
            },
            save_secret: self.save_secret,
            pins: Vec::new(),
        }
    }
}

pub fn pin_infos(profile: &ConnectionProfile) -> Vec<PinInfoDto> {
    profile
        .pins
        .iter()
        .map(|pin| PinInfoDto {
            protocol: pin.protocol,
            host: pin.host.clone(),
            port: pin.port,
            sha256: pin.sha256.clone(),
            key_type: pin.key_type.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------- Profiles -> remote

pub fn ftp_profile(p: &ConnectionProfile) -> FtpProfile {
    FtpProfile {
        name: p.name.clone(),
        host: p.host.clone(),
        port: p.port,
        user: p.user.clone(),
        tls: match p.tls {
            ConnectionTls::None => TlsMode::None,
            ConnectionTls::Explicit => TlsMode::Explicit,
            ConnectionTls::Implicit => TlsMode::Implicit,
        },
        allow_plaintext: p.allow_plaintext,
        start_folder: p.start_folder.clone(),
    }
}

pub fn sftp_profile(p: &ConnectionProfile) -> SftpProfile {
    SftpProfile {
        transport: p.transport,
        name: p.name.clone(),
        host: p.host.clone(),
        port: p.port,
        user: p.user.clone(),
        auth: match &p.auth {
            ConnectionAuth::Password => SftpAuth::Password,
            ConnectionAuth::KeyFile { path } => SftpAuth::KeyFile { path: path.clone() },
        },
        start_folder: p.start_folder.clone(),
    }
}

/// The pin protocol a profile's server is pinned under (plain FTP has no pin).
pub fn pin_protocol(p: &ConnectionProfile) -> Option<PinProtocol> {
    match p.protocol {
        ConnectionProtocol::System
        | ConnectionProtocol::Ftp
        | ConnectionProtocol::Smb
        | ConnectionProtocol::Webdav => None,
        ConnectionProtocol::Ftps => Some(PinProtocol::Ftps),
        ConnectionProtocol::Sftp => Some(PinProtocol::Sftp),
    }
}

fn remote_protocol(protocol: PinProtocol) -> Protocol {
    match protocol {
        PinProtocol::Ftps => Protocol::Ftps,
        PinProtocol::Sftp => Protocol::Sftp,
    }
}

fn core_protocol(protocol: Protocol) -> Option<PinProtocol> {
    match protocol {
        Protocol::Ftps => Some(PinProtocol::Ftps),
        Protocol::Sftp => Some(PinProtocol::Sftp),
        Protocol::Smb | Protocol::Webdav => None,
    }
}

pub fn pin_to_remote(pin: &PinEntry) -> Pin {
    Pin {
        protocol: remote_protocol(pin.protocol),
        host: pin.host.clone(),
        port: pin.port,
        sha256: pin.sha256.clone(),
        key_type: pin.key_type.clone(),
    }
}

pub fn pin_to_entry(pin: &Pin) -> Option<PinEntry> {
    Some(PinEntry {
        protocol: core_protocol(pin.protocol)?,
        host: pin.host.clone(),
        port: pin.port,
        sha256: pin.sha256.clone(),
        key_type: pin.key_type.clone(),
    })
}

// ---------------------------------------------------------------- Trust

/// The pins in force while connecting: filled from the profiles before each connect. A pin
/// the user chose to keep (`TrustAlways`) is also remembered in `added`, so the caller can
/// write it into the profile once the connection worked.
#[derive(Default)]
pub struct ProfilePins {
    pins: Mutex<HashMap<PinKey, Pin>>,
    added: Mutex<Vec<Pin>>,
}

impl ProfilePins {
    pub fn replace_all(&self, pins: Vec<Pin>) {
        let mut map = self.pins.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
        for pin in pins {
            map.insert(pin.key(), pin);
        }
        self.added.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// The pins set since the last `replace_all` / `take_added`.
    pub fn take_added(&self) -> Vec<Pin> {
        std::mem::take(&mut *self.added.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl PinStore for ProfilePins {
    fn get(&self, key: &PinKey) -> Option<Pin> {
        self.pins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    fn set(&self, pin: Pin) {
        self.added
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(pin.clone());
        self.pins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pin.key(), pin);
    }

    fn remove(&self, key: &PinKey) -> Option<Pin> {
        self.pins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
    }
}

/// Brings a fingerprint typed or shown by the user into the stored form (lower case hex,
/// no colons or blanks).
pub fn normalize_fingerprint(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The answer to the trust question while connecting: nothing is trusted unless the user
/// confirmed exactly this fingerprint beforehand.
pub struct DecisionConfirm {
    decision: Option<(TrustDecision, String)>,
}

impl DecisionConfirm {
    /// First attempt: every question is answered "no" (the request is reported to the UI).
    pub fn ask() -> Arc<Self> {
        Arc::new(Self { decision: None })
    }

    /// After the user decided: only the confirmed fingerprint is accepted.
    pub fn decided(decision: TrustDecision, fingerprint: String) -> Arc<Self> {
        Arc::new(Self {
            decision: Some((decision, fingerprint)),
        })
    }
}

impl ConfirmTrust for DecisionConfirm {
    fn confirm(&self, request: &TrustRequest) -> TrustDecision {
        match &self.decision {
            Some((decision, fingerprint)) if *fingerprint == request.sha256 => *decision,
            _ => TrustDecision::Reject,
        }
    }
}

/// The text of `needsTrust.kind` for a request.
pub fn trust_kind(request: &TrustRequest) -> &'static str {
    if request.key.protocol == Protocol::Sftp {
        "newHostKey"
    } else if request.reasons.contains(&TrustReason::HostnameMismatch) {
        "hostnameMismatch"
    } else {
        "unknownIssuer"
    }
}

/// The open question of one profile: which fingerprint the user was shown. A password typed
/// for this attempt is kept (and overwritten when dropped) so the answer can finish the
/// connect without asking for it again.
pub struct PendingTrust {
    pub fingerprint: String,
    pub secret: Option<Secret>,
    pub created: Instant,
}

// ---------------------------------------------------------------- Backend seam

/// What the app state needs from the connection manager.
pub trait ConnectionBackend: Send + Sync {
    fn connect_ftp(
        &self,
        profile: FtpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError>;
    fn connect_sftp(
        &self,
        profile: SftpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError>;
    fn connect_smb(
        &self,
        profile: dualfile_remote::smb::SmbProfile,
        secret: Secret,
    ) -> Result<ConnectionInfo, ConnectError>;
    fn connect_webdav(
        &self,
        profile: dualfile_remote::profile::WebdavProfile,
        secret: Secret,
    ) -> Result<ConnectionInfo, ConnectError>;
    fn disconnect(&self, id: &str) -> bool;
    fn disconnect_all(&self);
    fn is_connected(&self, id: &str) -> bool;
    fn is_live(&self, id: &str) -> bool;
}

impl<C: Connector, S: SftpConnector> ConnectionBackend for ConnectionManager<C, S> {
    fn connect_ftp(
        &self,
        profile: FtpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError> {
        self.connect(profile, secret, confirm)
    }
    fn connect_sftp(
        &self,
        profile: SftpProfile,
        secret: Secret,
        confirm: Arc<dyn ConfirmTrust>,
    ) -> Result<ConnectionInfo, ConnectError> {
        ConnectionManager::connect_sftp(self, profile, secret, confirm)
    }
    fn connect_smb(
        &self,
        profile: dualfile_remote::smb::SmbProfile,
        secret: Secret,
    ) -> Result<ConnectionInfo, ConnectError> {
        ConnectionManager::connect_smb(self, profile, secret)
    }
    fn connect_webdav(
        &self,
        profile: dualfile_remote::profile::WebdavProfile,
        secret: Secret,
    ) -> Result<ConnectionInfo, ConnectError> {
        ConnectionManager::connect_webdav(self, profile, secret)
    }
    fn disconnect(&self, id: &str) -> bool {
        ConnectionManager::disconnect(self, id)
    }
    fn disconnect_all(&self) {
        ConnectionManager::disconnect_all(self)
    }
    fn is_live(&self, id: &str) -> bool {
        ConnectionManager::is_live(self, id)
    }
    fn is_connected(&self, id: &str) -> bool {
        ConnectionManager::is_connected(self, id)
    }
}

type BackendFactory =
    Box<dyn FnOnce(Arc<VfsRouter>, Arc<dyn PinStore>) -> Arc<dyn ConnectionBackend> + Send>;

/// Everything about servers that differs between the app and the tests: the secret store and
/// the connection manager (real connectors or fake servers).
pub struct RemoteParts {
    pub vault: SecretVault,
    pub backend: BackendFactory,
}

impl RemoteParts {
    /// The real thing: the system keychain (session memory where it fails or is missing) and
    /// the FTP/FTPS and SFTP connectors.
    #[cfg(any(test, not(unix)))]
    pub fn production() -> Self {
        #[cfg(unix)]
        {
            Self::production_with_prompt(None)
        }
        #[cfg(not(unix))]
        {
            Self::production_builtin()
        }
    }

    #[cfg(unix)]
    pub fn production_with_prompt(
        handler: Option<Arc<dyn dualfile_remote::askpass::PromptHandler>>,
    ) -> Self {
        let mut openssh = dualfile_remote::openssh_transport::OpensshConnector::default();
        if let Some(handler) = handler {
            openssh = openssh.with_prompt_handler(handler);
        }
        Self {
            vault: SecretVault::new(crate::keychain::system_store()),
            backend: Box::new(move |router, pins| {
                Arc::new(ConnectionManager::with_sftp(
                    dualfile_remote::manager::SuppaConnector::default(),
                    dualfile_remote::select_connector::SelectConnector {
                        builtin: dualfile_remote::russh_transport::RusshConnector,
                        openssh,
                    },
                    router,
                    pins,
                    ManagerConfig::default(),
                ))
            }),
        }
    }

    #[cfg(not(unix))]
    fn production_builtin() -> Self {
        Self {
            vault: SecretVault::new(crate::keychain::system_store()),
            backend: Box::new(|router, pins| {
                Arc::new(ConnectionManager::with_sftp(
                    dualfile_remote::manager::SuppaConnector::default(),
                    dualfile_remote::russh_transport::RusshConnector,
                    router,
                    pins,
                    ManagerConfig::default(),
                ))
            }),
        }
    }

    /// Test seam: any connectors.
    #[cfg(test)]
    pub fn with_connectors<C: Connector, S: SftpConnector>(
        vault: SecretVault,
        ftp: C,
        sftp: S,
    ) -> Self {
        Self {
            vault,
            backend: Box::new(move |router, pins| {
                Arc::new(ConnectionManager::with_sftp(
                    ftp,
                    sftp,
                    router,
                    pins,
                    ManagerConfig::default(),
                ))
            }),
        }
    }
}

/// A connection that is open for a profile.
#[derive(Clone)]
pub struct ActiveConnection {
    pub info: ConnectionInfo,
}

/// The connection part of `AppState`.
pub struct ConnectionState {
    pub backend: Arc<dyn ConnectionBackend>,
    pub vault: SecretVault,
    pub pins: Arc<ProfilePins>,
    /// profile id -> open connection
    pub active: Mutex<HashMap<String, ActiveConnection>>,
    /// profile id -> the fingerprint the user is being asked about
    pub pending: Mutex<HashMap<String, PendingTrust>>,
    /// Profiles being connected right now (one attempt at a time per profile).
    pub connecting: Mutex<HashSet<String>>,
    /// Read-modify-write of the profile list in the settings file.
    pub settings_lock: Mutex<()>,
}

impl ConnectionState {
    pub fn new(parts: RemoteParts, router: Arc<VfsRouter>) -> Self {
        let pins = Arc::new(ProfilePins::default());
        let store: Arc<dyn PinStore> = pins.clone();
        Self {
            backend: (parts.backend)(router, store),
            vault: parts.vault,
            pins,
            active: Mutex::default(),
            pending: Mutex::default(),
            connecting: Mutex::default(),
            settings_lock: Mutex::new(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_compared_in_one_form() {
        assert_eq!(normalize_fingerprint("AB:cd 12"), "abcd12");
    }

    #[test]
    fn the_confirm_answers_only_for_the_confirmed_fingerprint() {
        let request = TrustRequest {
            key: PinKey::new(Protocol::Ftps, "h", 21),
            sha256: "ab".repeat(32),
            key_type: None,
            reasons: vec![TrustReason::UnknownIssuer],
        };
        assert_eq!(
            DecisionConfirm::ask().confirm(&request),
            TrustDecision::Reject
        );
        assert_eq!(
            DecisionConfirm::decided(TrustDecision::TrustAlways, "ab".repeat(32)).confirm(&request),
            TrustDecision::TrustAlways
        );
        assert_eq!(
            DecisionConfirm::decided(TrustDecision::TrustAlways, "cd".repeat(32)).confirm(&request),
            TrustDecision::Reject
        );
        assert_eq!(trust_kind(&request), "unknownIssuer");
    }

    #[test]
    fn pins_set_by_the_user_are_reported_once() {
        let pins = ProfilePins::default();
        let pin = Pin {
            protocol: Protocol::Sftp,
            host: "h".into(),
            port: 22,
            sha256: "ab".repeat(32),
            key_type: Some("ssh-ed25519".into()),
        };
        pins.set(pin.clone());
        assert_eq!(pins.get(&pin.key()), Some(pin.clone()));
        assert_eq!(pins.take_added(), vec![pin.clone()]);
        assert!(pins.take_added().is_empty());
        pins.replace_all(vec![]);
        assert!(pins.get(&pin.key()).is_none());
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use dualfile_remote::error::{TResult, TransportError};
    use dualfile_remote::profile::SftpConnectParams;
    use dualfile_remote::trust::CertVerifier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count(Arc<AtomicUsize>);
    impl SftpConnector for Count {
        type Transport = dualfile_remote::sftp_fake::FakeSftpTransport;
        fn connect(
            &self,
            _: &SftpConnectParams,
            _: &Secret,
            _: Arc<dyn CertVerifier>,
        ) -> TResult<Self::Transport> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(TransportError::Auth)
        }
    }
    #[test]
    fn backend_routes_profiles_to_the_selected_fake_connector() {
        let builtin = Arc::new(AtomicUsize::new(0));
        let openssh = Arc::new(AtomicUsize::new(0));
        let parts = RemoteParts::with_connectors(
            SecretVault::new(Box::new(dualfile_remote::secret_store::SessionOnly::new())),
            dualfile_remote::manager::SuppaConnector::default(),
            dualfile_remote::select_connector::SelectConnector {
                builtin: Count(builtin.clone()),
                openssh: Count(openssh.clone()),
            },
        );
        let backend = (parts.backend)(Arc::new(VfsRouter::new()), Arc::new(ProfilePins::default()));
        let mut p = ConnectionProfile {
            id: "p1".into(),
            name: "n".into(),
            protocol: ConnectionProtocol::Sftp,
            tls: ConnectionTls::None,
            host: "nas".into(),
            user: "u".into(),
            port: 22,
            ..Default::default()
        };
        assert!(backend
            .connect_sftp(sftp_profile(&p), Secret::new(""), DecisionConfirm::ask())
            .is_err());
        assert_eq!(builtin.load(Ordering::SeqCst), 1);
        assert_eq!(openssh.load(Ordering::SeqCst), 0);
        p.transport = SshTransport::Openssh;
        p.user.clear();
        p.port = 0;
        assert!(backend
            .connect_sftp(sftp_profile(&p), Secret::new(""), DecisionConfirm::ask())
            .is_err());
        assert_eq!(builtin.load(Ordering::SeqCst), 1);
        assert_eq!(openssh.load(Ordering::SeqCst), 1);
    }
}
