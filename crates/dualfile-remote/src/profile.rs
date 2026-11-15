//! What it takes to open one FTP connection. The password is not part of the profile; it
//! is handed over separately as a `Secret` and never stored.

use crate::transport::{Limits, Timeouts};
pub use dualfile_core::connections::{valid_ssh_endpoint, valid_webdav_host, SshTransport};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Plain connection upgraded with `AUTH TLS`.
    Explicit,
    /// TLS from the first byte (usually port 990).
    Implicit,
    /// No encryption: password and data in the clear. Only with `allow_plaintext`.
    None,
}

/// Connection profile for an FTP/FTPS server (what the settings keep).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FtpProfile {
    /// Name shown to the user.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub tls: TlsMode,
    /// The caller's explicit confirmation of an unencrypted connection (the warning was
    /// shown). Without it `TlsMode::None` is refused.
    #[serde(default)]
    pub allow_plaintext: bool,
    /// Folder shown after connecting; `None` = the server's start folder.
    #[serde(default)]
    pub start_folder: Option<String>,
}

impl FtpProfile {
    pub fn default_port(tls: TlsMode) -> u16 {
        if tls == TlsMode::Implicit {
            990
        } else {
            21
        }
    }

    /// Checks the profile before any network use.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host.is_empty()
            || self.host.len() > 253
            || self
                .host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '/')
        {
            return Err("Ungültiger Servername");
        }
        if self.port == 0 {
            return Err("Ungültiger Port");
        }
        if self.user.len() > 256 || self.user.chars().any(char::is_control) {
            return Err("Ungültiger Benutzername");
        }
        if self.tls == TlsMode::None && !self.allow_plaintext {
            return Err("Unverschlüsselte Verbindungen brauchen eine ausdrückliche Bestätigung");
        }
        if let Some(folder) = &self.start_folder {
            if !folder.starts_with('/') || folder.chars().any(char::is_control) {
                return Err("Ungültiger Startordner");
            }
        }
        Ok(())
    }
}

/// Connection profile for a WebDAV server (what the settings keep; no password in here).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebdavProfile {
    /// Name shown to the user.
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Empty = anonymous.
    pub user: String,
    /// Base path of the share on the server, e.g. `/dav` (default `/`).
    pub base_path: String,
    /// `true` = https (the default), `false` = http (needs `allow_plaintext`).
    pub tls: bool,
    /// The caller's explicit confirmation of an unencrypted connection. Without it `tls: false`
    /// is refused.
    #[serde(default)]
    pub allow_plaintext: bool,
    /// Folder shown after connecting; `None` = `/` below the base path.
    #[serde(default)]
    pub start_folder: Option<String>,
}

impl WebdavProfile {
    pub fn default_port(tls: bool) -> u16 {
        if tls {
            443
        } else {
            80
        }
    }

    /// Checks the profile before any network use.
    pub fn validate(&self) -> Result<(), &'static str> {
        // Strict host check: the host becomes the server part of the request URL, next to the
        // credentials, so userinfo (`@`), ports, paths, query and fragment must be rejected or the
        // Authorization header could be sent to another origin.
        if !valid_webdav_host(&self.host) {
            return Err("Ungültiger Servername");
        }
        if self.port == 0 {
            return Err("Ungültiger Port");
        }
        if self.user.len() > 256 || self.user.chars().any(char::is_control) {
            return Err("Ungültiger Benutzername");
        }
        if !self.tls && !self.allow_plaintext {
            return Err("Unverschlüsselte Verbindungen brauchen eine ausdrückliche Bestätigung");
        }
        if !self.base_path.starts_with('/') || self.base_path.chars().any(char::is_control) {
            return Err("Ungültiger Basispfad");
        }
        if let Some(folder) = &self.start_folder {
            if !folder.starts_with('/') || folder.chars().any(char::is_control) {
                return Err("Ungültiger Startordner");
            }
        }
        Ok(())
    }
}

/// How an SFTP login proves who the user is.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SftpAuth {
    /// The secret handed to `connect` is the password.
    Password,
    /// The secret handed to `connect` is the passphrase of this key file (empty = none).
    /// OpenSSH private key format (also PEM). There is no SSH agent support.
    KeyFile { path: String },
}

/// Connection profile for an SFTP server (what the settings keep; no password in here).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SftpProfile {
    #[serde(default)]
    pub transport: SshTransport,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SftpAuth,
    #[serde(default)]
    pub start_folder: Option<String>,
}

impl SftpProfile {
    pub const DEFAULT_PORT: u16 = 22;

