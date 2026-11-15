//! Commands for server connections: profiles, secrets, connect / trust / disconnect. Part of
//! `AppState`; DTOs and helpers live in `crate::connections`.
//!
//! Rules kept here: a password only enters through `connection_connect` / `secret_set` (as a
//! `Secret`), is never put into settings, events, errors or answers, and nothing connects
//! unless the user asked for it by command.

use super::{lock, AppState};
use crate::connections::{
    ftp_profile, normalize_fingerprint, pin_infos, pin_protocol, pin_to_entry, pin_to_remote,
    sftp_profile, ActiveConnection, ConnectOutcomeDto, ConnectionEntryDto, ConnectionProfileDto,
    DecisionConfirm, PendingTrust, SecretStoreDto, TrustDecisionDto, MAX_SECRET_BYTES,
    TRUST_QUESTION_TTL,
};
use crate::convert::{connect_error, vpath_text, UiError};
use dualfile_core::connections::{
    new_profile_id, parse_profile_path, validate_connections, ConnectionProfile,
    ConnectionProtocol, ConnectionTls, SshTransport,
};
use dualfile_core::settings::{self, LoadOutcome, Settings};
use dualfile_core::vfs::VPath;
use dualfile_core::Error;
use dualfile_remote::manager::{ConnectError, ConnectionInfo};
use dualfile_remote::secret::Secret;
use dualfile_remote::secret_store::{secret_key, StoreKind};
use dualfile_remote::trust::{Protocol, TrustDecision, TrustError};
use std::time::Instant;

fn not_found_profile() -> UiError {
    UiError::new("notFound", "Das Verbindungsprofil gibt es nicht")
}

/// The keychain would not give up a stored secret. Fixed text: nothing of the secret or of the
/// keychain's own error goes in.
fn secret_removal_failed() -> UiError {
    UiError::new(
        "secretRemovalFailed",
        "Das gespeicherte Passwort konnte nicht aus dem Schlüsselbund entfernt werden",
    )
}

fn invalid(message: &str) -> UiError {
    UiError::new("invalidInput", message)
}

/// Frees the "connecting" mark of a profile when the attempt ends, however it ends.
struct ConnectingGuard<'a> {
    state: &'a AppState,
    id: String,
}

impl Drop for ConnectingGuard<'_> {
    fn drop(&mut self) {
        lock(&self.state.connections.connecting).remove(&self.id);
    }
}

impl AppState {
    /// The settings as stored; a file of a newer version is never touched.
    pub(super) fn load_for_update(&self) -> Result<Settings, UiError> {
        let (loaded, outcome) = settings::load(&self.config_dir);
        if let LoadOutcome::NewerVersion(_) = outcome {
            return Err(UiError::new(
                "unsupported",
                "Die Einstellungen stammen aus einer neueren Version und werden nicht verändert",
            ));
        }
        Ok(loaded)
    }

    fn profile(&self, id: &str) -> Result<ConnectionProfile, UiError> {
        let (loaded, _) = settings::load(&self.config_dir);
        loaded
            .connections
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(not_found_profile)
    }

    /// `conn:<profile id>!<path>` -> the location on the profile's open connection.
    pub(super) fn resolve_profile_path(&self, raw: &str) -> Result<VPath, UiError> {
        let (profile_id, path) = parse_profile_path(raw)?;
        let info = lock(&self.connections.active)
            .get(&profile_id)
            .map(|active| active.info.clone());
        let Some(info) = info else {
            return Err(Error::Disconnected.into());
        };
        Ok(match info.protocol {
            Protocol::Smb => VPath::smb(&info.id, &path)?,
            Protocol::Webdav => VPath::webdav(&info.id, &path)?,
            Protocol::Sftp => VPath::sftp(&info.id, &path)?,
            Protocol::Ftps => VPath::ftp(&info.id, &path)?,
        })
    }

    /// Forgets connections the manager no longer has.
    fn prune_active(&self) {
        let backend = &self.connections.backend;
        lock(&self.connections.active).retain(|_, active| backend.is_connected(&active.info.id));
    }

    fn entry_for(&self, profile: &ConnectionProfile) -> ConnectionEntryDto {
        let active = lock(&self.connections.active).get(&profile.id).cloned();
        let mounted = if profile.protocol == ConnectionProtocol::System {
            profile
                .system_url
                .as_deref()
                .and_then(|url| dualfile_core::system_places::parse_url(url, "", ""))
                .and_then(|place| {
                    dualfile_core::system_places::mounted_path(
                        &place,
                        &dualfile_core::system_places::DiscoveryPaths::system(),
                    )
                })
                .map(|path| path.to_string_lossy().into_owned())
        } else {
            None
        };
        let key = secret_key(&profile.id);
        // The keychain is only asked for profiles that want a stored secret (asking can make
        // the system prompt for its password).
        let kind = if profile.protocol == ConnectionProtocol::System
            || profile.transport == SshTransport::Openssh
        {
            None
        } else {
            self.connections.vault.kind(&key, profile.save_secret)
        };
        ConnectionEntryDto {
            profile: ConnectionProfileDto::from(ConnectionProfile {
                pins: Vec::new(),
                ..profile.clone()
            }),
            pins: pin_infos(profile),
            connected: mounted.is_some()
                || active
                    .as_ref()
                    .is_some_and(|a| self.connections.backend.is_live(&a.info.id)),
            connection_id: active.as_ref().map(|a| a.info.id.clone()),
            root: mounted.or_else(|| active.as_ref().map(|a| vpath_text(&a.info.root))),
            secret_set: kind.is_some(),
            secret_store: SecretStoreDto::from(kind),
        }
    }

    pub fn connections_list(&self) -> Vec<ConnectionEntryDto> {
        self.prune_active();
        let (loaded, _) = settings::load(&self.config_dir);
        loaded
            .connections
            .iter()
            .map(|profile| self.entry_for(profile))
            .collect()
    }

    /// Creates (empty id) or changes a profile. Pins stay as they are as long as the server
    /// (protocol, host, port) is the same; the UI cannot set them.
    pub fn connection_save(
        &self,
        dto: ConnectionProfileDto,
    ) -> Result<ConnectionEntryDto, UiError> {
        let _guard = lock(&self.connections.settings_lock);
        let mut current = self.load_for_update()?;
        let mut profile = dto.into_profile();
        if lock(&self.connections.connecting).contains(&profile.id) {
            return Err(invalid("Die Verbindung wird gerade aufgebaut"));
        }
        let existing = if profile.id.is_empty() {
            profile.id = new_profile_id(&current.connections);
            None
        } else {
            let found = current.connections.iter().position(|p| p.id == profile.id);
            Some(found.ok_or_else(not_found_profile)?)
        };
        let transport_changed =
            existing.is_some_and(|i| current.connections[i].transport != profile.transport);
        if let Some(index) = existing {
            let same_server = |old: &ConnectionProfile| {
                old.host.eq_ignore_ascii_case(&profile.host)
                    && old.port == profile.port
                    && pin_protocol(old) == pin_protocol(&profile)
            };
            let old = &current.connections[index];
            if same_server(old)
                || (old.protocol == ConnectionProtocol::Sftp
                    && profile.protocol == ConnectionProtocol::Sftp
                    && (old.transport == SshTransport::Openssh
                        || profile.transport == SshTransport::Openssh))
            {
                profile.pins = old.pins.clone();
            }
        }
        profile.validate().map_err(invalid)?;
        // "Save the password" switched off: the stored one goes too, before the profile is
        // written. If the keychain refuses, the profile stays as it was (still "save"), so it
        // never claims a removal that did not happen.
        let turns_off =
            existing.is_some_and(|i| current.connections[i].save_secret) && !profile.save_secret;
        if turns_off && !self.connections.vault.delete(&secret_key(&profile.id)) {
            return Err(secret_removal_failed());
        }
        match existing {
            Some(index) => current.connections[index] = profile.clone(),
            None => current.connections.push(profile.clone()),
        }
        validate_connections(&current.connections).map_err(invalid)?;
        settings::save(&self.config_dir, &current, false)?;
        if transport_changed {
            self.drop_connection(&profile.id);
        }
        // The profile changed: an open question about its old fingerprint is void.
        lock(&self.connections.pending).remove(&profile.id);
        Ok(self.entry_for(&profile))
    }

