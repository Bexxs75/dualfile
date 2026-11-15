//! Server connection profiles as the settings keep them (Phase 6c), and the persistent form of
//! a server location.
//!
//! A profile has **no password field**: passwords and key passphrases live in the keychain or
//! in the session memory (`dualfile-remote`). The pins (accepted certificate / host key
//! fingerprints) belong to the profile but are only ever written by the connection code.
//!
//! Persistent server locations (bookmarks, tabs, last places) are written as
//! `conn:<profile id>!<path>`: the profile id is stable, the connection id of a running
//! connection (`ftp:<connection id>!<path>`) is not and never ends up in a file. The text
//! after the first `!` is the path, taken literally like in the IPC format.

use crate::error::{Error, Result};
use crate::vfs::{is_valid_connection_id, VPath};
use serde::{Deserialize, Serialize};

/// Profiles kept at most.
pub const MAX_CONNECTIONS: usize = 100;
/// Pins per profile at most.
pub const MAX_PINS: usize = 16;
/// Start of a persistent server location.
pub const PROFILE_PATH_PREFIX: &str = "conn:";

const MAX_NAME_CHARS: usize = 100;
const MAX_HOST_BYTES: usize = 253;
const MAX_USER_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionProtocol {
    /// FTP without TLS (needs `allow_plaintext`).
    Ftp,
    /// FTP with TLS (explicit or implicit).
    #[default]
    Ftps,
    Sftp,
    Smb,
    /// WebDAV over HTTP(S); `share` holds the server base path, `tls` other than `None` = https.
    Webdav,
    System,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionTls {
    /// No encryption (FTP) or not applicable (SFTP).
    None,
    #[default]
    Explicit,
    Implicit,
}

/// SSH implementation. Port zero and an empty user mean “use ssh configuration” for OpenSSH.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransport {
    #[default]
    Builtin,
    Openssh,
}

pub fn valid_ssh_endpoint(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 255
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._@:%[]-".contains(&b))
}

/// How an SFTP login proves who the user is (FTP/FTPS always use a password).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionAuth {
    /// The secret is the password.
    #[default]
    Password,
    /// The secret is the passphrase of this OpenSSH private key file (empty = none).
    KeyFile { path: String },
}

/// Which pins a protocol knows (plain FTP has none).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinProtocol {
    Ftps,
    Sftp,
}

/// A trusted certificate / host key: SHA-256 as 64 lower-case hex digits, for SFTP also the
/// key type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PinEntry {
    pub protocol: PinProtocol,
    pub host: String,
    pub port: u16,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionProfile {
    /// Stable id (`[a-z0-9-]`, at most 32 characters), assigned when the profile is saved the
    /// first time.
    pub id: String,
    pub name: String,
    pub protocol: ConnectionProtocol,
    pub transport: SshTransport,
    pub system_url: Option<String>,
    pub share: String,
    pub domain: String,
    pub guest: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub start_folder: Option<String>,
    pub tls: ConnectionTls,
    /// The user's confirmation of an unencrypted FTP connection (the warning was shown).
    pub allow_plaintext: bool,
    pub auth: ConnectionAuth,
    /// Keep the password / passphrase in the keychain.
    pub save_secret: bool,
    pub pins: Vec<PinEntry>,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            protocol: ConnectionProtocol::Ftps,
            transport: SshTransport::Builtin,
            system_url: None,
            share: String::new(),
            domain: String::new(),
            guest: false,
            host: String::new(),
            port: 21,
            user: String::new(),
            start_folder: None,
            tls: ConnectionTls::Explicit,
            allow_plaintext: false,
            auth: ConnectionAuth::Password,
            save_secret: false,
            pins: Vec::new(),
        }
    }
}

fn plain(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && !host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '/' || c == '\\')
}

fn valid_folder(folder: &str) -> bool {
    folder.starts_with('/') && folder.len() <= MAX_PATH_BYTES && plain(folder)
}

