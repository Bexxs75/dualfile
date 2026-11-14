use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Die Datei oder der Ordner wurde nicht gefunden: {0}")]
    NotFound(PathBuf),
    #[error("Die Datei oder der Ordner ist bereits vorhanden: {0}")]
    Exists(PathBuf),
    #[error("Zugriff verweigert: {0}")]
    PermissionDenied(PathBuf),
    #[error("Der Pfad ist kein Ordner: {0}")]
    NotADirectory(PathBuf),
    #[error("Der Pfad ist ein Ordner: {0}")]
    IsADirectory(PathBuf),
    #[error("Der Ordner ist nicht leer: {0}")]
    DirectoryNotEmpty(PathBuf),
    #[error("Der Eintrag hat sich seit der Planung geändert und wurde nicht gelöscht: {0}")]
    ChangedSincePlan(PathBuf),
    #[error("Der Löschplan ist unvollständig (zu tief, zu viele Einträge, nicht lesbar oder abgebrochen); es wurde nichts gelöscht")]
    IncompletePlan,
    #[error("Der Name ist ungültig: {0}")]
    InvalidName(String),
    #[error("Der Eintrag blieb unter dem Zwischennamen stehen und konnte nicht zurückbenannt werden: {0}")]
    RenameLeftover(PathBuf),
    #[error("Der Pfad ist ungültig: {0}")]
    InvalidPath(String),
    #[error("Der Zielordner liegt innerhalb der Quelle")]
    DestinationInsideSource,
    #[error("Quelle und Ziel sind dieselbe Datei")]
    SameFile,
    #[error("Ungültige Eingabe: {0}")]
    InvalidInput(&'static str),
    #[error("Der Vorgang wurde abgebrochen")]
    Cancelled,
    #[error("Diese Funktion wird nicht unterstützt: {0}")]
    Unsupported(&'static str),
    /// Archive-specific problem with a fixed, user-readable message.
    #[error("{0}")]
    Archive(&'static str),
    #[error("Das Dateisystem unterstützt sicheres Anlegen ohne Ersetzen nicht")]
    NoReplaceUnsupported,
    #[error("Die Größe stimmt nach dem Übertragen nicht: {0}")]
    SizeMismatch(PathBuf),
    #[error("Die Prüfsumme stimmt nach dem Übertragen nicht überein")]
    VerifyFailed,
    /// A server connection is unknown, was closed or expired (never opened by this process).
    #[error("Die Verbindung ist getrennt oder nicht mehr vorhanden")]
    Disconnected,
    /// A problem reported by a server back end. The text is built by the back end from fixed
    /// phrases and sanitized server replies; it never contains credentials.
    #[error("Serverfehler: {0}")]
    Remote(String),
    /// A failure of a plugin process (time-out, crash, locked, ...). `kind` is the UI error
    /// kind, `message` a fixed phrase built by the plugin layer; text a plugin sent is never
    /// part of it.
    #[error("{message}")]
    Plugin { kind: &'static str, message: String },
    #[error("{op} fehlgeschlagen für {path}: {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn from_io(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Error {
    let path = path.into();
    match source.kind() {
        io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::NotConnected
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::TimedOut => Error::Disconnected,
        io::ErrorKind::NotFound => Error::NotFound(path),
        io::ErrorKind::AlreadyExists => Error::Exists(path),
        io::ErrorKind::PermissionDenied => Error::PermissionDenied(path),
        io::ErrorKind::NotADirectory => Error::NotADirectory(path),
        io::ErrorKind::IsADirectory => Error::IsADirectory(path),
        _ => Error::Io { op, path, source },
    }
}