    /// Removes a profile together with its connection, its open trust question and its stored
    /// secret.
    pub fn connection_delete(&self, profile_id: &str) -> Result<(), UiError> {
        let _guard = lock(&self.connections.settings_lock);
        let mut current = self.load_for_update()?;
        let Some(existing) = current.connections.iter().find(|p| p.id == profile_id) else {
            return Err(not_found_profile());
        };
        let had_saved_secret = existing.save_secret;
        if lock(&self.connections.connecting).contains(profile_id) {
            return Err(invalid("Die Verbindung wird gerade aufgebaut"));
        }
        self.drop_connection(profile_id);
        lock(&self.connections.pending).remove(profile_id);
        // Keep the profile available for a retry if the keychain refuses removal. A profile
        // that never saved a secret has nothing in the keychain to lose, so an unavailable
        // keychain (no Secret Service) must not block deleting it.
        let removed = existing.protocol == ConnectionProtocol::System
            || self.connections.vault.delete(&secret_key(profile_id));
        if !removed && had_saved_secret {
            return Err(secret_removal_failed());
        }
        current.connections.retain(|p| p.id != profile_id);
        settings::save(&self.config_dir, &current, false)?;
        Ok(())
    }

    fn drop_connection(&self, profile_id: &str) {
        let active = lock(&self.connections.active).remove(profile_id);
        if let Some(active) = active {
            self.connections.backend.disconnect(&active.info.id);
        }
    }

    pub fn connection_disconnect(&self, profile_id: &str) -> Result<(), UiError> {
        self.profile(profile_id)?;
        self.drop_connection(profile_id);
        lock(&self.connections.pending).remove(profile_id);
        Ok(())
    }

    /// Forgets the pinned fingerprints of the profile's server (every profile with the same
    /// server loses them, so the pin really is gone). A deliberate step of its own.
    pub fn connection_reset_trust(&self, profile_id: &str) -> Result<(), UiError> {
        let _guard = lock(&self.connections.settings_lock);
        let mut current = self.load_for_update()?;
        let target = current
            .connections
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
            .ok_or_else(not_found_profile)?;
        if target.transport == SshTransport::Openssh {
            return Err(invalid("OpenSSH verwaltet known_hosts selbst"));
        }
        for profile in &mut current.connections {
            if pin_protocol(profile) == pin_protocol(&target) {
                profile.pins.retain(|pin| {
                    !(pin.host.eq_ignore_ascii_case(&target.host) && pin.port == target.port)
                });
            }
        }
        settings::save(&self.config_dir, &current, false)?;
        lock(&self.connections.pending).remove(profile_id);
        Ok(())
    }

    fn secret_from(text: String) -> Result<Secret, UiError> {
        if text.len() > MAX_SECRET_BYTES {
            return Err(invalid("Das Passwort ist zu lang"));
        }
        Ok(Secret::new(text))
    }

    pub fn secret_set(&self, profile_id: &str, secret: String) -> Result<SecretStoreDto, UiError> {
        let profile = self.profile(profile_id)?;
        if profile.protocol == ConnectionProtocol::System
            || profile.transport == SshTransport::Openssh
        {
            return Err(invalid("OpenSSH speichert keine Antworten"));
        }
        let secret = Self::secret_from(secret)?;
        let kind = self.connections.vault.set(&secret_key(profile_id), &secret);
        Ok(SecretStoreDto::from(Some(kind)))
    }

    pub fn secret_clear(&self, profile_id: &str) -> Result<(), UiError> {
        if self.profile(profile_id)?.protocol == ConnectionProtocol::System {
            return Ok(());
        }
        if self.connections.vault.delete(&secret_key(profile_id)) {
            Ok(())
        } else {
            Err(secret_removal_failed())
        }
    }

    /// Connects a profile. `secret` is the password (or key passphrase) typed for this
    /// attempt; without it the stored one is used (keychain or session), else none. The
    /// result is one of the states of `ConnectOutcomeDto`; other failures are errors.
    pub fn connection_connect(
        &self,
        profile_id: &str,
        secret: Option<String>,
    ) -> Result<ConnectOutcomeDto, UiError> {
        if self.profile(profile_id)?.protocol == ConnectionProtocol::System {
            let root = self.system_mount_connect(profile_id)?;
            return Ok(ConnectOutcomeDto::Connected {
                connection_id: profile_id.into(),
                root,
                secret_store: SecretStoreDto::None,
            });
        }
        let provided = secret.map(Self::secret_from).transpose()?;
        // A new attempt ends an older open question of this profile.
        lock(&self.connections.pending).remove(profile_id);
        self.connect_with(profile_id, provided, DecisionConfirm::ask())
    }

    /// Answers the trust question of `connection_connect`: only the fingerprint that was
    /// reported is accepted (another one ends the question), then the connect is finished.
    pub fn connection_trust(
        &self,
        profile_id: &str,
        decision: TrustDecisionDto,
        fingerprint: &str,
    ) -> Result<ConnectOutcomeDto, UiError> {
        self.profile(profile_id)?;
        let pending = lock(&self.connections.pending)
            .remove(profile_id)
            .ok_or_else(|| invalid("Es gibt keine offene Rückfrage zu diesem Profil"))?;
        if pending.created.elapsed() > TRUST_QUESTION_TTL {
            return Err(invalid("Die Rückfrage ist abgelaufen; bitte neu verbinden"));
        }
        let given = normalize_fingerprint(fingerprint);
        if given != pending.fingerprint {
            return Err(invalid(
                "Der Fingerabdruck passt nicht zur Rückfrage; bitte neu verbinden",
            ));
        }
        let decision = match decision {
            TrustDecisionDto::Once => TrustDecision::TrustOnce,
            TrustDecisionDto::Always => TrustDecision::TrustAlways,
        };
        self.connect_with(
            profile_id,
            pending.secret,
            DecisionConfirm::decided(decision, given),
        )
    }