/// A WebDAV host: a DNS name / IPv4 literal (letters, digits, `.`, `-`) or a bracketed IPv6 literal.
/// Stricter than [`valid_host`] because the host goes into a URL alongside the credentials: it must
/// not carry userinfo (`@`), a port (`:` outside brackets), a path (`/`), query (`?`) or fragment
/// (`#`), or the server part of the origin — and with it the `Authorization` header — could be
/// redirected to another host.
pub fn valid_webdav_host(host: &str) -> bool {
    if host.is_empty() || host.len() > MAX_HOST_BYTES {
        return false;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':');
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

impl ConnectionProfile {
    /// Checks everything the profile itself says (not whether the name is unique; see
    /// `validate_connections`). The texts are shown to the user as they are.
    pub fn validate(&self) -> std::result::Result<(), &'static str> {
        if !is_valid_connection_id(&self.id) {
            return Err("Ungültige Profilkennung");
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || !plain(name) {
            return Err("Ungültiger Name");
        }
        if self.protocol == ConnectionProtocol::System {
            #[cfg(unix)]
            if let Some(place) = self
                .system_url
                .as_deref()
                .and_then(|url| crate::system_places::parse_url(url, "", ""))
            {
                if Some(&place.url) == self.system_url.as_ref()
                    && matches!(
                        place.scheme.as_str(),
                        "smb" | "nfs" | "dav" | "davs" | "webdav" | "webdavs" | "afp"
                    )
                    && self.port == place.port.unwrap_or(0)
                    && self.user == place.user.clone().unwrap_or_default()
                    && self.start_folder.as_deref() == Some(place.path.as_str())
                    && self.host == place.host
                    && self.transport == SshTransport::Builtin
                    && !self.save_secret
                    && self.pins.is_empty()
                    && self.auth == ConnectionAuth::Password
                    && self.tls == ConnectionTls::None
                    && !self.allow_plaintext
                {
                    return Ok(());
                }
            }
            return Err("Ungültige System-Einbindung");
        }
        if self.system_url.is_some() {
            return Err("System-URL nur für System-Einbindungen");
        }
        let openssh = self.transport == SshTransport::Openssh;
        if openssh && (self.protocol != ConnectionProtocol::Sftp || self.save_secret) {
            return Err("OpenSSH ist nur für SFTP ohne gespeichertes Passwort verfügbar");
        }
        if (openssh && !valid_ssh_endpoint(&self.host)) || (!openssh && !valid_host(&self.host)) {
            return Err("Ungültiger Servername");
        }
        if self.port == 0 && !openssh {
            return Err("Ungültiger Port");
        }
        if (!openssh
            && self.user.is_empty()
            && !matches!(
                self.protocol,
                ConnectionProtocol::Smb | ConnectionProtocol::Webdav
            ))
            || self.user.len() > MAX_USER_BYTES
            || !plain(&self.user)
            || (openssh && !self.user.is_empty() && !valid_ssh_endpoint(&self.user))
        {
            return Err("Ungültiger Benutzername");
        }
        if let Some(folder) = &self.start_folder {
            if !valid_folder(folder) {
                return Err("Ungültiger Startordner");
            }
        }
        if self.protocol == ConnectionProtocol::Smb {
            if self
                .start_folder
                .as_ref()
                .is_some_and(|folder| VPath::smb(&self.id, folder).is_err())
            {
                return Err("Ungültiger Startordner");
            }
            if self.share.len() > 255
                || matches!(self.share.as_str(), "." | "..")
                || self
                    .share
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            {
                return Err("Ungültige SMB-Freigabe");
            }
            if self.domain.len() > 256 || !plain(&self.domain) {
                return Err("Ungültige SMB-Domäne");
            }
            if !self.guest && self.user.eq_ignore_ascii_case("guest") {
                return Err("Gast ausdrücklich auswählen");
            }
        }
        if self.protocol == ConnectionProtocol::Webdav {
            if !valid_webdav_host(&self.host) {
                return Err("Ungültiger WebDAV-Servername");
            }
            if !valid_folder(&self.share) || VPath::webdav(&self.id, &self.share).is_err() {
                return Err("Ungültiger WebDAV-Basispfad");
            }
        }
        match self.protocol {
            ConnectionProtocol::System => unreachable!(),
            ConnectionProtocol::Ftp => {
                if self.tls != ConnectionTls::None {
                    return Err("FTP ohne Verschlüsselung verwendet keinen TLS-Modus");
                }
            }
            ConnectionProtocol::Ftps => {
                if self.tls == ConnectionTls::None {
                    return Err("FTPS braucht einen TLS-Modus (explizit oder implizit)");
                }
            }
            ConnectionProtocol::Sftp | ConnectionProtocol::Smb => {
                if self.tls != ConnectionTls::None {
                    return Err("SFTP verwendet keinen TLS-Modus");
                }
            }
            ConnectionProtocol::Webdav => {
                if self.tls == ConnectionTls::None && !self.allow_plaintext {
                    return Err(
                        "Unverschlüsselte Verbindungen brauchen eine ausdrückliche Bestätigung",
                    );
                }
            }
        }
        if self.allow_plaintext
            && !matches!(
                self.protocol,
                ConnectionProtocol::Ftp | ConnectionProtocol::Webdav
            )
        {
            return Err("Die Bestätigung für unverschlüsselte Verbindungen gilt nur für FTP");
        }
        match &self.auth {
            ConnectionAuth::Password => {}
            ConnectionAuth::KeyFile { path } => {
                if self.protocol != ConnectionProtocol::Sftp {
                    return Err("Schlüsseldateien gibt es nur bei SFTP");
                }
                if path.is_empty() || path.len() > MAX_PATH_BYTES || !plain(path) {
                    return Err("Ungültiger Pfad der Schlüsseldatei");
                }
            }
        }
        if self.pins.len() > MAX_PINS {
            return Err("Zu viele gespeicherte Fingerabdrücke");
        }
        for pin in &self.pins {
            let expected = match self.protocol {
                ConnectionProtocol::System
                | ConnectionProtocol::Ftp
                | ConnectionProtocol::Smb
                | ConnectionProtocol::Webdav => {
                    return Err("FTP ohne TLS hat keine Fingerabdrücke")
                }
                ConnectionProtocol::Ftps => PinProtocol::Ftps,
                ConnectionProtocol::Sftp => PinProtocol::Sftp,
            };
            let hex_ok = pin.sha256.len() == 64
                && pin
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            let type_ok = pin
                .key_type
                .as_ref()
                .is_none_or(|t| !t.is_empty() && t.len() <= 64 && plain(t));
            if pin.protocol != expected
                || !valid_host(&pin.host)
                || pin.port == 0
                || !hex_ok
                || !type_ok
            {
                return Err("Ungültiger gespeicherter Fingerabdruck");
            }
        }
        Ok(())
    }
}

/// Name comparison for "unique": case-insensitive, surrounding blanks ignored.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Checks a whole profile list: every profile valid, ids and names unique, at most
/// `MAX_CONNECTIONS`.
pub fn validate_connections(list: &[ConnectionProfile]) -> std::result::Result<(), &'static str> {
    if list.len() > MAX_CONNECTIONS {
        return Err("Zu viele Verbindungsprofile");
    }
    for (index, profile) in list.iter().enumerate() {
        profile.validate()?;
        if list[..index].iter().any(|other| other.id == profile.id) {
            return Err("Doppelte Profilkennung");
        }
        if list[..index]
            .iter()
            .any(|other| name_key(&other.name) == name_key(&profile.name))
        {
            return Err("Es gibt schon ein Profil mit diesem Namen");
        }
    }
    Ok(())
}