    /// Checks the profile before any network use.
    pub fn validate(&self) -> Result<(), &'static str> {
        let openssh = self.transport == SshTransport::Openssh;
        if (openssh && !valid_ssh_endpoint(&self.host))
            || self.host.is_empty()
            || self.host.len() > if openssh { 255 } else { 253 }
            || self
                .host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '/')
        {
            return Err("Ungültiger Servername");
        }
        if self.port == 0 && !openssh {
            return Err("Ungültiger Port");
        }
        if (!openssh && self.user.is_empty())
            || self.user.len() > 256
            || self.user.chars().any(char::is_control)
            || (openssh && !self.user.is_empty() && !valid_ssh_endpoint(&self.user))
        {
            return Err("Ungültiger Benutzername");
        }
        if let SftpAuth::KeyFile { path } = &self.auth {
            if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err("Ungültiger Pfad der Schlüsseldatei");
            }
        }
        if let Some(folder) = &self.start_folder {
            if !folder.starts_with('/') || folder.chars().any(char::is_control) {
                return Err("Ungültiger Startordner");
            }
        }
        Ok(())
    }
}

/// Everything an SFTP transport needs besides the password / passphrase.
#[derive(Clone, Debug)]
pub struct SftpConnectParams {
    pub profile: SftpProfile,
    pub timeouts: Timeouts,
    pub limits: Limits,
}

impl SftpConnectParams {
    pub fn new(profile: SftpProfile) -> Self {
        Self {
            profile,
            timeouts: Timeouts::default(),
            limits: Limits::default(),
        }
    }
}

/// Everything a transport needs besides the password.
#[derive(Clone, Debug)]
pub struct ConnectParams {
    pub profile: FtpProfile,
    pub timeouts: Timeouts,
    pub limits: Limits,
}

impl ConnectParams {
    pub fn new(profile: FtpProfile) -> Self {
        Self {
            profile,
            timeouts: Timeouts::default(),
            limits: Limits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> FtpProfile {
        FtpProfile {
            name: "x".into(),
            host: "ftp.example.org".into(),
            port: 21,
            user: "u".into(),
            tls: TlsMode::Explicit,
            allow_plaintext: false,
            start_folder: None,
        }
    }

    #[test]
    fn plaintext_needs_the_explicit_flag_and_bad_values_are_refused() {
        assert!(profile().validate().is_ok());
        let mut p = profile();
        p.tls = TlsMode::None;
        assert!(p.validate().is_err());
        p.allow_plaintext = true;
        assert!(p.validate().is_ok());
        for mutate in [
            (|p: &mut FtpProfile| p.host = String::new()) as fn(&mut FtpProfile),
            |p| p.host = "a b".into(),
            |p| p.host = "a\r\nb".into(),
            |p| p.port = 0,
            |p| p.user = "u\u{1}".into(),
            |p| p.start_folder = Some("rel".into()),
            |p| p.start_folder = Some("/a\nb".into()),
        ] {
            let mut p = profile();
            mutate(&mut p);
            assert!(p.validate().is_err());
        }
        assert_eq!(FtpProfile::default_port(TlsMode::Implicit), 990);
        assert_eq!(FtpProfile::default_port(TlsMode::Explicit), 21);
    }
}

#[cfg(test)]
mod ssh_profile_tests {
    use super::*;
    #[test]
    fn legacy_serde_and_openssh_inheritance() {
        let mut profile: SftpProfile = serde_json::from_str(
            r#"{"name":"old","host":"host","port":22,"user":"u","auth":{"kind":"password"}}"#,
        )
        .unwrap();
        assert_eq!(profile.transport, SshTransport::Builtin);
        profile.port = 0;
        profile.user.clear();
        assert!(profile.validate().is_err());
        profile.transport = SshTransport::Openssh;
        assert!(profile.validate().is_ok());
        for host in ["-oProxyCommand=x", "a b", "a/b", "!host", "*"] {
            profile.host = host.into();
            assert!(profile.validate().is_err());
        }
        profile.host = "nas".into();
        profile.user = "-bad".into();
        assert!(profile.validate().is_err());
    }

    fn webdav() -> WebdavProfile {
        WebdavProfile {
            name: "dav".into(),
            host: "cloud.example.org".into(),
            port: 443,
            user: "u".into(),
            base_path: "/dav".into(),
            tls: true,
            allow_plaintext: false,
            start_folder: Some("/docs".into()),
        }
    }

    #[test]
    fn webdav_profile_validates_and_refuses_bad_values() {
        assert!(webdav().validate().is_ok());
        assert_eq!(WebdavProfile::default_port(true), 443);
        assert_eq!(WebdavProfile::default_port(false), 80);
        let mut p = webdav();
        p.tls = false;
        assert!(p.validate().is_err(), "plain http needs the explicit flag");
        p.allow_plaintext = true;
        assert!(p.validate().is_ok());
        for mutate in [
            (|p: &mut WebdavProfile| p.host.clear()) as fn(&mut WebdavProfile),
            |p| p.host = "a/b".into(),
            |p| p.host = "a".repeat(254),
            |p| p.port = 0,
            |p| p.base_path = "dav".into(),
            |p| p.user = "u\n".replace("\\n", "\n") + "\u{1}",
            |p| p.start_folder = Some("docs".into()),
        ] {
            let mut p = webdav();
            mutate(&mut p);
            assert!(p.validate().is_err());
        }
    }
}
