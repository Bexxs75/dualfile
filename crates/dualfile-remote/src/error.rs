//! Errors of the FTP layer and their mapping onto the core's `Error`. Every text is built
//! from fixed phrases and sanitized server replies, never from credentials.

use crate::trust::TrustError;
use dualfile_core::Error;
use std::path::PathBuf;

/// Longest server reply text kept in an error.
const MAX_REPLY_TEXT: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("smbSecurity")]
    SmbSecurity,
    #[error("smbUnreachable")]
    SmbUnreachable,
    #[error("smbShareMissing")]
    SmbShareMissing,
    #[error("smbFailure")]
    SmbFailure,
    #[error("smbAuth")]
    SmbAuth,
    #[error("OpenSSH (ssh) wurde nicht gefunden")]
    OpensshMissing,
    #[error("SSH-Hostschlüssel abgelehnt")]
    HostKeyRejected,
    #[error("Anmeldung fehlgeschlagen: Schlüssel oder ssh-agent nötig")]
    AuthFailed,
    #[error("SSH-Server nicht erreichbar")]
    Unreachable,
    #[error("SFTP-Subsystem nicht verfügbar")]
    SubsystemMissing,
    /// The connection could not be made or broke down (`Connection` text is sanitized).
    #[error("Verbindungsfehler: {0}")]
    Connection(String),
    #[error("Zeitüberschreitung")]
    Timeout,
    /// The server refused the login. The reply text is not shown (it may echo input).
    #[error("Anmeldung fehlgeschlagen")]
    Auth,
    /// The server refused a command (`code` is the three digit reply code).
    #[error("Der Server meldet {code}: {text}")]
    Reply { code: u16, text: String },
    #[error("Protokollfehler: {0}")]
    Protocol(String),
    #[error("TLS-Fehler: {0}")]
    Tls(String),
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// A command argument contained characters that could inject further commands.
    #[error("Unzulässiges Zeichen in einem Befehl")]
    Rejected,
    #[error("Zu große Antwort des Servers: {0}")]
    TooLarge(&'static str),
    #[error("Nicht unterstützt: {0}")]
    Unsupported(&'static str),
    /// An SFTP status answer (`SSH_FXP_STATUS`) other than success. The text is sanitized.
    #[error("Der Server meldet: {code}{}", status_suffix(text))]
    Status { code: SftpStatus, text: String },
}

/// The SFTP status codes (protocol version 3) that matter to the file-system layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SftpStatus {
    NoSuchFile,
    PermissionDenied,
    /// The catch-all code: servers also use it for "exists", "not empty" and similar.
    Failure,
    BadMessage,
    NoConnection,
    ConnectionLost,
    OpUnsupported,
}

impl std::fmt::Display for SftpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoSuchFile => "Nicht gefunden",
            Self::PermissionDenied => "Zugriff verweigert",
            Self::Failure => "Fehler",
            Self::BadMessage => "Ungültige Nachricht",
            Self::NoConnection => "Keine Verbindung",
            Self::ConnectionLost => "Verbindung verloren",
            Self::OpUnsupported => "Nicht unterstützt",
        })
    }
}

fn status_suffix(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!(" ({text})")
    }
}

pub type TResult<T> = Result<T, TransportError>;

/// Removes control characters and cuts `text` to `max` characters.
pub fn sanitize_text(text: &str, max: usize) -> String {
    let mut clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    clean.truncate(clean.trim_end().len());
    clean
}

impl TransportError {
    pub fn status(code: SftpStatus, text: &str) -> Self {
        Self::Status {
            code,
            text: sanitize_text(text, MAX_REPLY_TEXT),
        }
    }

    pub fn reply(code: u16, text: &str) -> Self {
        Self::Reply {
            code,
            text: sanitize_text(text, MAX_REPLY_TEXT),
        }
    }

    /// True if the control connection is unusable afterwards (a new one is needed).
    pub fn is_connection_lost(&self) -> bool {
        matches!(
            self,
            Self::Connection(_)
                | Self::SmbUnreachable
                | Self::SmbSecurity
                | Self::Timeout
                | Self::Protocol(_)
                | Self::Tls(_)
                | Self::OpensshMissing
                | Self::HostKeyRejected
                | Self::AuthFailed
                | Self::Unreachable
                | Self::SubsystemMissing
        ) || matches!(self, Self::Reply { code: 421, .. })
            || matches!(
                self,
                Self::Status {
                    code: SftpStatus::NoConnection | SftpStatus::ConnectionLost,
                    ..
                }
            )
    }