/// A fresh profile id that no profile in `existing` has: `p` plus eight hex digits.
pub fn new_profile_id(existing: &[ConnectionProfile]) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mixed = nanos
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(u64::from(std::process::id()) << 20)
            .wrapping_add(
                COUNTER
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_mul(0x1000_0001),
            );
        let id = format!("p{:08x}", (mixed >> 16) as u32);
        if !existing.iter().any(|profile| profile.id == id) {
            return id;
        }
    }
}

/// `conn:<profile id>!<path>`; the path is written as it is (see the module text).
pub fn format_profile_path(profile_id: &str, path: &str) -> String {
    format!("{PROFILE_PATH_PREFIX}{profile_id}!{path}")
}

/// Splits a persistent server location into profile id and normalized path. Forged texts
/// (bad id, relative path, `..`, control characters, too long) are refused.
pub fn parse_profile_path(text: &str) -> Result<(String, String)> {
    let bad = || Error::InvalidPath("Ungültiger Serverpfad".into());
    let rest = text.strip_prefix(PROFILE_PATH_PREFIX).ok_or_else(bad)?;
    let (id, path) = rest.split_once('!').ok_or_else(bad)?;
    if !is_valid_connection_id(id) {
        return Err(bad());
    }
    // the path rules are the ones of a server location; the id is a stand-in
    let normalized = VPath::ftp("x", path)?;
    Ok((
        id.to_owned(),
        normalized.path().to_string_lossy().into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webdav_host_rejects_url_injection_and_accepts_plain_hosts() {
        // WD-4A-01: a host carrying userinfo/port/path/query/fragment could redirect the request —
        // and the credentials — to another origin, so all of these are rejected.
        for host in [
            "dav.example.org",
            "192.168.1.2",
            "a-b.c",
            "[::1]",
            "[2001:db8::1]",
        ] {
            assert!(valid_webdav_host(host), "{host} should be valid");
        }
        for host in [
            "",
            "trusted.example@attacker.example",
            "host:8080",
            "host/path",
            "host?q=1",
            "host#frag",
            "ho st",
            "host\\x",
            "[]",
            "[zz::1]",
        ] {
            assert!(!valid_webdav_host(host), "{host} should be rejected");
        }
    }

    fn profile(id: &str, name: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: id.into(),
            name: name.into(),
            host: "ftp.example.org".into(),
            port: 21,
            user: "u".into(),
            ..ConnectionProfile::default()
        }
    }

    #[test]
    fn webdav_profiles_validate_base_path_and_plaintext() {
        let dav = ConnectionProfile {
            protocol: ConnectionProtocol::Webdav,
            tls: ConnectionTls::Explicit,
            port: 443,
            share: "/dav".into(),
            ..profile("dav1", "dav")
        };
        assert_eq!(dav.validate(), Ok(()));
        let mut p = dav.clone();
        p.tls = ConnectionTls::None;
        assert!(p.validate().is_err());
        p.allow_plaintext = true;
        assert_eq!(p.validate(), Ok(()));
        p = dav.clone();
        p.share = "dav".into();
        assert!(p.validate().is_err());
        p = dav;
        p.user.clear();
        assert_eq!(p.validate(), Ok(()));
    }

    fn sftp(id: &str, name: &str) -> ConnectionProfile {
        ConnectionProfile {
            protocol: ConnectionProtocol::Sftp,
            tls: ConnectionTls::None,
            port: 22,
            ..profile(id, name)
        }
    }

    #[test]
    fn smb_settings_defaults_validation_and_legacy_mount() {
        let mut p: ConnectionProfile = serde_json::from_str(
            r#"{"id":"nas","name":"NAS","protocol":"smb","host":"nas","port":445,"tls":"none"}"#,
        )
        .unwrap();
        assert!(p.validate().is_ok());
        assert!(p.share.is_empty() && p.domain.is_empty() && !p.guest);
        p.share = "bad/share".into();
        assert!(p.validate().is_err());
        p.share = "team".into();
        p.port = 0;
        assert!(p.validate().is_err());
        p.port = 445;
        p.guest = true;
        assert!(p.validate().is_ok());
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("password_value"));
        let legacy = ConnectionProfile {
            protocol: ConnectionProtocol::System,
            system_url: Some("smb://nas/team".into()),
            share: String::new(),
            port: 0,
            guest: false,
            start_folder: Some("/team".into()),
            ..p
        };
        assert!(legacy.validate().is_ok());
    }

    #[test]
    fn a_normal_profile_of_each_protocol_is_valid() {
        assert!(profile("a1", "FTPS").validate().is_ok());
        assert!(sftp("a2", "SFTP").validate().is_ok());
        let mut plain = profile("a3", "Alt");
        plain.protocol = ConnectionProtocol::Ftp;
        plain.tls = ConnectionTls::None;
        plain.allow_plaintext = true;
        assert!(plain.validate().is_ok());
        let mut key = sftp("a4", "Schlüssel");
        key.auth = ConnectionAuth::KeyFile {
            path: "/home/u/.ssh/id_ed25519".into(),
        };
        assert!(key.validate().is_ok());
    }

    #[test]
    fn bad_values_are_refused() {
        type Mutation = fn(&mut ConnectionProfile);
        let cases: Vec<(&str, Mutation)> = vec![
            ("id leer", |p| p.id = String::new()),
            ("id Großbuchstabe", |p| p.id = "A1".into()),
            ("id zu lang", |p| p.id = "a".repeat(33)),
            ("name leer", |p| p.name = "  ".into()),
            ("name Steuerzeichen", |p| p.name = "a\nb".into()),
            ("name zu lang", |p| p.name = "x".repeat(101)),
            ("host leer", |p| p.host = String::new()),
            ("host Leerzeichen", |p| p.host = "a b".into()),
            ("host Steuerzeichen", |p| p.host = "a\r\nb".into()),
            ("host Schrägstrich", |p| p.host = "a/b".into()),
            ("host zu lang", |p| p.host = "h".repeat(254)),
            ("port 0", |p| p.port = 0),
            ("benutzer leer", |p| p.user = String::new()),
            ("benutzer Steuerzeichen", |p| p.user = "u\nPASS x".into()),
            ("startordner relativ", |p| {
                p.start_folder = Some("rel".into())
            }),
            ("startordner Steuerzeichen", |p| {
                p.start_folder = Some("/a\nb".into())
            }),
            ("startordner zu lang", |p| {
                p.start_folder = Some(format!("/{}", "a".repeat(5000)))
            }),
            ("ftps ohne tls", |p| p.tls = ConnectionTls::None),
            ("ftp mit tls", |p| p.protocol = ConnectionProtocol::Ftp),
            ("plaintext-flag bei ftps", |p| p.allow_plaintext = true),
            ("schluesseldatei bei ftps", |p| {
                p.auth = ConnectionAuth::KeyFile { path: "/k".into() }
            }),
            ("pin ohne hex", |p| {
                p.pins = vec![PinEntry {
                    protocol: PinProtocol::Ftps,
                    host: "h".into(),
                    port: 21,
                    sha256: "zz".into(),
                    key_type: None,
                }]
            }),
            ("pin falsches protokoll", |p| {
                p.pins = vec![PinEntry {
                    protocol: PinProtocol::Sftp,
                    host: "h".into(),
                    port: 21,
                    sha256: "a".repeat(64),
                    key_type: None,
                }]
            }),
            ("zu viele pins", |p| {
                p.pins = (0..=MAX_PINS)
                    .map(|i| PinEntry {
                        protocol: PinProtocol::Ftps,
                        host: "h".into(),
                        port: 21 + i as u16,
                        sha256: "a".repeat(64),
                        key_type: None,
                    })
                    .collect()
            }),
        ];
        for (what, mutate) in cases {
            let mut p = profile("a1", "x");
            mutate(&mut p);
            assert!(p.validate().is_err(), "{what}");
        }
        let mut key = sftp("a1", "x");
        key.auth = ConnectionAuth::KeyFile {
            path: String::new(),
        };
        assert!(key.validate().is_err());
        let mut sftp_tls = sftp("a1", "x");
        sftp_tls.tls = ConnectionTls::Explicit;
        assert!(sftp_tls.validate().is_err());
    }

    #[test]
    fn lists_need_unique_ids_and_names_and_a_bounded_size() {
        assert!(validate_connections(&[profile("a1", "Eins"), profile("a2", "Zwei")]).is_ok());
        assert!(validate_connections(&[profile("a1", "Eins"), profile("a1", "Zwei")]).is_err());
        assert!(validate_connections(&[profile("a1", "Eins"), profile("a2", " EINS ")]).is_err());
        let many: Vec<ConnectionProfile> = (0..=MAX_CONNECTIONS)
            .map(|i| profile(&format!("a{i}"), &format!("n{i}")))
            .collect();
        assert!(validate_connections(&many).is_err());
        assert!(validate_connections(&many[..MAX_CONNECTIONS]).is_ok());
    }

    #[test]
    fn new_ids_are_valid_and_do_not_repeat() {
        let mut list: Vec<ConnectionProfile> = Vec::new();
        for i in 0..50 {
            let id = new_profile_id(&list);
            assert!(is_valid_connection_id(&id), "{id}");
            list.push(profile(&id, &format!("n{i}")));
        }
        let mut ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 50);
    }

    #[test]
    fn profile_paths_round_trip_and_forgeries_fail() {
        assert_eq!(format_profile_path("p1", "/a/b!c%d"), "conn:p1!/a/b!c%d");
        assert_eq!(
            parse_profile_path("conn:p1!/a//b/").unwrap(),
            ("p1".to_owned(), "/a/b".to_owned())
        );
        assert_eq!(
            parse_profile_path("conn:p1!/a/b!c%d").unwrap(),
            ("p1".to_owned(), "/a/b!c%d".to_owned())
        );
        assert_eq!(
            parse_profile_path("conn:p-1!/").unwrap(),
            ("p-1".to_owned(), "/".to_owned())
        );
        for bad in [
            "conn:",
            "conn:p1",
            "conn:P1!/x",
            "conn:!/x",
            "conn:p1!rel",
            "conn:p1!/a/../b",
            "conn:p1!/a\nb",
            "conn:p1!/a\0b",
            "ftp:p1!/x",
            "/conn:p1!/x",
        ] {
            assert!(parse_profile_path(bad).is_err(), "{bad:?}");
        }
        let long = format!("conn:p1!/{}", "a".repeat(5000));
        assert!(parse_profile_path(&long).is_err());
    }

    #[test]
    fn a_profile_serializes_without_any_secret_field() {
        let json = serde_json::to_value(profile("a1", "x")).unwrap();
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert!(keys
            .iter()
            .all(|k| !k.contains("pass") && !k.contains("secret_value")));
        // an old or partial file keeps loading with defaults
        let loaded: ConnectionProfile =
            serde_json::from_str(r#"{"id":"a1","name":"x","host":"h","user":"u"}"#).unwrap();
        assert_eq!(loaded.protocol, ConnectionProtocol::Ftps);
        assert_eq!(loaded.tls, ConnectionTls::Explicit);
    }
}