    fn connect_with(
        &self,
        profile_id: &str,
        provided: Option<Secret>,
        confirm: std::sync::Arc<DecisionConfirm>,
    ) -> Result<ConnectOutcomeDto, UiError> {
        let settings_guard = lock(&self.connections.settings_lock);
        let profile = self.profile(profile_id)?;
        if cfg!(not(unix)) && profile.transport == SshTransport::Openssh {
            return Err(UiError::new(
                "opensshMissing",
                "OpenSSH ist auf dieser Plattform nicht verfügbar",
            ));
        }
        self.prune_active();
        let stale = lock(&self.connections.active)
            .get(profile_id)
            .filter(|a| !self.connections.backend.is_live(&a.info.id))
            .map(|a| a.info.id.clone());
        if let Some(id) = stale {
            self.connections.backend.disconnect(&id);
            self.prune_active();
        }
        if let Some(active) = lock(&self.connections.active).get(profile_id) {
            return Ok(ConnectOutcomeDto::Connected {
                connection_id: active.info.id.clone(),
                root: vpath_text(&active.info.root),
                secret_store: SecretStoreDto::from(
                    self.connections.vault.kind(&secret_key(profile_id), false),
                ),
            });
        }
        if !lock(&self.connections.connecting).insert(profile_id.to_owned()) {
            return Err(invalid("Die Verbindung wird gerade aufgebaut"));
        }
        let _guard = ConnectingGuard {
            state: self,
            id: profile_id.to_owned(),
        };
        profile.validate().map_err(invalid)?;
        drop(settings_guard);
        let provided = if profile.transport == SshTransport::Openssh {
            None
        } else {
            provided
        };
        let key = secret_key(profile_id);
        let secret = provided
            .clone()
            // a stored secret is only used while the profile is set to keep one
            .or_else(|| {
                profile
                    .save_secret
                    .then(|| self.connections.vault.get(&key))
                    .flatten()
            })
            .unwrap_or_else(|| Secret::new(""));
        // The pins of all profiles are in force; a profile never connects to a server that
        // presents another key than the pinned one.
        if profile.transport != SshTransport::Openssh {
            let (all, _) = settings::load(&self.config_dir);
            self.connections.pins.replace_all(
                all.connections
                    .iter()
                    .flat_map(|p| p.pins.iter().map(pin_to_remote))
                    .collect(),
            );
        }
        let confirm: std::sync::Arc<dyn dualfile_remote::trust::ConfirmTrust> = confirm;
        let result = match profile.protocol {
            ConnectionProtocol::System => {
                return Err(invalid(
                    "System-Einbindung verwendet keinen nativen Transport",
                ))
            }
            ConnectionProtocol::Smb => self.connections.backend.connect_smb(
                dualfile_remote::smb::SmbProfile {
                    name: profile.name.clone(),
                    host: profile.host.clone(),
                    port: profile.port,
                    user: profile.user.clone(),
                    domain: profile.domain.clone(),
                    share: profile.share.clone(),
                    guest: profile.guest,
                    start_folder: profile.start_folder.clone(),
                },
                secret,
            ),
            ConnectionProtocol::Webdav => self.connections.backend.connect_webdav(
                dualfile_remote::profile::WebdavProfile {
                    name: profile.name.clone(),
                    host: profile.host.clone(),
                    port: profile.port,
                    user: profile.user.clone(),
                    base_path: profile.share.clone(),
                    tls: profile.tls != ConnectionTls::None,
                    allow_plaintext: profile.allow_plaintext,
                    start_folder: profile.start_folder.clone(),
                },
                secret,
                confirm,
            ),
            ConnectionProtocol::Sftp => {
                self.connections
                    .backend
                    .connect_sftp(sftp_profile(&profile), secret, confirm)
            }
            _ => self
                .connections
                .backend
                .connect_ftp(ftp_profile(&profile), secret, confirm),
        };
        match result {
            Ok(info) => self.connected(&profile, info, provided),
            Err(error) => self.connect_failed(profile_id, error, provided),
        }
    }

    fn connected(
        &self,
        profile: &ConnectionProfile,
        info: ConnectionInfo,
        provided: Option<Secret>,
    ) -> Result<ConnectOutcomeDto, UiError> {
        // Fingerprints the user chose to keep go into the profile now. If that cannot be
        // written the connection is closed again: "always" must not silently become "once".
        let added = if profile.transport == SshTransport::Openssh {
            Vec::new()
        } else {
            self.connections.pins.take_added()
        };
        if !added.is_empty() {
            if let Err(error) = self.store_pins(&profile.id, &added) {
                self.connections.backend.disconnect(&info.id);
                return Err(error);
            }
        }
        let key = secret_key(&profile.id);
        let kind: Option<StoreKind> = if profile.transport == SshTransport::Openssh {
            None
        } else {
            match provided {
                Some(secret) if profile.save_secret => {
                    Some(self.connections.vault.set(&key, &secret))
                }
                _ => self.connections.vault.kind(&key, profile.save_secret),
            }
        };
        let outcome = ConnectOutcomeDto::Connected {
            connection_id: info.id.clone(),
            root: vpath_text(&info.root),
            secret_store: SecretStoreDto::from(kind),
        };
        lock(&self.connections.active).insert(profile.id.clone(), ActiveConnection { info });
        Ok(outcome)
    }

    fn store_pins(
        &self,
        profile_id: &str,
        added: &[dualfile_remote::trust::Pin],
    ) -> Result<(), UiError> {
        let _guard = lock(&self.connections.settings_lock);
        let mut current = self.load_for_update()?;
        let profile = current
            .connections
            .iter_mut()
            .find(|p| p.id == profile_id)
            .ok_or_else(not_found_profile)?;
        for pin in added {
            let Some(entry) = pin_to_entry(pin) else {
                continue;
            };
            profile.pins.retain(|old| {
                !(old.protocol == entry.protocol
                    && old.host.eq_ignore_ascii_case(&entry.host)
                    && old.port == entry.port)
            });
            profile.pins.push(entry);
        }
        profile.validate().map_err(invalid)?;
        settings::save(&self.config_dir, &current, false)?;
        Ok(())
    }

    fn connect_failed(
        &self,
        profile_id: &str,
        error: ConnectError,
        provided: Option<Secret>,
    ) -> Result<ConnectOutcomeDto, UiError> {
        match error.trust_error() {
            Some(TrustError::NotConfirmed(request)) => {
                // Remember exactly what the user is shown; only this fingerprint can be
                // confirmed. The typed password waits (in memory) for the answer.
                lock(&self.connections.pending).insert(
                    profile_id.to_owned(),
                    PendingTrust {
                        fingerprint: request.sha256.clone(),
                        secret: provided,
                        created: Instant::now(),
                    },
                );
                Ok(ConnectOutcomeDto::NeedsTrust {
                    fingerprint: request.sha256.clone(),
                    key_type: request.key_type.clone(),
                    kind: crate::connections::trust_kind(request),
                    changed: false,
                })
            }
            Some(TrustError::PinMismatch { expected, actual }) => {
                Ok(ConnectOutcomeDto::PinMismatch {
                    old: expected.clone(),
                    new: actual.clone(),
                })
            }
            _ => Err(connect_error(&error)),
        }
    }

    /// App exit: close the connections and drop the secrets held in memory.
    pub(super) fn close_connections(&self) {
        self.connections.backend.disconnect_all();
        lock(&self.connections.active).clear();
        lock(&self.connections.pending).clear();
        self.connections.vault.clear_session();
    }
}