    /// Replaces occurrences of `secret` in the texts of this error.
    pub fn scrubbed(self, secret: &crate::secret::Secret) -> Self {
        match self {
            Self::Connection(t) => Self::Connection(secret.scrub(&t)),
            Self::Reply { code, text } => Self::Reply {
                code,
                text: secret.scrub(&text),
            },
            Self::Protocol(t) => Self::Protocol(secret.scrub(&t)),
            Self::Tls(t) => Self::Tls(secret.scrub(&t)),
            Self::Status { code, text } => Self::Status {
                code,
                text: secret.scrub(&text),
            },
            other => other,
        }
    }

    /// The core error for a failed operation on `path`. File-system like replies keep their
    /// meaning (not found, exists, permission denied); everything else becomes a server error.
    pub fn into_core(self, path: &std::path::Path) -> Error {
        let p = || PathBuf::from(path);
        match &self {
            Self::Reply { code: 550, text } => {
                let lower = text.to_lowercase();
                if lower.contains("exist")
                    && !lower.contains("not exist")
                    && !lower.contains("no such")
                {
                    Error::Exists(p())
                } else if lower.contains("permission") || lower.contains("denied") {
                    Error::PermissionDenied(p())
                } else if lower.contains("not empty") {
                    Error::DirectoryNotEmpty(p())
                } else if lower.contains("not a dir") {
                    Error::NotADirectory(p())
                } else {
                    Error::NotFound(p())
                }
            }
            Self::Reply { code: 553, .. } => Error::PermissionDenied(p()),
            Self::Reply {
                code: 530 | 532, ..
            }
            | Self::Auth
            | Self::AuthFailed => Error::PermissionDenied(p()),
            Self::Reply { code: 421, .. }
            | Self::Connection(_)
            | Self::Timeout
            | Self::Unreachable
            | Self::SmbUnreachable => Error::Disconnected,
            Self::Status { code, .. } => match code {
                SftpStatus::NoSuchFile => Error::NotFound(p()),
                SftpStatus::PermissionDenied => Error::PermissionDenied(p()),
                SftpStatus::OpUnsupported => Error::Unsupported("Vorgang vom Server"),
                SftpStatus::NoConnection | SftpStatus::ConnectionLost => Error::Disconnected,
                SftpStatus::Failure | SftpStatus::BadMessage => Error::Remote(self.to_string()),
            },
            Self::Rejected => Error::InvalidPath("Ungültiger Pfad auf dem Server".into()),
            Self::Unsupported(what) => Error::Unsupported(what),
            _ => Error::Remote(self.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_loss_maps_to_disconnected() {
        for error in [
            TransportError::Connection("closed".into()),
            TransportError::Timeout,
            TransportError::Unreachable,
            TransportError::SmbUnreachable,
            TransportError::reply(421, "closed"),
            TransportError::status(SftpStatus::NoConnection, ""),
            TransportError::status(SftpStatus::ConnectionLost, ""),
        ] {
            assert!(matches!(
                error.into_core(std::path::Path::new("/x")),
                Error::Disconnected
            ));
        }
        assert!(matches!(
            TransportError::AuthFailed.into_core(std::path::Path::new("/x")),
            Error::PermissionDenied(_)
        ));
    }

    #[test]
    fn texts_are_sanitized_and_bounded() {
        let long = format!("a\r\nb\u{0}{}", "x".repeat(500));
        let text = sanitize_text(&long, 50);
        assert!(!text.contains('\r') && !text.contains('\n') && !text.contains('\0'));
        assert_eq!(text.chars().count(), 50);
        assert_eq!(sanitize_text("ok  ", 10), "ok");
    }

    #[test]
    fn replies_map_onto_core_errors() {
        let path = std::path::Path::new("/x");
        let map = |code, text: &str| TransportError::reply(code, text).into_core(path);
        assert!(matches!(
            map(550, "No such file or directory"),
            Error::NotFound(_)
        ));
        assert!(matches!(map(550, "File exists"), Error::Exists(_)));
        assert!(matches!(
            map(550, "Permission denied"),
            Error::PermissionDenied(_)
        ));
        assert!(matches!(
            map(550, "Directory not empty"),
            Error::DirectoryNotEmpty(_)
        ));
        assert!(matches!(map(421, "Timeout"), Error::Disconnected));
        assert!(matches!(map(500, "what"), Error::Remote(_)));
        assert!(TransportError::reply(421, "x").is_connection_lost());
        assert!(!TransportError::reply(550, "x").is_connection_lost());
    }
}
