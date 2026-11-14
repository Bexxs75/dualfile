//! Errors of the plugin layer. Every message is a fixed phrase; text that a plugin sends is
//! never part of a message (it is kept apart in `Diagnostic`, bounded and cleaned, for a
//! diagnosis view that shows it as plain text).

use std::fmt;

/// Longest plugin text kept in a `Diagnostic`, in bytes.
pub const MAX_DIAGNOSTIC_BYTES: usize = 512;

/// Plugin-supplied text, cleaned (control characters replaced) and bounded. It is data for a
/// diagnosis view only: never a path, a command or markup, and never part of an error message.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Diagnostic(String);

impl Diagnostic {
    pub fn new(raw: &str) -> Self {
        let mut text = String::with_capacity(raw.len().min(MAX_DIAGNOSTIC_BYTES));
        for ch in raw.chars() {
            let ch = if ch.is_control() { '\u{fffd}' } else { ch };
            if text.len() + ch.len_utf8() > MAX_DIAGNOSTIC_BYTES {
                break;
            }
            text.push(ch);
        }
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Diagnostic({:?})", self.0)
    }
}

/// The error kinds a plugin may report (a closed set; anything else is a protocol error).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    NotFound,
    Exists,
    PermissionDenied,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    Unsupported,
    Invalid,
    TooLarge,
    Other,
}

impl ErrorCode {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "not_found" => Self::NotFound,
            "exists" => Self::Exists,
            "permission_denied" => Self::PermissionDenied,
            "not_a_directory" => Self::NotADirectory,
            "is_a_directory" => Self::IsADirectory,
            "not_empty" => Self::NotEmpty,
            "unsupported" => Self::Unsupported,
            "invalid" => Self::Invalid,
            "too_large" => Self::TooLarge,
            "other" => Self::Other,
            _ => return None,
        })
    }

    fn phrase(self) -> &'static str {
        match self {
            Self::NotFound => "nicht gefunden",
            Self::Exists => "bereits vorhanden",
            Self::PermissionDenied => "Zugriff verweigert",
            Self::NotADirectory => "kein Ordner",
            Self::IsADirectory => "ist ein Ordner",
            Self::NotEmpty => "Ordner nicht leer",
            Self::Unsupported => "nicht unterstützt",
            Self::Invalid => "ungültige Anfrage",
            Self::TooLarge => "zu groß",
            Self::Other => "sonstiger Fehler",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Das Plugin-Manifest ist ungültig: {0}")]
    Manifest(&'static str),
    #[error("Das Plugin ist nicht installiert")]
    NotInstalled,
    #[error("Das Plugin-Paket ist unsicher: {0}")]
    UnsafePackage(&'static str),
    #[error("Das Plugin ist gesperrt: Dateien oder Manifest weichen vom bestätigten Stand ab")]
    Locked,
    #[error("Plugins werden nicht als Administrator gestartet")]
    Root,
    #[error("Die Sandbox ist auf diesem System nicht vollständig herstellbar; das Plugin wird nicht gestartet")]
    SandboxUnavailable,
    #[error("Das Plugin konnte nicht gestartet werden")]
    Spawn,
    #[error("Das Plugin hat nicht rechtzeitig geantwortet")]
    Timeout,
    #[error("Das Plugin hat ungültig geantwortet: {0}")]
    Protocol(&'static str),
    #[error("Das Plugin wurde unerwartet beendet")]
    Crashed,
    #[error("Es laufen bereits zu viele Plugin-Prozesse")]
    TooManyProcesses,
    #[error("Das Plugin ist nach wiederholten Fehlern deaktiviert")]
    Disabled,
    #[error("Das Plugin ist nach einem Fehler kurz gesperrt; bitte gleich noch einmal versuchen")]
    Backoff,
    #[error("Das Plugin kann diese Funktion nicht: {0}")]
    Unsupported(&'static str),
    #[error("Das Plugin meldet einen Fehler ({})", .code.phrase())]
    Reported {
        code: ErrorCode,
        diagnostic: Diagnostic,
    },
}

impl PluginError {
    /// The cleaned text the plugin sent along with an error, for a diagnosis view.
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Reported { diagnostic, .. } if !diagnostic.is_empty() => {
                Some(diagnostic.as_str())
            }
            _ => None,
        }
    }

    /// The kind the UI sees (`CoreError.kind`); a closed set, the text of the error is fixed.
    pub fn ui_kind(&self) -> &'static str {
        match self {
            Self::Manifest(_) | Self::UnsafePackage(_) => "pluginUnsafe",
            Self::NotInstalled => "pluginNotInstalled",
            Self::Locked => "pluginLocked",
            Self::SandboxUnavailable => "pluginSandboxUnavailable",
            Self::Timeout => "pluginTimeout",
            Self::Protocol(_) => "pluginProtocol",
            Self::Crashed => "pluginCrashed",
            Self::TooManyProcesses | Self::Backoff => "pluginBusy",
            Self::Disabled => "pluginDisabled",
            Self::Unsupported(_) => "unsupported",
            Self::Root | Self::Spawn => "pluginFailed",
            Self::Reported { code, .. } => match code {
                ErrorCode::NotFound => "notFound",
                ErrorCode::Exists => "exists",
                ErrorCode::PermissionDenied => "permission",
                ErrorCode::NotADirectory => "notDirectory",
                ErrorCode::IsADirectory | ErrorCode::NotEmpty => "io",
                ErrorCode::Unsupported => "unsupported",
                ErrorCode::Invalid => "invalidInput",
                ErrorCode::TooLarge => "pluginTooLarge",
                ErrorCode::Other => "pluginFailed",
            },
        }
    }

    /// True if the failure says the process or the connection to it is broken (as opposed to a
    /// regular error answer): these count towards the automatic deactivation.
    pub fn is_process_failure(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Protocol(_) | Self::Crashed | Self::Spawn
        )
    }
}

pub type PluginResult<T> = std::result::Result<T, PluginError>;