impl AppState {
    pub fn system_places_import(
        &self,
        urls: Vec<String>,
    ) -> Result<Vec<ConnectionEntryDto>, UiError> {
        use dualfile_core::system_places::{discover, DiscoveryPaths};
        let discovered = discover(&DiscoveryPaths::system());
        let _guard = lock(&self.connections.settings_lock);
        let mut settings = self.load_for_update()?;
        settings.connections =
            dualfile_core::system_places::import_places(&settings.connections, &discovered, &urls)
                .map_err(invalid)?;
        settings::save(&self.config_dir, &settings, false)?;
        Ok(settings
            .connections
            .iter()
            .map(|p| self.entry_for(p))
            .collect())
    }
    pub fn system_mount_connect(&self, id: &str) -> Result<String, UiError> {
        let profile = self.profile(id)?;
        if profile.protocol != ConnectionProtocol::System {
            return Err(invalid("Keine System-Einbindung"));
        }
        let url = profile
            .system_url
            .ok_or_else(|| invalid("System-URL fehlt"))?;
        dualfile_remote::system_mount::connect(
            &url,
            &dualfile_core::system_places::DiscoveryPaths::system(),
        )
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(crate::convert::system_mount_error)
    }
    pub fn system_mount_unmount(&self, id: &str) -> Result<(), UiError> {
        let profile = self.profile(id)?;
        if profile.protocol != ConnectionProtocol::System {
            return Err(invalid("Keine System-Einbindung"));
        }
        dualfile_remote::system_mount::unmount(
            profile
                .system_url
                .as_deref()
                .ok_or_else(|| invalid("System-URL fehlt"))?,
        )
        .map_err(crate::convert::system_mount_error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AppState, EventSink};
    use crate::connections::{
        AuthDto, ConnectOutcomeDto, ConnectionProfileDto, RemoteParts, SecretStoreDto,
        TrustDecisionDto,
    };
    use crate::convert::{CoreEventDto, DeletePlanInput, UiError};
    use dualfile_core::connections::{ConnectionProtocol, ConnectionTls, SshTransport};
    use dualfile_core::jobs::JobState;
    use dualfile_core::settings::{self, LoadOutcome};
    use dualfile_remote::fake::{FakeConnector, FakeServer};
    use dualfile_remote::secret_store::{FakeKeychain, SecretVault};
    use dualfile_remote::sftp_fake::{FakeSftpConnector, FakeSftpServer, HostKey};
    use dualfile_remote::trust::{CertObservation, ChainStatus};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    const PW: &str = "marker-PW-4711";

    struct ChannelSink(Mutex<Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    struct Fx {
        state: AppState,
        events: Receiver<CoreEventDto>,
        root: tempfile::TempDir,
        ftp: FakeServer,
        sftp: FakeSftpServer,
        keychain: Arc<FakeKeychain>,
    }

    fn fixture() -> Fx {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("home")).unwrap();
        let ftp = FakeServer::new();
        ftp.state().password = PW.into();
        ftp.state().cert = Some(CertObservation {
            sha256: [7; 32],
            chain: ChainStatus::UnknownIssuer,
            hostname_ok: true,
            key_type: None,
        });
        let sftp = FakeSftpServer::new();
        sftp.state().password = PW.into();
        let keychain = FakeKeychain::new();
        let (tx, events) = channel();
        let state = AppState::with_remote(
            Arc::new(ChannelSink(Mutex::new(tx))),
            root.path().join("home"),
            root.path().join("config"),
            RemoteParts::with_connectors(
                SecretVault::new(Box::new(keychain.clone())),
                FakeConnector {
                    server: ftp.clone(),
                },
                FakeSftpConnector {
                    server: sftp.clone(),
                },
            ),
        );
        Fx {
            state,
            events,
            root,
            ftp,
            sftp,
            keychain,
        }
    }

    fn dto(name: &str, protocol: ConnectionProtocol) -> ConnectionProfileDto {
        ConnectionProfileDto {
            transport: SshTransport::Builtin,
            system_url: None,
            share: String::new(),
            domain: String::new(),
            guest: false,
            id: String::new(),
            name: name.into(),
            protocol,
            host: "files.example.org".into(),
            port: if protocol == ConnectionProtocol::Sftp {
                22
            } else {
                21
            },
            user: "user".into(),
            start_folder: None,
            tls: if protocol == ConnectionProtocol::Ftps {
                ConnectionTls::Explicit
            } else {
                ConnectionTls::None
            },
            allow_plaintext: false,
            auth: AuthDto::Password,
            save_secret: false,
        }
    }

    impl Fx {
        fn save(&self, d: ConnectionProfileDto) -> String {
            self.state.connection_save(d).unwrap().profile.id
        }
        fn settings_text(&self) -> String {
            std::fs::read_to_string(self.root.path().join("config/settings.json"))
                .unwrap_or_default()
        }
        /// FTPS profile, connected (trust given).
        fn connect_ftps(&self) -> (String, String) {
            let id = self.save(dto("FTPS", ConnectionProtocol::Ftps));
            let fingerprint = match self.state.connection_connect(&id, Some(PW.into())).unwrap() {
                ConnectOutcomeDto::NeedsTrust { fingerprint, .. } => fingerprint,
                other => panic!("{other:?}"),
            };
            match self
                .state
                .connection_trust(&id, TrustDecisionDto::Always, &fingerprint)
                .unwrap()
            {
                ConnectOutcomeDto::Connected { root, .. } => (id, root),
                other => panic!("{other:?}"),
            }
        }
        fn local(&self, rel: &str) -> String {
            self.root.path().join(rel).to_string_lossy().into_owned()
        }
        fn wait(&self, job: u64) -> JobState {
            self.state.jobs.wait(job).unwrap().state
        }
    }

    fn kind(result: Result<impl std::fmt::Debug, UiError>) -> String {
        result.unwrap_err().kind
    }

    #[test]
    fn system_profile_dto_roundtrip_disconnect_and_secret_rejection() {
        use crate::connections::ConnectionProfileDto;
        use dualfile_core::system_places::parse_url;
        let f = fixture();
        let profile = parse_url("nfs://nas/team", "dolphin", "Team")
            .unwrap()
            .profile();
        let dto = ConnectionProfileDto::from(profile.clone());
        assert_eq!(dto.clone().into_profile(), profile);
        let id = f.save(dto);
        let entry = f
            .state
            .connections_list()
            .into_iter()
            .find(|e| e.profile.id == id)
            .unwrap();
        assert_eq!(entry.profile.system_url.as_deref(), Some("nfs://nas/team"));
        assert!(!entry.secret_set);
        assert!(f.state.secret_set(&id, "secret".into()).is_err());
        f.state.connection_disconnect(&id).unwrap();
        assert!(f.keychain.keys().is_empty());
        f.state.connection_delete(&id).unwrap();
    }

    #[test]
    fn listing_loss_updates_status_and_explicit_reconnect_replaces_dead_session() {
        let f = fixture();
        let (id, root) = f.connect_ftps();
        let old = f.state.connections_list()[0].connection_id.clone();
        f.ftp.disconnect_everybody();
        f.ftp.state().refuse_connects_after = Some(0);
        assert!(f.state.list(&root).is_err());
        let entry = &f.state.connections_list()[0];
        assert!(!entry.connected);
        assert_eq!(entry.connection_id, old);
        f.ftp.state().refuse_connects_after = None;
        let outcome = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        assert!(matches!(outcome, ConnectOutcomeDto::Connected { .. }));
        let entry = &f.state.connections_list()[0];
        assert!(entry.connected);
        assert_ne!(entry.connection_id, old);
    }

    #[test]
    fn old_settings_migrate_and_openssh_keeps_pins_without_secrets() {
        let f = fixture();
        let mut p = dto("SSH", ConnectionProtocol::Sftp);
        let id = f.save(p.clone());
        let mut settings = settings::load(&f.state.config_dir).0;
        settings.connections[0]
            .pins
            .push(dualfile_core::connections::PinEntry {
                protocol: dualfile_core::connections::PinProtocol::Sftp,
                host: p.host.clone(),
                port: p.port,
                sha256: "ab".repeat(32),
                key_type: Some("ssh-ed25519".into()),
            });
        let mut old = serde_json::to_value(&settings).unwrap();
        old["version"] = serde_json::json!(3);
        old["connections"][0]
            .as_object_mut()
            .unwrap()
            .remove("transport");
        std::fs::write(
            f.state.config_dir.join("settings.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        let (loaded, outcome) = settings::load(&f.state.config_dir);
        assert_eq!(outcome, LoadOutcome::Migrated { from: 3, to: 6 });
        assert_eq!(loaded.connections[0].transport, SshTransport::Builtin);
        p.id = id.clone();
        p.transport = SshTransport::Openssh;
        p.host = "nas".into();
        p.user.clear();
        p.port = 0;
        f.state.connection_save(p).unwrap();
        assert_eq!(
            settings::load(&f.state.config_dir).0.connections[0].pins,
            loaded.connections[0].pins
        );
        assert!(f.state.secret_set(&id, "must-not-save".into()).is_err());
        assert!(f.state.connection_reset_trust(&id).is_err());
        assert!(!f.settings_text().contains("must-not-save"));
    }

    #[test]
    fn profiles_are_saved_listed_validated_and_deleted() {
        let f = fixture();
        assert!(f.state.connections_list().is_empty());
        let id = f.save(dto("Server", ConnectionProtocol::Ftps));
        let list = f.state.connections_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].profile.id, id);
        assert!(!list[0].connected && !list[0].secret_set);
        // a duplicate name (any case) and bad values are refused, nothing is stored
        let mut dup = dto("server", ConnectionProtocol::Ftps);
        assert_eq!(kind(f.state.connection_save(dup.clone())), "invalidInput");
        dup.name = "Anderer".into();
        dup.host = "a b".into();
        assert_eq!(kind(f.state.connection_save(dup)), "invalidInput");
        let mut plain = dto("Klartext", ConnectionProtocol::Ftp);
        plain.tls = ConnectionTls::Explicit;
        assert_eq!(kind(f.state.connection_save(plain)), "invalidInput");
        assert_eq!(f.state.connections_list().len(), 1);
        // change in place
        let mut edit = list[0].profile.clone();
        edit.name = "Umbenannt".into();
        f.state.connection_save(edit).unwrap();
        assert_eq!(f.state.connections_list()[0].profile.name, "Umbenannt");
        // an unknown id is not created by saving
        let mut ghost = dto("Geist", ConnectionProtocol::Ftps);
        ghost.id = "zz9".into();
        assert_eq!(kind(f.state.connection_save(ghost)), "notFound");
        f.state.connection_delete(&id).unwrap();
        assert!(f.state.connections_list().is_empty());
        assert_eq!(kind(f.state.connection_delete(&id)), "notFound");
    }

    #[test]
    fn at_most_the_profile_limit_is_stored() {
        let f = fixture();
        for i in 0..dualfile_core::connections::MAX_CONNECTIONS {
            f.state
                .connection_save(dto(&format!("S{i}"), ConnectionProtocol::Ftps))
                .unwrap();
        }
        assert_eq!(
            kind(
                f.state
                    .connection_save(dto("Zuviel", ConnectionProtocol::Ftps))
            ),
            "invalidInput"
        );
    }

    #[test]
    fn old_settings_without_connections_load_and_keep_working() {
        let f = fixture();
        std::fs::create_dir_all(f.root.path().join("config")).unwrap();
        std::fs::write(
            f.root.path().join("config/settings.json"),
            r##"{"version":1,"theme":"papier","accent":"#010203"}"##,
        )
        .unwrap();
        let loaded = f.state.load_settings();
        assert_eq!(loaded.settings.theme, "papier");
        assert!(f.state.connections_list().is_empty());
        f.save(dto("Neu", ConnectionProtocol::Ftps));
        // saving the UI's settings keeps the profiles and their pins
        let mut settings = f.state.load_settings().settings;
        settings.font_size = 20;
        f.state.save_settings(&settings).unwrap();
        assert_eq!(f.state.connections_list().len(), 1);
        assert_eq!(f.state.load_settings().settings.font_size, 20);
    }

    #[test]
    fn connect_asks_first_trusts_exactly_the_shown_fingerprint_and_pins_it() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        let outcome = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        let ConnectOutcomeDto::NeedsTrust {
            fingerprint,
            kind: trust_kind,
            changed,
            ..
        } = outcome
        else {
            panic!("{outcome:?}")
        };
        assert_eq!(fingerprint, "07".repeat(32));
        assert_eq!(trust_kind, "unknownIssuer");
        assert!(!changed);
        assert!(f.state.connections_list()[0].connection_id.is_none());
        // a different fingerprint is refused and ends the question
        assert_eq!(
            kind(
                f.state
                    .connection_trust(&id, TrustDecisionDto::Always, &"08".repeat(32))
            ),
            "invalidInput"
        );
        assert_eq!(
            kind(
                f.state
                    .connection_trust(&id, TrustDecisionDto::Always, &fingerprint)
            ),
            "invalidInput",
            "the question was consumed"
        );
        assert!(f.state.connections_list()[0].pins.is_empty());
        // ask again, confirm with the shown (differently written) fingerprint
        f.state.connection_connect(&id, Some(PW.into())).unwrap();
        let shown = fingerprint.to_uppercase();
        let connected = f
            .state
            .connection_trust(&id, TrustDecisionDto::Always, &shown)
            .unwrap();
        assert!(matches!(connected, ConnectOutcomeDto::Connected { .. }));
        let entry = &f.state.connections_list()[0];
        assert!(entry.connected);
        assert_eq!(entry.pins.len(), 1);
        assert_eq!(entry.pins[0].sha256, fingerprint);
        // pinned: after disconnecting, connecting needs no question
        f.state.connection_disconnect(&id).unwrap();
        assert!(!f.state.connections_list()[0].connected);
        let again = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        assert!(matches!(again, ConnectOutcomeDto::Connected { .. }));
        // connecting a connected profile gives the same connection back
        let same = f.state.connection_connect(&id, None).unwrap();
        assert_eq!(same, again);
    }

    #[test]
    fn trust_once_is_not_stored_and_asks_again_next_time() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        f.state.connection_connect(&id, Some(PW.into())).unwrap();
        let fingerprint = "07".repeat(32);
        let once = f
            .state
            .connection_trust(&id, TrustDecisionDto::Once, &fingerprint)
            .unwrap();
        assert!(matches!(once, ConnectOutcomeDto::Connected { .. }));
        assert!(f.state.connections_list()[0].pins.is_empty());
        f.state.connection_disconnect(&id).unwrap();
        assert!(matches!(
            f.state.connection_connect(&id, Some(PW.into())).unwrap(),
            ConnectOutcomeDto::NeedsTrust { .. }
        ));
    }

    #[test]
    fn a_changed_certificate_is_shown_and_never_connected_until_trust_is_reset() {
        let f = fixture();
        let (id, _) = f.connect_ftps();
        f.state.connection_disconnect(&id).unwrap();
        f.ftp.state().cert.as_mut().unwrap().sha256 = [9; 32];
        let outcome = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        assert_eq!(
            outcome,
            ConnectOutcomeDto::PinMismatch {
                old: "07".repeat(32),
                new: "09".repeat(32)
            }
        );
        assert!(!f.state.connections_list()[0].connected);
        assert!(f.state.router.remote_ids().is_empty());
        // no way to confirm it
        assert_eq!(
            kind(
                f.state
                    .connection_trust(&id, TrustDecisionDto::Always, &"09".repeat(32))
            ),
            "invalidInput"
        );
        // the deliberate step: reset trust, then the new key is asked about
        f.state.connection_reset_trust(&id).unwrap();
        assert!(f.state.connections_list()[0].pins.is_empty());
        assert!(matches!(
            f.state.connection_connect(&id, Some(PW.into())).unwrap(),
            ConnectOutcomeDto::NeedsTrust { .. }
        ));
    }

    #[test]
    fn an_expired_certificate_is_an_error_not_a_question() {
        let f = fixture();
        f.ftp.state().cert.as_mut().unwrap().chain = ChainStatus::Expired;
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        let error = f
            .state
            .connection_connect(&id, Some(PW.into()))
            .unwrap_err();
        assert_eq!(error.kind, "trust");
    }

    #[test]
    fn a_wrong_password_is_an_auth_error_and_leaves_nothing_open() {
        let f = fixture();
        f.ftp.state().echo_password = true;
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        let fp = "07".repeat(32);
        f.state
            .connection_connect(&id, Some("falsch-XYZ".into()))
            .unwrap();
        let error = f
            .state
            .connection_trust(&id, TrustDecisionDto::Always, &fp)
            .unwrap_err();
        assert_eq!(error.kind, "auth");
        assert!(!error.message.contains("falsch-XYZ"));
        assert!(!f.state.connections_list()[0].connected);
        assert!(f.state.router.remote_ids().is_empty());
    }

    #[test]
    fn plain_ftp_needs_the_confirmation_flag_to_connect() {
        let f = fixture();
        let mut d = dto("Alt", ConnectionProtocol::Ftp);
        let id = f.save(d.clone());
        assert_eq!(
            kind(f.state.connection_connect(&id, Some(PW.into()))),
            "invalidInput"
        );
        d.id = id.clone();
        d.allow_plaintext = true;
        f.state.connection_save(d).unwrap();
        let connected = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        assert!(matches!(connected, ConnectOutcomeDto::Connected { .. }));
    }

    #[test]
    fn sftp_asks_about_a_new_host_key_and_refuses_a_changed_one() {
        let f = fixture();
        let id = f.save(dto("SFTP", ConnectionProtocol::Sftp));
        let ConnectOutcomeDto::NeedsTrust {
            fingerprint,
            key_type,
            kind: trust_kind,
            ..
        } = f.state.connection_connect(&id, Some(PW.into())).unwrap()
        else {
            panic!()
        };
        assert_eq!(trust_kind, "newHostKey");
        assert_eq!(key_type.as_deref(), Some("ssh-ed25519"));
        assert!(matches!(
            f.state
                .connection_trust(&id, TrustDecisionDto::Always, &fingerprint)
                .unwrap(),
            ConnectOutcomeDto::Connected { .. }
        ));
        f.state.connection_disconnect(&id).unwrap();
        f.sftp.state().host_key = HostKey::ed25519(2);
        assert!(matches!(
            f.state.connection_connect(&id, Some(PW.into())).unwrap(),
            ConnectOutcomeDto::PinMismatch { .. }
        ));
        f.sftp.state().host_key = HostKey {
            signature_algorithm: "ssh-dss".into(),
            key_type: "ssh-dss".into(),
            blob: vec![1; 20],
        };
        f.state.connection_reset_trust(&id).unwrap();
        assert_eq!(
            kind(f.state.connection_connect(&id, Some(PW.into()))),
            "algorithmNotAllowed"
        );
    }

    #[test]
    fn the_password_appears_nowhere() {
        let f = fixture();
        f.ftp.state().echo_in_errors = Some(PW.into());
        let (id, root) = f.connect_ftps();
        f.ftp.put("/dir/a.txt", b"hello");
        let mut seen: Vec<String> = Vec::new();
        seen.push(format!("{:?}", f.state.connections_list()));
        seen.push(serde_json::to_string(&f.state.connections_list()).unwrap());
        seen.push(format!("{:?}", f.state.list(&format!("{root}dir"))));
        seen.push(format!("{:?}", f.state.list(&format!("{}nix", root))));
        seen.push(format!("{:?}", f.state.file_info(&format!("{root}nix"))));
        seen.push(format!("{:?}", f.state.make_dir(&root, "x")));
        seen.push(format!("{:?}", f.state.rename(&format!("{root}nix"), "y")));
        seen.push(f.settings_text());
        // the stored secret path as well
        f.state
            .connection_save({
                let mut d = f.state.connections_list()[0].profile.clone();
                d.save_secret = true;
                d
            })
            .unwrap();
        f.state.secret_set(&id, PW.into()).unwrap();
        seen.push(format!("{:?}", f.state.connections_list()));
        seen.push(f.settings_text());
        f.state.connection_disconnect(&id).unwrap();
        seen.push(format!("{:?}", f.state.list(&format!("{root}dir"))));
        for event in f.events.try_iter() {
            seen.push(format!("{event:?}"));
        }
        for text in seen {
            assert!(!text.contains(PW), "{text}");
        }
        // the same for a failed login with an echoing server
        let id2 = f.save(dto("Zwei", ConnectionProtocol::Ftps));
        f.ftp.state().echo_password = true;
        f.ftp.state().cert.as_mut().unwrap().chain = ChainStatus::Valid;
        let error = f
            .state
            .connection_connect(&id2, Some("other-secret-9".into()))
            .unwrap_err();
        assert!(!format!("{error:?}").contains("other-secret-9"));
    }

    #[test]
    fn secrets_go_to_the_keychain_or_fall_back_to_the_session() {
        let f = fixture();
        f.ftp.state().cert.as_mut().unwrap().chain = ChainStatus::Valid;
        let mut d = dto("FTPS", ConnectionProtocol::Ftps);
        d.save_secret = true;
        let id = f.save(d);
        let connected = f.state.connection_connect(&id, Some(PW.into())).unwrap();
        let ConnectOutcomeDto::Connected { secret_store, .. } = connected else {
            panic!()
        };
        assert_eq!(secret_store, SecretStoreDto::Keychain);
        assert_eq!(f.keychain.keys(), [format!("dualfile:{id}")]);
        let entry = &f.state.connections_list()[0];
        assert!(entry.secret_set);
        assert_eq!(entry.secret_store, SecretStoreDto::Keychain);
        // the stored one is used without typing it again
        f.state.connection_disconnect(&id).unwrap();
        assert!(matches!(
            f.state.connection_connect(&id, None).unwrap(),
            ConnectOutcomeDto::Connected { .. }
        ));
        // clearing removes it
        f.state.secret_clear(&id).unwrap();
        assert!(f.keychain.keys().is_empty());
        assert!(!f.state.connections_list()[0].secret_set);
        // a broken keychain: session memory only, never on disk
        f.keychain.set_broken(true);
        assert_eq!(
            f.state.secret_set(&id, PW.into()).unwrap(),
            SecretStoreDto::Session
        );
        let entry = &f.state.connections_list()[0];
        assert!(entry.secret_set);
        assert_eq!(entry.secret_store, SecretStoreDto::Session);
        f.keychain.set_broken(false);
        assert!(f.keychain.keys().is_empty());
        f.state.connection_disconnect(&id).unwrap();
        assert!(matches!(
            f.state.connection_connect(&id, None).unwrap(),
            ConnectOutcomeDto::Connected { .. }
        ));
        assert!(!f.settings_text().contains(PW));
        // without a stored or typed secret the login fails
        f.state.connection_disconnect(&id).unwrap();
        f.state.secret_clear(&id).unwrap();
        assert_eq!(kind(f.state.connection_connect(&id, None)), "auth");
    }

    #[test]
    fn switching_save_secret_off_removes_the_stored_secret() {
        let f = fixture();
        f.ftp.state().cert.as_mut().unwrap().chain = ChainStatus::Valid;
        let mut d = dto("FTPS", ConnectionProtocol::Ftps);
        d.save_secret = true;
        let id = f.save(d.clone());
        f.state.connection_connect(&id, Some(PW.into())).unwrap();
        f.state.connection_disconnect(&id).unwrap();
        assert_eq!(f.keychain.keys(), [format!("dualfile:{id}")]);
        d.id = id.clone();
        d.save_secret = false;
        let entry = f.state.connection_save(d).unwrap();
        assert!(!entry.secret_set);
        assert!(f.keychain.keys().is_empty());
        // the connect asks for a password again instead of using an old one
        assert_eq!(kind(f.state.connection_connect(&id, None)), "auth");
    }

    #[test]
    fn a_keychain_that_keeps_the_secret_stops_switching_save_secret_off() {
        let f = fixture();
        let mut d = dto("FTPS", ConnectionProtocol::Ftps);
        d.save_secret = true;
        let id = f.save(d.clone());
        f.state.secret_set(&id, PW.into()).unwrap();
        f.keychain.set_broken(true);
        d.id = id.clone();
        d.save_secret = false;
        let error = f.state.connection_save(d).unwrap_err();
        assert_eq!(error.kind, "secretRemovalFailed");
        assert!(!error.message.contains(PW));
        // the profile still says "save the password"
        f.keychain.set_broken(false);
        assert!(f.state.profile(&id).unwrap().save_secret);
        assert_eq!(f.keychain.keys(), [format!("dualfile:{id}")]);
    }

    #[test]
    fn clearing_a_secret_that_cannot_be_removed_reports_it() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        f.state.secret_set(&id, PW.into()).unwrap();
        f.keychain.set_broken(true);
        let error = f.state.secret_clear(&id).unwrap_err();
        assert_eq!(error.kind, "secretRemovalFailed");
        assert!(!error.message.contains(PW));
        f.keychain.set_broken(false);
        f.state.secret_clear(&id).unwrap();
        assert!(f.keychain.keys().is_empty());
    }

    #[test]
    fn deleting_a_profile_preserves_it_when_keychain_removal_fails() {
        let f = fixture();
        let mut d = dto("FTPS", ConnectionProtocol::Ftps);
        d.save_secret = true;
        let id = f.save(d);
        f.state.secret_set(&id, PW.into()).unwrap();
        f.keychain.set_broken(true);
        assert_eq!(kind(f.state.connection_delete(&id)), "secretRemovalFailed");
        assert!(f.state.profile(&id).is_ok());
        f.keychain.set_broken(false);
        assert_eq!(f.keychain.keys(), [format!("dualfile:{id}")]);
        f.state.connection_delete(&id).unwrap();
        assert!(f.keychain.keys().is_empty());
        assert_eq!(kind(f.state.connection_delete(&id)), "notFound");
    }

    #[test]
    fn deleting_a_profile_without_saved_secret_ignores_a_broken_keychain() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        f.keychain.set_broken(true);
        f.state.connection_delete(&id).unwrap();
        assert_eq!(kind(f.state.connection_delete(&id)), "notFound");
    }

    #[test]
    fn deleting_a_connecting_profile_is_refused() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        crate::state::lock(&f.state.connections.connecting).insert(id.clone());
        assert_eq!(kind(f.state.connection_delete(&id)), "invalidInput");
        assert!(f.state.profile(&id).is_ok());
    }

    #[test]
    fn deleting_a_profile_removes_its_connection_secret_and_question() {
        let f = fixture();
        f.ftp.state().cert.as_mut().unwrap().chain = ChainStatus::Valid;
        let mut d = dto("FTPS", ConnectionProtocol::Ftps);
        d.save_secret = true;
        let id = f.save(d);
        let ConnectOutcomeDto::Connected { root, .. } =
            f.state.connection_connect(&id, Some(PW.into())).unwrap()
        else {
            panic!()
        };
        assert_eq!(f.keychain.keys().len(), 1);
        f.state.connection_delete(&id).unwrap();
        assert!(f.keychain.keys().is_empty());
        assert!(f.state.router.remote_ids().is_empty());
        assert_eq!(kind(f.state.list(&root)), "disconnected");
        assert!(f.state.connections.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn remote_tabs_load_disconnected_and_nothing_connects_by_itself() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        let mut settings = f.state.load_settings().settings;
        settings.session.windows[0].tabs[0].path = format!("conn:{id}!/home");
        settings.bookmarks.push(crate::convert::BookmarkDto {
            name: "Server".into(),
            path: format!("conn:{id}!/pub"),
            color: "#112233".into(),
            order: 0,
        });
        f.state.save_settings(&settings).unwrap();
        let again = f.state.load_settings().settings;
        assert_eq!(
            again.session.windows[0].tabs[0].path,
            format!("conn:{id}!/home")
        );
        assert_eq!(
            kind(f.state.list(&format!("conn:{id}!/home"))),
            "disconnected"
        );
        assert_eq!(f.ftp.state().connects, 0);
        assert_eq!(f.sftp.state().connects, 0);
        // a running connection's location is never written into the settings
        let mut bad = again.clone();
        bad.session.windows[0].tabs[0].path = "ftp:c1-abc!/x".into();
        assert_eq!(kind(f.state.save_settings(&bad)), "invalidSettings");
        let mut forged = again;
        forged.session.windows[0].tabs[0].path = "conn:P!/..".into();
        assert_eq!(kind(f.state.save_settings(&forged)), "invalidSettings");
    }

    #[test]
    fn profile_paths_resolve_on_the_open_connection() {
        let f = fixture();
        let (id, root) = f.connect_ftps();
        f.ftp.put("/pub/a.txt", b"abc");
        let by_profile = f.state.list(&format!("conn:{id}!/pub")).unwrap();
        let by_connection = f.state.list(&format!("{root}pub").replace("//", "/")).ok();
        assert_eq!(by_profile.entries.len(), 1);
        assert!(by_profile.path.starts_with("ftp:"));
        let _ = by_connection;
        f.state.connection_disconnect(&id).unwrap();
        assert_eq!(
            kind(f.state.list(&format!("conn:{id}!/pub"))),
            "disconnected"
        );
    }

    #[test]
    fn copy_works_in_both_directions_as_jobs() {
        let f = fixture();
        let (_, root) = f.connect_ftps();
        let base = root.trim_end_matches('/').to_owned();
        f.ftp.put("/down/a.txt", b"vom-server");
        std::fs::create_dir_all(f.root.path().join("ziel")).unwrap();
        let job = f
            .state
            .copy(&[format!("{base}/down/a.txt")], &f.local("ziel"))
            .unwrap();
        assert_eq!(f.wait(job), JobState::Done);
        assert_eq!(std::fs::read(f.local("ziel/a.txt")).unwrap(), b"vom-server");
        assert!(f
            .state
            .jobs()
            .iter()
            .any(|j| j.name == "Herunterladen: a.txt"));
        std::fs::write(f.local("hoch.txt"), "zum-server").unwrap();
        f.ftp
            .state()
            .nodes
            .insert("/up".into(), dualfile_remote::fake::Node::Dir);
        let job = f
            .state
            .copy(&[f.local("hoch.txt")], &format!("{base}/up"))
            .unwrap();
        assert_eq!(f.wait(job), JobState::Done);
        assert_eq!(f.ftp.get("/up/hoch.txt").unwrap(), b"zum-server");
        assert!(f.state.undo_list().is_empty());
        assert!(f
            .state
            .jobs()
            .iter()
            .any(|j| j.name == "Hochladen: hoch.txt"));
        // the refreshed folder is announced in the UI form
        let events: Vec<_> = f.events.try_iter().collect();
        assert!(events.contains(&CoreEventDto::DirChanged {
            path: format!("{base}/up")
        }));
        // a missing destination folder on the server is refused up front
        assert_eq!(
            kind(
                f.state
                    .copy(&[f.local("hoch.txt")], &format!("{base}/fehlt"))
            ),
            "notFound"
        );
    }

    #[test]
    fn rename_mkdir_and_move_work_on_a_server() {
        let f = fixture();
        let (_, root) = f.connect_ftps();
        let base = root.trim_end_matches('/').to_owned();
        f.ftp.put("/w/a.txt", b"1");
        let renamed = f.state.rename(&format!("{base}/w/a.txt"), "b.txt").unwrap();
        assert_eq!(renamed.new_path, format!("{base}/w/b.txt"));
        assert!(f.ftp.exists("/w/b.txt") && !f.ftp.exists("/w/a.txt"));
        let made = f.state.make_dir(&format!("{base}/w"), "sub").unwrap();
        assert_eq!(made, format!("{base}/w/sub"));
        let job = f
            .state
            .move_items(&[format!("{base}/w/b.txt")], &format!("{base}/w/sub"))
            .unwrap();
        assert_eq!(f.wait(job), JobState::Done);
        assert!(f.ftp.exists("/w/sub/b.txt") && !f.ftp.exists("/w/b.txt"));
        assert!(f.state.undo_list().is_empty());
    }

    #[test]
    fn delete_plans_on_a_server_are_bound_to_that_connection() {
        let f = fixture();
        let (id, root) = f.connect_ftps();
        let base = root.trim_end_matches('/').to_owned();
        f.ftp.put("/d/x.txt", b"x");
        f.ftp.put("/d/y.txt", b"y");
        let plan = f.state.plan_delete(&[format!("{base}/d/x.txt")]).unwrap();
        let input = DeletePlanInput {
            token: plan.token.clone(),
        };
        // the connection is closed and opened again (another connection id): refused
        f.state.connection_disconnect(&id).unwrap();
        f.state.connection_connect(&id, Some(PW.into())).unwrap();
        assert_eq!(
            kind(f.state.delete_permanent(&input, &plan.token)),
            "invalidToken"
        );
        assert!(f.ftp.exists("/d/x.txt"));
        // a fresh plan on the new connection works exactly once
        let new_root = match f.state.connections_list()[0].root.clone() {
            Some(root) => root.trim_end_matches('/').to_owned(),
            None => panic!(),
        };
        let plan = f
            .state
            .plan_delete(&[format!("{new_root}/d/x.txt")])
            .unwrap();
        let input = DeletePlanInput {
            token: plan.token.clone(),
        };
        let job = f.state.delete_permanent(&input, &plan.token).unwrap();
        assert_eq!(f.wait(job), JobState::Done);
        assert!(!f.ftp.exists("/d/x.txt") && f.ftp.exists("/d/y.txt"));
        assert_eq!(
            kind(f.state.delete_permanent(&input, &plan.token)),
            "invalidToken"
        );
    }

    #[test]
    fn servers_have_no_trash_no_pack_target_no_sync_target() {
        let f = fixture();
        let (_, root) = f.connect_ftps();
        let base = root.trim_end_matches('/').to_owned();
        f.ftp.put("/t/a.txt", b"a");
        let item = format!("{base}/t/a.txt");
        assert_eq!(
            kind(f.state.trash(std::slice::from_ref(&item))),
            "unsupported"
        );
        assert_eq!(
            kind(f.state.restore_from_trash(std::slice::from_ref(&item))),
            "unsupported"
        );
        // Opening a remote file externally is supported now (it is copied locally first and
        // handed to xdg-open); that path is covered without launching a desktop app by the
        // `fetch_remote_to_temp` and `remote_open_*` unit tests in `state.rs`.
        assert!(
            f.ftp.exists("/t/a.txt"),
            "nothing was deleted as a replacement"
        );
        // pack: remote source fine, remote destination refused
        let pack = f.state.pack_archive(
            std::slice::from_ref(&item),
            &format!("{base}/t"),
            "x.zip",
            crate::convert::PackFormatDto::Zip,
        );
        assert_eq!(kind(pack), "unsupported");
        std::fs::create_dir_all(f.root.path().join("out")).unwrap();
        let job = f
            .state
            .pack_archive(
                &[item],
                &f.local("out"),
                "x.zip",
                crate::convert::PackFormatDto::Zip,
            )
            .unwrap();
        assert_eq!(f.wait(job), JobState::Done);
        assert!(f.root.path().join("out/x.zip").exists());
        // sync: server as target is refused, a plan to a local target from the server works
        std::fs::create_dir_all(f.root.path().join("l")).unwrap();
        let opts = crate::compare::CompareOptionsDto::default();
        let compare = f
            .state
            .compare_dirs(&f.local("l"), &format!("{base}/t"), &opts)
            .unwrap();
        assert_eq!(f.wait(compare), JobState::Done);
        let sync = f.state.plan_sync(
            compare,
            crate::compare::SyncModeDto::LeftToRight,
            &crate::compare::SyncOptionsDto::default(),
            None,
        );
        assert_eq!(kind(sync), "unsupported");
        let back = f.state.plan_sync(
            compare,
            crate::compare::SyncModeDto::RightToLeft,
            &crate::compare::SyncOptionsDto::default(),
            None,
        );
        assert!(back.is_ok(), "{back:?}");
    }

    #[test]
    fn a_lost_connection_answers_disconnected_instead_of_crashing() {
        let f = fixture();
        let (id, root) = f.connect_ftps();
        f.ftp.put("/q/a.txt", b"a");
        assert!(f.state.list(&format!("{root}q")).is_ok());
        f.state.connection_disconnect(&id).unwrap();
        for result in [
            kind(f.state.list(&format!("{root}q"))),
            kind(f.state.file_info(&format!("{root}q/a.txt"))),
            kind(f.state.read_block(&format!("{root}q/a.txt"), 0, 10)),
            kind(f.state.make_dir(&root, "n")),
            kind(f.state.rename(&format!("{root}q/a.txt"), "b")),
            kind(f.state.plan_delete(&[format!("{root}q/a.txt")])),
            kind(f.state.copy(&[format!("{root}q/a.txt")], &f.local(""))),
        ] {
            assert_eq!(result, "disconnected");
        }
        assert!(f.state.watch(&[format!("{root}q")]).is_ok());
    }

    #[test]
    fn watching_skips_server_locations() {
        let f = fixture();
        let id = f.save(dto("FTPS", ConnectionProtocol::Ftps));
        assert!(f
            .state
            .watch(&[
                "ftp:c1-00000!/x".to_owned(),
                format!("conn:{id}!/y"),
                f.local("home")
            ])
            .is_ok());
    }
}
