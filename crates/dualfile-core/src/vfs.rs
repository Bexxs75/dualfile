use crate::error::{from_io, Error, Result};
pub use crate::resume::{ResumableWrite, SourceIdentity};
use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
static FORCE_LINK_FALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Scheme {
    Local,
    Archive,
    /// A folder on an FTP/FTPS server (`ftp:<connection id>!<path>` at the IPC boundary).
    Ftp,
    /// A folder on an SFTP server (`sftp:<connection id>!<path>`).
    Sftp,
    /// A folder on an SMB2/3 connection (`smb:<connection id>!<path>`).
    Smb,
    /// A folder on an NFSv3 export (`nfs:<connection id>!<path>`).
    Nfs,
    /// A folder on a WebDAV server (`webdav:<connection id>!<path>`).
    Webdav,
    /// A folder in a file system served by a plugin process (`plugin:<plugin id>!<path>`). The
    /// plugin id takes the place of the connection id; the path is the plugin's own path.
    Plugin,
}

impl Scheme {
    /// True for locations behind an external back end (server or plugin): the path is never
    /// a local path, there is no watcher, and nothing is synchronized, packed or trashed
    /// into it.
    pub fn is_remote(self) -> bool {
        matches!(
            self,
            Scheme::Ftp
                | Scheme::Sftp
                | Scheme::Smb
                | Scheme::Nfs
                | Scheme::Webdav
                | Scheme::Plugin
        )
    }

    /// True for locations on an FTP/FTPS/SFTP server (a connection id names the server).
    pub fn is_server(self) -> bool {
        matches!(
            self,
            Scheme::Ftp | Scheme::Sftp | Scheme::Smb | Scheme::Nfs | Scheme::Webdav
        )
    }
}

/// Maximum number of archives nested inside each other (archive in archive in archive).
pub const MAX_ARCHIVE_NESTING: usize = 3;
/// Maximum number of path components inside an archive.
pub const MAX_ARCHIVE_PATH_DEPTH: usize = 128;
/// Maximum length of one path component inside an archive, in bytes.
pub const MAX_ARCHIVE_NAME_LEN: usize = 255;
const ARCHIVE_PREFIX: &str = "archive:";
const FTP_PREFIX: &str = "ftp:";
const SMB_PREFIX: &str = "smb:";
const NFS_PREFIX: &str = "nfs:";
const WEBDAV_PREFIX: &str = "webdav:";
const SFTP_PREFIX: &str = "sftp:";
const PLUGIN_PREFIX: &str = "plugin:";
/// Longest connection id (characters `a-z`, `0-9`, `-`).
pub const MAX_CONNECTION_ID_LEN: usize = 32;
/// Maximum number of path components of a path on a server.
pub const MAX_REMOTE_PATH_DEPTH: usize = 128;
/// Maximum length of one path component on a server, in bytes.
pub const MAX_REMOTE_NAME_LEN: usize = 255;
/// Maximum length of a whole path on a server, in bytes.
pub const MAX_REMOTE_PATH_LEN: usize = 4096;

/// A location: a local absolute path, an entry inside an archive file, or a path on a server
/// connection (`Scheme::Ftp`/`Scheme::Sftp`, see `conn_id()`).
///
/// For the archive scheme `path()` is the path *inside* the archive (always starting with
/// `/`, `/` is the archive root) and `outer()` is the location of the archive file itself,
/// which may again be inside an archive. `path()` of an archive or server location must never
/// be used to touch the local file system; for server locations it is the path on the server
/// (always starting with `/`) and `outer()` is `None`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct VPath {
    scheme: Scheme,
    path: PathBuf,
    outer: Option<Box<VPath>>,
    /// Connection id of a server location (`[a-z0-9-]`, at most 32 characters).
    conn: Option<Box<str>>,
}

/// A connection id is assigned by the connection manager, never typed by the user. Plugin ids
/// (`plugin:<id>!<path>`) follow the same rule: `[a-z0-9-]{1,32}`.
pub fn is_valid_connection_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CONNECTION_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Normalizes a path on a server: starts with `/`, no `.`/`..` components, no NUL, backslash
/// or control characters (those would inject commands into the control connection), bounded
/// component length, depth and total length. Empty components are dropped.
fn normalize_remote_path(path: &str, scheme: Scheme) -> Result<PathBuf> {
    let bad = || {
        Error::InvalidPath(if scheme == Scheme::Plugin {
            "Ungültiger Pfad im Plugin".into()
        } else {
            "Ungültiger Pfad auf dem Server".into()
        })
    };
    if !path.starts_with('/') || path.len() > MAX_REMOTE_PATH_LEN {
        return Err(bad());
    }
    let mut parts = Vec::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if part == "."
            || part == ".."
            || part.len() > MAX_REMOTE_NAME_LEN
            || part.contains('\\')
            || part.chars().any(char::is_control)
        {
            return Err(bad());
        }
        parts.push(part);
    }
    if parts.len() > MAX_REMOTE_PATH_DEPTH {
        return Err(bad());
    }
    Ok(PathBuf::from(format!("/{}", parts.join("/"))))
}

/// Normalizes a path inside an archive: starts with `/`, no `.`/`..`/NUL/backslash
/// components, bounded component length and depth. Empty components (`//`, trailing `/`)
/// are dropped.
fn normalize_archive_inner(inner: &str) -> Result<PathBuf> {
    let bad = || Error::InvalidPath("Ungültiger Pfad im Archiv".into());
    if !inner.starts_with('/') {
        return Err(bad());
    }
    let mut parts = Vec::new();
    for part in inner.split('/').filter(|part| !part.is_empty()) {
        if part == "."
            || part == ".."
            || part.contains('\0')
            || part.contains('\\')
            || part.len() > MAX_ARCHIVE_NAME_LEN
        {
            return Err(bad());
        }
        parts.push(part);
    }
    if parts.len() > MAX_ARCHIVE_PATH_DEPTH {
        return Err(bad());
    }
    Ok(PathBuf::from(format!("/{}", parts.join("/"))))
}

fn encode_outer(text: &str) -> String {
    text.replace('%', "%25").replace('!', "%21")
}

fn decode_outer(text: &str) -> Result<String> {
    let bad = || Error::InvalidPath("Ungültige Kodierung im Archivpfad".into());
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                match bytes.get(index + 1..index + 3) {
                    Some(b"25") => out.push(b'%'),
                    Some(b"21") => out.push(b'!'),
                    _ => return Err(bad()),
                }
                index += 3;
            }
            b'!' => return Err(bad()),
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| bad())
}

impl VPath {
    pub fn local(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(Error::InvalidPath(path.display().to_string()));
        }
        if path.to_string_lossy().contains('\0') {
            return Err(Error::InvalidPath(
                "Der Pfad enthält ein ungültiges Zeichen".into(),
            ));
        }
        // Reject parent components instead of resolving them, so paths never hide traversal.
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(Error::InvalidPath(path.display().to_string()));
        }
        Ok(Self {
            scheme: Scheme::Local,
            path: path.to_path_buf(),
            outer: None,
            conn: None,
        })
    }

    /// A path on the FTP/FTPS connection `conn` (see `is_valid_connection_id`).
    pub fn ftp(conn: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Ftp, conn, path)
    }

    /// A path relative to an SMB share (or server share browser).
    pub fn smb(conn: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Smb, conn, path)
    }

    /// A path on the NFSv3 export of the connection `conn`.
    pub fn nfs(conn: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Nfs, conn, path)
    }

    /// A path on the WebDAV connection `conn`.
    pub fn webdav(conn: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Webdav, conn, path)
    }

    /// A path on the SFTP connection `conn`.
    pub fn sftp(conn: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Sftp, conn, path)
    }

    /// A path in the file system of the plugin `plugin_id` (`[a-z0-9-]{1,32}`).
    pub fn plugin(plugin_id: &str, path: &str) -> Result<Self> {
        Self::remote(Scheme::Plugin, plugin_id, path)
    }

    fn remote(scheme: Scheme, conn: &str, path: &str) -> Result<Self> {
        if !is_valid_connection_id(conn) {
            return Err(Error::InvalidPath(if scheme == Scheme::Plugin {
                "Ungültige Plugin-Kennung".into()
            } else {
                "Ungültige Verbindungskennung".into()
            }));
        }
        Ok(Self {
            scheme,
            path: normalize_remote_path(path, scheme)?,
            outer: None,
            conn: Some(conn.into()),
        })
    }

    /// A location inside the archive file at `outer`. `inner` is the path in the archive
    /// (`/` = archive root). Rejects traversal components, too deep nesting and an
    /// `outer` that is itself an archive root.
    pub fn archive(outer: VPath, inner: &str) -> Result<Self> {
        let inner = normalize_archive_inner(inner)?;
        if outer.archive_depth() + 1 > MAX_ARCHIVE_NESTING {
            return Err(Error::InvalidPath(
                "Archive sind zu tief verschachtelt".into(),
            ));
        }
        if outer.scheme.is_remote() {
            return Err(Error::InvalidPath(
                "Archive auf Servern werden nicht unterstützt".into(),
            ));
        }
        if outer.scheme == Scheme::Archive && outer.path == Path::new("/") {
            return Err(Error::InvalidPath(
                "Der Archivpfad zeigt auf keine Datei".into(),
            ));
        }
        Ok(Self {
            scheme: Scheme::Archive,
            path: inner,
            outer: Some(Box::new(outer)),
            conn: None,
        })
    }

    /// The location `name` next to this one (same folder, same back end).
    pub fn sibling(&self, name: &str) -> Result<Self> {
        match self.parent() {
            Some(parent) => parent.join_entry(name),
            None => Err(Error::InvalidPath(self.path.display().to_string())),
        }
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }
    /// Local path, or the path inside the archive (see the type documentation).
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The connection id of a server location (`None` for local and archive locations).
    pub fn conn_id(&self) -> Option<&str> {
        self.conn.as_deref()
    }
    /// The archive file this location lies in (`None` for local paths).
    pub fn outer(&self) -> Option<&VPath> {
        self.outer.as_deref()
    }
    /// Number of archive levels (0 for a local path).
    pub fn archive_depth(&self) -> usize {
        match &self.outer {
            Some(outer) => 1 + outer.archive_depth(),
            None => 0,
        }
    }

    pub fn join(&self, name: &str) -> Result<Self> {
        crate::names::validate_name(name)?;
        self.join_entry(name)
    }

    /// Like `join`, but without the file-name rules for local paths (used for names read
    /// from a listing). Archive names are always checked.
    pub(crate) fn join_entry(&self, name: &str) -> Result<Self> {
        if let Some(conn) = self.conn_id() {
            if name.is_empty() || name.contains('/') {
                return Err(Error::InvalidName(name.to_owned()));
            }
            let joined = self.path.join(name);
            return Self::remote(self.scheme, conn, &joined.to_string_lossy());
        }
        match &self.outer {
            None => Self::local(self.path.join(name)),
            Some(outer) => {
                if name.is_empty() || name.contains('/') {
                    return Err(Error::InvalidName(name.to_owned()));
                }
                let joined = self.path.join(name);
                Self::archive((**outer).clone(), &joined.to_string_lossy())
            }
        }
    }

    /// Parent location inside the same archive; `None` at the archive root and for local
    /// roots. Leaving an archive is the caller's decision (`outer()`).
    pub fn parent(&self) -> Option<Self> {
        let parent = self.path.parent()?;
        Some(Self {
            scheme: self.scheme,
            path: parent.to_path_buf(),
            outer: self.outer.clone(),
            conn: self.conn.clone(),
        })
    }
}

/// The one place that formats a location for the UI/IPC boundary: a local path as is,
/// an archive entry as `archive:<outer, with % and ! percent-encoded>!<inner path>`, a server
/// location as `ftp:<connection id>!<path>` / `sftp:<connection id>!<path>`, a plugin location
/// as `plugin:<plugin id>!<path>`.
pub fn format_ipc_path(path: &VPath) -> String {
    if let Some(conn) = path.conn_id() {
        let prefix = match path.scheme {
            Scheme::Ftp => FTP_PREFIX,
            Scheme::Smb => SMB_PREFIX,
            Scheme::Nfs => NFS_PREFIX,
            Scheme::Webdav => WEBDAV_PREFIX,
            Scheme::Plugin => PLUGIN_PREFIX,
            _ => SFTP_PREFIX,
        };
        // The connection id cannot contain `!`, so the first `!` always ends it; the path
        // after it is taken literally (including any `!` or `%` in names).
        return format!("{prefix}{conn}!{}", path.path.to_string_lossy());
    }
    match &path.outer {
        None => path.path.to_string_lossy().into_owned(),
        Some(outer) => format!(
            "{ARCHIVE_PREFIX}{}!{}",
            encode_outer(&format_ipc_path(outer)),
            path.path.to_string_lossy()
        ),
    }
}

/// The one place that parses a location from the UI/IPC boundary (inverse of
/// `format_ipc_path`). Forged paths (`..`, NUL, bad encoding, too deep nesting) are rejected.
pub fn parse_ipc_path(text: &str) -> Result<VPath> {
    fn parse(text: &str, depth: usize) -> Result<VPath> {
        for (prefix, scheme) in [
            (FTP_PREFIX, Scheme::Ftp),
            (SFTP_PREFIX, Scheme::Sftp),
            (SMB_PREFIX, Scheme::Smb),
            (NFS_PREFIX, Scheme::Nfs),
            (WEBDAV_PREFIX, Scheme::Webdav),
            (PLUGIN_PREFIX, Scheme::Plugin),
        ] {
            if let Some(rest) = text.strip_prefix(prefix) {
                let (conn, path) = rest
                    .split_once('!')
                    .ok_or_else(|| Error::InvalidPath("Ungültiger Serverpfad".into()))?;
                return VPath::remote(scheme, conn, path);
            }
        }
        let Some(rest) = text.strip_prefix(ARCHIVE_PREFIX) else {
            return VPath::local(text);
        };
        if depth >= MAX_ARCHIVE_NESTING {
            return Err(Error::InvalidPath(
                "Archive sind zu tief verschachtelt".into(),
            ));
        }
        let (encoded_outer, inner) = rest
            .split_once('!')
            .ok_or_else(|| Error::InvalidPath("Ungültiger Archivpfad".into()))?;
        let outer = parse(&decode_outer(encoded_outer)?, depth + 1)?;
        VPath::archive(outer, inner)
    }
    parse(text, 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
    pub hidden: bool,
    pub link_target: Option<String>,
    pub broken_link: bool,
    /// A per-entry inspection failure does not prevent other entries from being listed.
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    NoReplace,
    Replace,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplaceMode {
    NoReplace,
    Replace,
}

pub trait WriteFinish: Write + Send {
    fn commit(&mut self) -> Result<()>;
}

/// What the lister needs to know about a file before reading it.
#[derive(Clone, Copy, Debug)]
pub struct FileStat {
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub can_trash: bool,
    pub can_rename_in_place: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SpaceFree {
    pub available: u64,
    pub total: u64,
}

pub trait Vfs: Send + Sync {
    fn space_free(&self, _path: &VPath) -> Option<SpaceFree> {
        None
    }

    fn list(&self, p: &VPath) -> Result<Vec<Entry>>;
    fn stat(&self, p: &VPath) -> Result<Entry>;
    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>>;
    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>>;
    fn mkdir(&self, p: &VPath) -> Result<()>;
    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()>;
    fn read_link(&self, p: &VPath) -> Result<String>;
    fn create_symlink(&self, target: &str, at: &VPath) -> Result<()>;
    fn trash(&self, p: &VPath) -> Result<()>;
    fn remove_permanent(&self, p: &VPath) -> Result<()>;
    fn capabilities(&self) -> Caps;
    /// Sets permission bits and/or modification time of an existing file or directory.
    /// Symlinks are never touched (Ok, no change). Default: not supported.
    fn set_metadata(
        &self,
        _p: &VPath,
        _mode: Option<u32>,
        _modified: Option<SystemTime>,
    ) -> Result<()> {
        Err(Error::Unsupported("Rechte und Änderungszeit setzen"))
    }
    /// Size and modification time of the regular file behind `p`; links are followed.
    /// Directories and special files (pipe, device, socket) are refused. Used by the lister.
    fn stat_file(&self, p: &VPath) -> Result<FileStat> {
        let entry = self.stat(p)?;
        match entry.kind {
            EntryKind::File => Ok(FileStat {
                size: entry.size,
                modified: entry.modified,
            }),
            EntryKind::Dir => Err(Error::IsADirectory(p.path().to_path_buf())),
            EntryKind::Symlink => Err(Error::Unsupported("Verknüpfungen folgen")),
        }
    }
    /// Reads up to `len` bytes starting at `offset` from a regular file (shorter at the end,
    /// empty behind it). Links are followed; directories and special files are refused
    /// without ever blocking. The default skips by reading, `LocalVfs` seeks.
    fn read_range(&self, p: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.stat_file(p)?;
        let io_err = |e| from_io("Datei lesen", p.path(), e);
        let mut reader = self.open_read(p)?;
        let skipped = std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())
            .map_err(io_err)?;
        let mut data = Vec::new();
        if skipped == offset {
            reader
                .take(len as u64)
                .read_to_end(&mut data)
                .map_err(io_err)?;
        }
        Ok(data)
    }
    /// Like `open_read`, starting at byte `offset` (resuming a copy). The default skips by
    /// reading; server back ends override it with a protocol restart (`REST`).
    fn open_read_from(&self, p: &VPath, offset: u64) -> Result<Box<dyn Read + Send>> {
        let mut reader = self.open_read(p)?;
        let skipped = std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())
            .map_err(|e| from_io("Datei lesen", p.path(), e))?;
        if skipped != offset {
            return Err(Error::Io {
                op: "Datei lesen",
                path: p.path().to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
            });
        }
        Ok(reader)
    }
    /// Optional: writes `p` through a partial file that survives an abort, so the transfer can
    /// continue later (see `crate::resume`). The new file only appears under its final name
    /// after `commit`, never replacing anything (no-replace semantics). Default: not supported.
    fn create_write_resumable(
        &self,
        _p: &VPath,
        _expected_source: SourceIdentity,
    ) -> Result<ResumableWrite> {
        Err(Error::Unsupported("Wiederaufnahme von Übertragungen"))
    }
    /// Removes the partial file (and its meta file) a resumable write of `p` left behind, but
    /// only if the meta file proves that Dualfile made it. Default: not supported.
    fn discard_resumable(&self, _p: &VPath) -> Result<()> {
        Err(Error::Unsupported("Wiederaufnahme von Übertragungen"))
    }
    /// For server back ends: the scheme and connection id this instance serves.
    fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
        None
    }
    /// True for the plain local file system; enables handle-based source traversal in `ops`.
    fn is_local(&self) -> bool {
        false
    }
    /// True if every changing operation is refused (archives). `ops` checks this first so a
    /// move never copies and then fails to delete.
    fn is_read_only(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalVfs;

impl LocalVfs {
    /// Opens the hidden staging file next to `p`; `commit` moves it into place.
    pub(crate) fn stage(&self, p: &VPath, mode: WriteMode) -> Result<LocalWrite> {
        let parent = p
            .path()
            .parent()
            .ok_or_else(|| Error::InvalidPath(p.path().display().to_string()))?;
        let name = p
            .path()
            .file_name()
            .ok_or_else(|| Error::InvalidPath(p.path().display().to_string()))?
            .to_string_lossy();
        let preferred = format!(".{name}.dualfile-part");
        let mut created = None;
        for attempt in 0..100 {
            let candidate = if preferred.len() <= 255 && attempt == 0 {
                parent.join(&preferred)
            } else {
                let suffix = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                parent.join(format!(".dualfile-part-{}-{suffix}", std::process::id()))
            };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&candidate) {
                Ok(file) => {
                    created = Some((candidate, file));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(from_io("Hilfsdatei anlegen", candidate, error)),
            }
        }
        let (temp, file) = created.ok_or_else(|| Error::Io {
            op: "Hilfsdatei anlegen",
            path: parent.to_path_buf(),
            source: std::io::Error::other("Kein freier Hilfsdateiname gefunden"),
        })?;
        Ok(LocalWrite {
            temp,
            target: p.path().to_path_buf(),
            file: Some(file),
            mode,
            committed: false,
            finalized: false,
        })
    }
}

impl LocalVfs {
    fn entry(path: &Path, metadata: Metadata) -> Result<Entry> {
        let file_name = path.file_name().unwrap_or_else(|| OsStr::new("/"));
        let name = file_name.to_string_lossy().into_owned();
        let kind = if metadata.file_type().is_symlink() {
            EntryKind::Symlink
        } else if metadata.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let link_target = if kind == EntryKind::Symlink {
            Some(
                fs::read_link(path)
                    .map_err(|e| from_io("Linkziel lesen", path, e))?
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        };
        let broken_link = kind == EntryKind::Symlink && fs::metadata(path).is_err();
        let entry_error = if kind == EntryKind::Dir {
            fs::read_dir(path).err().map(|error| error.to_string())
        } else {
            None
        };
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode() & 0o7777)
        };
        #[cfg(not(unix))]
        let mode = None;
        #[cfg(unix)]
        let (uid, gid) = {
            use std::os::unix::fs::MetadataExt;
            (Some(metadata.uid()), Some(metadata.gid()))
        };
        #[cfg(not(unix))]
        let (uid, gid) = (None, None);
        Ok(Entry {
            uid,
            gid,
            hidden: name.starts_with('.') && name != "." && name != "..",
            name,
            kind,
            size: if kind == EntryKind::File {
                metadata.len()
            } else {
                0
            },
            modified: metadata.modified().ok(),
            mode,
            link_target,
            broken_link,
            error: entry_error,
        })
    }
}

pub(crate) struct LocalWrite {
    temp: PathBuf,
    target: PathBuf,
    file: Option<File>,
    mode: WriteMode,
    committed: bool,
    finalized: bool,
}

impl Write for LocalWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.file.as_mut() {
            Some(file) => file.write(buf),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Der Schreibvorgang wurde bereits beendet",
            )),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Der Schreibvorgang wurde bereits beendet",
            )),
        }
    }
}

impl std::io::Seek for LocalWrite {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self.file.as_mut() {
            Some(file) => file.seek(pos),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Der Schreibvorgang wurde bereits beendet",
            )),
        }
    }
}

impl WriteFinish for LocalWrite {
    fn commit(&mut self) -> Result<()> {
        if self.committed {
            return Ok(());
        }
        if self.finalized {
            return Err(Error::Io {
                op: "Schreibvorgang abschließen",
                path: self.target.clone(),
                source: std::io::Error::other("Der Schreibvorgang wurde bereits beendet"),
            });
        }
        let result = self.commit_inner();
        if result.is_err() {
            self.finalized = true;
            self.file.take();
            let _ = fs::remove_file(&self.temp);
        }
        result
    }
}

impl LocalWrite {
    pub(crate) fn file_ref(&self) -> Option<&File> {
        self.file.as_ref()
    }
    fn commit_inner(&mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()
                .map_err(|e| from_io("Datei sichern", &self.temp, e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let permissions = if self.mode == WriteMode::Replace {
                    fs::metadata(&self.target)
                        .map(|m| m.permissions())
                        .unwrap_or_else(|_| fs::Permissions::from_mode(0o644))
                } else {
                    fs::Permissions::from_mode(0o644)
                };
                fs::set_permissions(&self.temp, permissions)
                    .map_err(|e| from_io("Dateirechte setzen", &self.temp, e))?;
            }
        }
        if self.mode == WriteMode::Replace {
            fs::rename(&self.temp, &self.target)
                .map_err(|e| from_io("Datei ersetzen", &self.target, e))?;
        } else {
            #[cfg(target_os = "linux")]
            {
                use rustix::fs::{renameat_with, RenameFlags, CWD};
                let force_fallback = {
                    #[cfg(test)]
                    {
                        FORCE_LINK_FALLBACK.load(Ordering::Relaxed)
                    }
                    #[cfg(not(test))]
                    {
                        false
                    }
                };
                let rename_result = if force_fallback {
                    Err(rustix::io::Errno::NOSYS)
                } else {
                    renameat_with(CWD, &self.temp, CWD, &self.target, RenameFlags::NOREPLACE)
                };
                match rename_result {
                    Ok(()) => {
                        self.committed = true;
                        return Ok(());
                    }
                    Err(err) if err == rustix::io::Errno::EXIST => {
                        return Err(Error::Exists(self.target.clone()))
                    }
                    Err(err)
                        if err == rustix::io::Errno::NOSYS
                            || err == rustix::io::Errno::INVAL
                            || err == rustix::io::Errno::OPNOTSUPP =>
                    {
                        // A hard link creates the destination atomically without replacing it.
                        fs::hard_link(&self.temp, &self.target).map_err(|e| {
                            if e.kind() == std::io::ErrorKind::AlreadyExists {
                                Error::Exists(self.target.clone())
                            } else {
                                Error::NoReplaceUnsupported
                            }
                        })?;
                        fs::remove_file(&self.temp)
                            .map_err(|e| from_io("Hilfsdatei entfernen", &self.temp, e))?;
                        self.committed = true;
                        return Ok(());
                    }
                    Err(err) => {
                        return Err(from_io(
                            "Datei sicher anlegen",
                            &self.target,
                            std::io::Error::from(err),
                        ))
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                fs::hard_link(&self.temp, &self.target).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Error::Exists(self.target.clone())
                    } else {
                        Error::NoReplaceUnsupported
                    }
                })?;
                fs::remove_file(&self.temp)
                    .map_err(|e| from_io("Hilfsdatei entfernen", &self.temp, e))?;
            }
        }
        self.committed = true;
        self.finalized = true;
        Ok(())
    }
}

impl Drop for LocalWrite {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed && !self.finalized {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

/// Opens a regular file for reading without ever blocking: `O_NONBLOCK` makes opening a FIFO
/// return at once, the check runs on the opened descriptor (`fstat`, so no path race between
/// check and use), and only then is `O_NONBLOCK` cleared. Links are followed.
#[cfg(unix)]
pub(crate) fn open_regular(path: &Path) -> Result<(File, Metadata)> {
    use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::NOCTTY).bits() as i32)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            // Opening a socket fails with ENXIO; same answer as for other special files.
            Some(code) if code == rustix::io::Errno::NXIO.raw_os_error() => special_file_error(),
            _ => from_io("Datei öffnen", path, e),
        })?;
    let metadata = file
        .metadata()
        .map_err(|e| from_io("Datei-Information lesen", path, e))?;
    if metadata.is_dir() {
        return Err(Error::IsADirectory(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(special_file_error());
    }
    let flags = fcntl_getfl(&file).map_err(|e| from_io("Datei öffnen", path, e.into()))?;
    fcntl_setfl(&file, flags - OFlags::NONBLOCK)
        .map_err(|e| from_io("Datei öffnen", path, e.into()))?;
    Ok((file, metadata))
}

#[cfg(not(unix))]
pub(crate) fn open_regular(path: &Path) -> Result<(File, Metadata)> {
    let file = File::open(path).map_err(|e| from_io("Datei öffnen", path, e))?;
    let metadata = file
        .metadata()
        .map_err(|e| from_io("Datei-Information lesen", path, e))?;
    if metadata.is_dir() {
        return Err(Error::IsADirectory(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(special_file_error());
    }
    Ok((file, metadata))
}

fn special_file_error() -> Error {
    Error::Unsupported("Pipes, Geräte und Sockets können nicht angezeigt werden")
}

impl Vfs for LocalVfs {
    fn space_free(&self, path: &VPath) -> Option<SpaceFree> {
        if path.scheme() != Scheme::Local {
            return None;
        }
        #[cfg(unix)]
        {
            let stat = rustix::fs::statvfs(path.path()).ok()?;
            Some(SpaceFree {
                available: stat.f_bavail.checked_mul(stat.f_frsize)?,
                total: stat.f_blocks.checked_mul(stat.f_frsize)?,
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn is_local(&self) -> bool {
        true
    }
    fn stat_file(&self, p: &VPath) -> Result<FileStat> {
        let (_, metadata) = open_regular(p.path())?;
        Ok(FileStat {
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
    fn read_range(&self, p: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let (mut file, _) = open_regular(p.path())?;
        let io_err = |e| from_io("Datei lesen", p.path(), e);
        file.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        let mut data = Vec::with_capacity(len.min(1 << 20));
        file.take(len as u64)
            .read_to_end(&mut data)
            .map_err(io_err)?;
        Ok(data)
    }
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        let iter = fs::read_dir(p.path()).map_err(|e| from_io("Ordner auflisten", p.path(), e))?;
        let mut entries = Vec::new();
        for item in iter {
            match item {
                Ok(item) => {
                    let path = item.path();
                    match fs::symlink_metadata(&path).and_then(|meta| {
                        Self::entry(&path, meta).map_err(|err| match err {
                            Error::Io { source, .. } => source,
                            _ => std::io::Error::other("Eintrag kann nicht gelesen werden"),
                        })
                    }) {
                        Ok(entry) => entries.push(entry),
                        Err(error) => entries.push(Entry {
                            uid: None,
                            gid: None,
                            name: item.file_name().to_string_lossy().into_owned(),
                            kind: EntryKind::File,
                            size: 0,
                            modified: None,
                            mode: None,
                            hidden: false,
                            link_target: None,
                            broken_link: false,
                            error: Some(error.to_string()),
                        }),
                    }
                }
                Err(error) => entries.push(Entry {
                    uid: None,
                    gid: None,
                    name: String::new(),
                    kind: EntryKind::File,
                    size: 0,
                    modified: None,
                    mode: None,
                    hidden: false,
                    link_target: None,
                    broken_link: false,
                    error: Some(error.to_string()),
                }),
            }
        }
        Ok(entries)
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        let metadata = fs::symlink_metadata(p.path())
            .map_err(|e| from_io("Datei-Information lesen", p.path(), e))?;
        Self::entry(p.path(), metadata)
    }
    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        let entry = self.stat(p)?;
        if entry.kind == EntryKind::Dir {
            return Err(Error::IsADirectory(p.path().to_path_buf()));
        }
        Ok(Box::new(
            File::open(p.path()).map_err(|e| from_io("Datei öffnen", p.path(), e))?,
        ))
    }
    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        Ok(Box::new(self.stage(p, mode)?))
    }
    fn mkdir(&self, p: &VPath) -> Result<()> {
        fs::create_dir(p.path()).map_err(|e| from_io("Ordner anlegen", p.path(), e))
    }
    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        if mode == ReplaceMode::Replace {
            return fs::rename(from.path(), to.path())
                .map_err(|e| from_io("Umbenennen", to.path(), e));
        }
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{renameat_with, RenameFlags, CWD};
            let force_fallback = {
                #[cfg(test)]
                {
                    FORCE_LINK_FALLBACK.load(Ordering::Relaxed)
                }
                #[cfg(not(test))]
                {
                    false
                }
            };
            let result = if force_fallback {
                Err(rustix::io::Errno::NOSYS)
            } else {
                renameat_with(CWD, from.path(), CWD, to.path(), RenameFlags::NOREPLACE)
            };
            match result {
                Ok(()) => Ok(()),
                Err(error) if error == rustix::io::Errno::EXIST => {
                    Err(Error::Exists(to.path().to_path_buf()))
                }
                Err(error)
                    if error == rustix::io::Errno::NOSYS
                        || error == rustix::io::Errno::INVAL
                        || error == rustix::io::Errno::OPNOTSUPP =>
                {
                    let source = fs::symlink_metadata(from.path())
                        .map_err(|e| from_io("Quelle prüfen", from.path(), e))?;
                    if source.is_dir() && !source.file_type().is_symlink() {
                        return Err(Error::NoReplaceUnsupported);
                    }
                    fs::hard_link(from.path(), to.path()).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            Error::Exists(to.path().to_path_buf())
                        } else {
                            Error::NoReplaceUnsupported
                        }
                    })?;
                    fs::remove_file(from.path())
                        .map_err(|e| from_io("Quelle nach Umbenennen entfernen", from.path(), e))
                }
                Err(error) => Err(from_io(
                    "Sicher umbenennen",
                    to.path(),
                    std::io::Error::from(error),
                )),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            fs::hard_link(from.path(), to.path())
                .map_err(|e| from_io("Sicher umbenennen", to.path(), e))?;
            fs::remove_file(from.path()).map_err(|e| from_io("Quelle entfernen", from.path(), e))
        }
    }
    fn read_link(&self, p: &VPath) -> Result<String> {
        let target = fs::read_link(p.path()).map_err(|e| from_io("Linkziel lesen", p.path(), e))?;
        Ok(target.to_string_lossy().into_owned())
    }
    fn create_symlink(&self, target: &str, at: &VPath) -> Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, at.path())
                .map_err(|e| from_io("Link anlegen", at.path(), e))
        }
        #[cfg(not(unix))]
        {
            let _ = target;
            let _ = at;
            Err(Error::Unsupported("symbolische Links"))
        }
    }
    fn trash(&self, p: &VPath) -> Result<()> {
        fs::symlink_metadata(p.path())
            .map_err(|e| from_io("Papierkorb-Eintrag prüfen", p.path(), e))?;
        trash::delete(p.path()).map_err(|error| match error {
            #[cfg(all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            ))]
            trash::Error::FileSystem { source, .. } => {
                if source.kind() == std::io::ErrorKind::Unsupported {
                    Error::Unsupported("Papierkorb")
                } else {
                    from_io("In den Papierkorb verschieben", p.path(), source)
                }
            }
            other => Error::Io {
                op: "In den Papierkorb verschieben",
                path: p.path().to_path_buf(),
                source: std::io::Error::other(other),
            },
        })
    }
    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        let meta =
            fs::symlink_metadata(p.path()).map_err(|e| from_io("Datei löschen", p.path(), e))?;
        let result = if meta.file_type().is_symlink() || meta.is_file() {
            fs::remove_file(p.path())
        } else {
            fs::remove_dir(p.path())
        };
        result.map_err(|e| {
            if e.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                Error::DirectoryNotEmpty(p.path().to_path_buf())
            } else {
                from_io("Datei löschen", p.path(), e)
            }
        })
    }
    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: true,
            can_rename_in_place: true,
        }
    }
    fn set_metadata(
        &self,
        p: &VPath,
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) -> Result<()> {
        let path = p.path();
        let to_error = |e| from_io("Rechte und Änderungszeit setzen", path, e);
        if fs::symlink_metadata(path)
            .map_err(to_error)?
            .file_type()
            .is_symlink()
        {
            return Ok(());
        }
        // Opened read-only (works for directories too); NOFOLLOW guards against a link
        // swapped in after the check above (the open then fails instead of following it).
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let file = options.open(path).map_err(to_error)?;
        // Time before mode: the handle is already open, so order only matters for clarity.
        if let Some(modified) = modified {
            file.set_modified(modified).map_err(to_error)?;
        }
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))
                .map_err(to_error)?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn vpath_requires_absolute_normalized_paths() {
        assert!(VPath::local("relative").is_err());
        assert!(VPath::local("/tmp/a/../b").is_err());
        assert!(VPath::local("/tmp/a\0b").is_err());
        assert!(VPath::local("/tmp/a").is_ok());
        assert!(VPath::local("/tmp").unwrap().join("../escape").is_err());
    }

    #[test]
    fn listing_and_stat_do_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("folder")).unwrap();
        fs::write(dir.path().join("file"), "hello").unwrap();
        fs::write(dir.path().join(".hidden"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("missing", dir.path().join("broken")).unwrap();
            let vfs = LocalVfs;
            let root = VPath::local(dir.path()).unwrap();
            let entries = vfs.list(&root).unwrap();
            let hidden = entries.iter().find(|e| e.name == ".hidden").unwrap();
            assert!(hidden.hidden);
            let link = entries.iter().find(|e| e.name == "broken").unwrap();
            assert_eq!(link.kind, EntryKind::Symlink);
            assert!(link.broken_link);
            assert_eq!(link.link_target.as_deref(), Some("missing"));
            assert_eq!(
                entries.iter().find(|e| e.name == "folder").unwrap().kind,
                EntryKind::Dir
            );
            assert_eq!(
                vfs.stat(&root.join("broken").unwrap()).unwrap().kind,
                EntryKind::Symlink
            );
        }
    }

    #[test]
    fn commit_never_replaces_and_drop_cleans_part() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = LocalVfs;
        let target = VPath::local(dir.path().join("data")).unwrap();
        fs::write(target.path(), "original").unwrap();
        let mut writer = vfs.create_write(&target, WriteMode::NoReplace).unwrap();
        writer.write_all(b"new").unwrap();
        assert!(matches!(writer.commit(), Err(Error::Exists(_))));
        assert_eq!(fs::read(target.path()).unwrap(), b"original");
        drop(writer);
        assert!(!dir.path().join(".data.dualfile-part").exists());
    }

    #[test]
    fn commit_creates_and_drop_removes_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = LocalVfs;
        let target = VPath::local(dir.path().join("data")).unwrap();
        {
            let mut writer = vfs.create_write(&target, WriteMode::NoReplace).unwrap();
            writer.write_all(b"ok").unwrap();
            writer.commit().unwrap();
        }
        assert_eq!(fs::read(target.path()).unwrap(), b"ok");
        let another = VPath::local(dir.path().join("another")).unwrap();
        drop(vfs.create_write(&another, WriteMode::NoReplace).unwrap());
        assert!(!dir.path().join(".another.dualfile-part").exists());
    }

    #[test]
    fn commit_race_removes_temp_and_cannot_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let target = VPath::local(dir.path().join("raced")).unwrap();
        let mut writer = LocalVfs
            .create_write(&target, WriteMode::NoReplace)
            .unwrap();
        writer.write_all(b"staged").unwrap();
        fs::write(target.path(), b"racer").unwrap();
        assert!(matches!(writer.commit(), Err(Error::Exists(_))));
        assert!(!dir.path().join(".raced.dualfile-part").exists());
        assert!(writer.commit().is_err());
        assert_eq!(fs::read(target.path()).unwrap(), b"racer");
    }

    #[test]
    fn explicit_replace_and_final_permissions_work() {
        let dir = tempfile::tempdir().unwrap();
        let target = VPath::local(dir.path().join("replace")).unwrap();
        fs::write(target.path(), b"old").unwrap();
        #[cfg(unix)]
        fs::set_permissions(target.path(), fs::Permissions::from_mode(0o640)).unwrap();
        let mut writer = LocalVfs.create_write(&target, WriteMode::Replace).unwrap();
        writer.write_all(b"new").unwrap();
        writer.commit().unwrap();
        assert_eq!(fs::read(target.path()).unwrap(), b"new");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(target.path()).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn long_target_uses_short_temp_name() {
        let dir = tempfile::tempdir().unwrap();
        let target = VPath::local(dir.path().join("x".repeat(255))).unwrap();
        let mut writer = LocalVfs
            .create_write(&target, WriteMode::NoReplace)
            .unwrap();
        writer.write_all(b"long").unwrap();
        writer.commit().unwrap();
        assert_eq!(fs::read(target.path()).unwrap(), b"long");
    }

    #[test]
    fn forced_link_fallback_is_no_replace() {
        let dir = tempfile::tempdir().unwrap();
        let target = VPath::local(dir.path().join("fallback")).unwrap();
        let rename_source = VPath::local(dir.path().join("rename-source")).unwrap();
        let rename_target = VPath::local(dir.path().join("rename-target")).unwrap();
        let rename_dir = VPath::local(dir.path().join("rename-dir")).unwrap();
        let rename_dir_target = VPath::local(dir.path().join("rename-dir-target")).unwrap();
        FORCE_LINK_FALLBACK.store(true, Ordering::Relaxed);
        let result = (|| {
            let mut writer = LocalVfs.create_write(&target, WriteMode::NoReplace)?;
            writer
                .write_all(b"first")
                .map_err(|e| from_io("Schreiben", target.path(), e))?;
            writer.commit()?;
            let mut collision = LocalVfs.create_write(&target, WriteMode::NoReplace)?;
            collision
                .write_all(b"second")
                .map_err(|e| from_io("Schreiben", target.path(), e))?;
            assert!(matches!(collision.commit(), Err(Error::Exists(_))));
            fs::write(rename_source.path(), b"move").unwrap();
            LocalVfs.rename(&rename_source, &rename_target, ReplaceMode::NoReplace)?;
            assert_eq!(fs::read(rename_target.path()).unwrap(), b"move");
            fs::create_dir(rename_dir.path()).unwrap();
            assert!(matches!(
                LocalVfs.rename(&rename_dir, &rename_dir_target, ReplaceMode::NoReplace),
                Err(Error::NoReplaceUnsupported)
            ));
            Ok::<(), Error>(())
        })();
        FORCE_LINK_FALLBACK.store(false, Ordering::Relaxed);
        result.unwrap();
        assert_eq!(fs::read(target.path()).unwrap(), b"first");
        assert!(!dir.path().join(".fallback.dualfile-part").exists());
    }

    #[test]
    fn rename_modes_cover_existing_files_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let from = VPath::local(dir.path().join("from")).unwrap();
        let to = VPath::local(dir.path().join("to")).unwrap();
        fs::write(from.path(), b"source").unwrap();
        fs::write(to.path(), b"destination").unwrap();
        assert!(matches!(
            LocalVfs.rename(&from, &to, ReplaceMode::NoReplace),
            Err(Error::Exists(_))
        ));
        assert!(from.path().exists());
        LocalVfs.rename(&from, &to, ReplaceMode::Replace).unwrap();
        assert_eq!(fs::read(to.path()).unwrap(), b"source");
        let folder = VPath::local(dir.path().join("folder")).unwrap();
        let renamed = VPath::local(dir.path().join("renamed")).unwrap();
        LocalVfs.mkdir(&folder).unwrap();
        // A no-replace directory rename succeeds where the filesystem supports renameat2's
        // RENAME_NOREPLACE; where it does not (some CI/overlay filesystems return EINVAL/OPNOTSUPP),
        // there is no safe fallback for a directory, so NoReplaceUnsupported is the honest answer
        // (the FORCE_LINK_FALLBACK test above exercises that branch deterministically).
        match LocalVfs.rename(&folder, &renamed, ReplaceMode::NoReplace) {
            Ok(()) => assert!(renamed.path().is_dir()),
            Err(Error::NoReplaceUnsupported) => assert!(folder.path().is_dir()),
            other => panic!("unexpected directory no-replace rename result: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn raw_read_link_preserves_relative_text() {
        let dir = tempfile::tempdir().unwrap();
        let link = VPath::local(dir.path().join("link")).unwrap();
        LocalVfs
            .create_symlink("../dualfile-no-such-target", &link)
            .unwrap();
        assert_eq!(
            LocalVfs.read_link(&link).unwrap(),
            "../dualfile-no-such-target"
        );
        assert!(LocalVfs.stat(&link).unwrap().broken_link);
    }

    #[cfg(unix)]
    #[test]
    fn permanent_removal_handles_links_and_only_empty_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = VPath::local(dir.path().join("file")).unwrap();
        fs::write(file.path(), b"x").unwrap();
        LocalVfs.remove_permanent(&file).unwrap();
        let target = VPath::local(dir.path().join("target")).unwrap();
        let link = VPath::local(dir.path().join("link")).unwrap();
        fs::write(target.path(), b"safe").unwrap();
        LocalVfs.create_symlink("target", &link).unwrap();
        LocalVfs.remove_permanent(&link).unwrap();
        assert!(target.path().exists());
        let empty = VPath::local(dir.path().join("empty")).unwrap();
        LocalVfs.mkdir(&empty).unwrap();
        LocalVfs.remove_permanent(&empty).unwrap();
        let nonempty = VPath::local(dir.path().join("nonempty")).unwrap();
        LocalVfs.mkdir(&nonempty).unwrap();
        fs::write(nonempty.path().join("child"), b"x").unwrap();
        assert!(matches!(
            LocalVfs.remove_permanent(&nonempty),
            Err(Error::DirectoryNotEmpty(_))
        ));
        assert!(nonempty.path().join("child").exists());
    }

    #[test]
    fn mkdir_reports_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = VPath::local(dir.path().join("new-dir")).unwrap();
        LocalVfs.mkdir(&path).unwrap();
        assert!(matches!(LocalVfs.mkdir(&path), Err(Error::Exists(_))));
    }

    #[test]
    fn trash_missing_path_is_not_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let missing = VPath::local(dir.path().join("absent")).unwrap();
        assert!(matches!(LocalVfs.trash(&missing), Err(Error::NotFound(_))));
    }

    #[test]
    fn trash_moves_item_into_temporary_data_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data-home");
        let home = dir.path().join("home");
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&home).unwrap();
        let previous_xdg = std::env::var_os("XDG_DATA_HOME");
        let previous_home = std::env::var_os("HOME");
        std::env::set_var("XDG_DATA_HOME", &data_home);
        std::env::set_var("HOME", &home);

        let item_path = dir.path().join("to-trash.txt");
        fs::write(&item_path, b"trash test").unwrap();
        let item = VPath::local(&item_path).unwrap();
        let result = LocalVfs.trash(&item);

        if let Some(value) = previous_xdg {
            std::env::set_var("XDG_DATA_HOME", value);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }
        if let Some(value) = previous_home {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }

        result.unwrap();
        assert!(!item_path.exists());
        assert!(data_home.join("Trash").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_child_is_reported_without_losing_other_entries() {
        if rustix::process::geteuid().is_root() {
            eprintln!("übersprungen: als Root sind POSIX-Leserechte nicht prüfbar");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(dir.path().join("visible"), b"x").unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o0)).unwrap();
        let entries = LocalVfs.list(&VPath::local(dir.path()).unwrap()).unwrap();
        assert!(entries.iter().any(|entry| entry.name == "visible"));
        assert!(entries
            .iter()
            .find(|entry| entry.name == "blocked")
            .unwrap()
            .error
            .is_some());
    }

    fn arch(outer: &str, inner: &str) -> VPath {
        VPath::archive(VPath::local(outer).unwrap(), inner).unwrap()
    }

    #[test]
    fn ipc_roundtrip_for_local_and_archive_paths() {
        for text in [
            "/",
            "/home/user/file.txt",
            "/with space/\u{00fc}ml\u{00e4}ut/a.zip",
            "archive:/home/u/a.zip!/",
            "archive:/home/u/a.zip!/dir/file.txt",
            "archive:/home/u/a.zip!/\u{00fc}/\u{1f600}.txt",
        ] {
            let parsed = parse_ipc_path(text).unwrap();
            assert_eq!(format_ipc_path(&parsed), text, "{text}");
        }
        let local = parse_ipc_path("/tmp/x").unwrap();
        assert_eq!(local, VPath::local("/tmp/x").unwrap());
        assert_eq!(local.scheme(), Scheme::Local);
        let archive = parse_ipc_path("archive:/tmp/a.zip!/d/f").unwrap();
        assert_eq!(archive.scheme(), Scheme::Archive);
        assert_eq!(archive.path(), Path::new("/d/f"));
        assert_eq!(archive.outer().unwrap().path(), Path::new("/tmp/a.zip"));
    }

    #[test]
    fn ipc_encodes_percent_and_exclamation_marks_in_the_outer_path() {
        let outer = VPath::local("/tmp/100%/wow!.zip").unwrap();
        let path = VPath::archive(outer, "/in!side/50%.txt").unwrap();
        let text = format_ipc_path(&path);
        assert_eq!(text, "archive:/tmp/100%25/wow%21.zip!/in!side/50%.txt");
        assert_eq!(parse_ipc_path(&text).unwrap(), path);
        // a literal "%21" in a real folder name survives too
        let tricky = VPath::archive(VPath::local("/tmp/%21/a.zip").unwrap(), "/x").unwrap();
        assert_eq!(parse_ipc_path(&format_ipc_path(&tricky)).unwrap(), tricky);
    }

    #[test]
    fn ipc_nested_archives_roundtrip() {
        let level1 = arch("/tmp/a!b.zip", "/inner%.tar.gz");
        let level2 = VPath::archive(level1, "/deep/b.zip").unwrap();
        let level3 = VPath::archive(level2, "/c.7z").unwrap();
        let text = format_ipc_path(&level3);
        assert_eq!(parse_ipc_path(&text).unwrap(), level3);
        assert_eq!(level3.archive_depth(), 3);
        assert_eq!(
            parse_ipc_path(&format_ipc_path(&arch("/a.zip", "/")))
                .unwrap()
                .path(),
            Path::new("/")
        );
    }

    #[test]
    fn ipc_nesting_depth_is_limited_to_three() {
        let level3 = VPath::archive(
            VPath::archive(arch("/a.zip", "/b.zip"), "/c.zip").unwrap(),
            "/d.zip",
        )
        .unwrap();
        assert_eq!(level3.archive_depth(), 3);
        assert!(VPath::archive(level3.clone(), "/e").is_err());
        let text = format!("archive:{}!/x", encode_outer(&format_ipc_path(&level3)));
        assert!(parse_ipc_path(&text).is_err());
    }

    #[test]
    fn ipc_rejects_forged_paths() {
        for text in [
            "archive:/x!/../../etc",
            "archive:/x.zip!/a/../b",
            "archive:/x.zip!/..",
            "archive:/x.zip!/a/./b",
            "archive:/x.zip!relative",
            "archive:/x.zip!",
            "archive:/x.zip",
            "archive:/x.zip!/a\0b",
            "archive:/x.zip!/a\\b",
            "archive:relative.zip!/a",
            "archive:/../etc/x.zip!/a",
            "archive:/x%2f.zip!/a",
            "archive:/x%.zip!/a",
            "archive:/x%2.zip!/a",
            "archive:archive:/x.zip!/!/a",
            "archive:archive:/x.zip!/!/",
            "archive:",
            "archive:!",
            "relative/path",
            "",
        ] {
            assert!(parse_ipc_path(text).is_err(), "must reject {text:?}");
        }
        // the outer location of an archive path must name a file, not an archive root
        assert!(parse_ipc_path("archive:archive:/x.zip%21/!/a").is_err());
        let too_long = format!("archive:/x.zip!/{}", "n".repeat(256));
        assert!(parse_ipc_path(&too_long).is_err());
        let too_deep = format!("archive:/x.zip!/{}", "d/".repeat(129));
        assert!(parse_ipc_path(&too_deep).is_err());
        let deep_ok = format!("archive:/x.zip!/{}", "d/".repeat(128));
        assert!(parse_ipc_path(&deep_ok).is_ok());
    }

    #[test]
    fn archive_paths_join_and_go_to_the_parent_inside_the_archive() {
        let root = arch("/tmp/a.zip", "/");
        let file = root.join("dir").unwrap().join("f.txt").unwrap();
        assert_eq!(format_ipc_path(&file), "archive:/tmp/a.zip!/dir/f.txt");
        assert!(root.join("..").is_err() && root.join("a/b").is_err());
        assert!(root.join_entry("..").is_err());
        let dir = file.parent().unwrap();
        assert_eq!(dir.path(), Path::new("/dir"));
        assert_eq!(dir.parent().unwrap(), root);
        assert!(root.parent().is_none());
        assert_eq!(file.outer(), root.outer());
    }
}

#[cfg(test)]
mod space_tests {
    use super::*;
    #[test]
    fn local_space_and_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let space = LocalVfs
            .space_free(&VPath::local(dir.path()).unwrap())
            .unwrap();
        assert!(space.available > 0);
        assert!(space.total >= space.available);
        assert!(LocalVfs
            .space_free(&VPath::local(dir.path().join("missing")).unwrap())
            .is_none());
    }
}

/// POSIX access bits, including setuid/setgid/sticky with and without execute.
pub fn format_permissions(mode: u32) -> String {
    let mut out = ['-'; 9];
    for (i, c) in ['r', 'w', 'x', 'r', 'w', 'x', 'r', 'w', 'x']
        .into_iter()
        .enumerate()
    {
        if mode & (1 << (8 - i)) != 0 {
            out[i] = c;
        }
    }
    for (index, bit, lower, upper) in [
        (2, 0o4000, 's', 'S'),
        (5, 0o2000, 's', 'S'),
        (8, 0o1000, 't', 'T'),
    ] {
        if mode & bit != 0 {
            out[index] = if out[index] == 'x' { lower } else { upper };
        }
    }
    out.into_iter().collect()
}

/// Cache successful and failed NSS lookups per ID for the lifetime of the process.
pub fn local_owner_name(id: u32, group: bool) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type OwnerCache = Mutex<HashMap<(u32, bool), Option<String>>>;
    static CACHE: OnceLock<OwnerCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = lock().get(&(id, group)) {
        return hit.clone();
    }
    // NSS (LDAP, SSSD) can block; resolve without holding the lock. A racing duplicate lookup is harmless.
    let name = resolve_owner_name(id, group);
    lock().entry((id, group)).or_insert(name).clone()
}

#[cfg(unix)]
fn resolve_owner_name(id: u32, group: bool) -> Option<String> {
    let mut buffer = vec![0u8; 1024];
    loop {
        // SAFETY: the reentrant NSS functions receive initialized structs and a writable
        // buffer of the stated length. A successful non-null result owns no memory;
        // its name is copied while the backing buffer is still alive.
        let (status, name) = unsafe {
            if group {
                let mut record: libc::group = std::mem::zeroed();
                let mut result = std::ptr::null_mut();
                let status = libc::getgrgid_r(
                    id,
                    &mut record,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                );
                (
                    status,
                    if status == 0 && !result.is_null() {
                        record.gr_name
                    } else {
                        std::ptr::null_mut()
                    },
                )
            } else {
                let mut record: libc::passwd = std::mem::zeroed();
                let mut result = std::ptr::null_mut();
                let status = libc::getpwuid_r(
                    id,
                    &mut record,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                );
                (
                    status,
                    if status == 0 && !result.is_null() {
                        record.pw_name
                    } else {
                        std::ptr::null_mut()
                    },
                )
            }
        };
        if status == libc::ERANGE {
            // Bound memory use even for a broken NSS provider.
            if buffer.len() >= 16 * 1024 * 1024 {
                return None;
            }
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || name.is_null() {
            return None;
        }
        // SAFETY: NSS returned a successful record whose name lives in buffer.
        return Some(
            unsafe { std::ffi::CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
}

#[cfg(not(unix))]
fn resolve_owner_name(_id: u32, _group: bool) -> Option<String> {
    None
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    #[test]
    fn old_entries_deserialize_without_ownership() {
        let entry: Entry = serde_json::from_value(serde_json::json!({
            "name": "old", "kind": "File", "size": 0, "modified": null,
            "mode": null, "hidden": false, "link_target": null, "broken_link": false, "error": null
        }))
        .unwrap();
        assert_eq!((entry.uid, entry.gid), (None, None));
        assert_eq!(local_owner_name(u32::MAX, false), None);
        assert_eq!(local_owner_name(u32::MAX, true), None);
    }
    #[test]
    fn permissions_include_special_bits() {
        for (mode, expected) in [
            (0o755, "rwxr-xr-x"),
            (0, "---------"),
            (0o7777, "rwsrwsrwt"),
            (0o7644, "rwSr-Sr-T"),
        ] {
            assert_eq!(format_permissions(mode), expected);
        }
    }
    #[cfg(unix)]
    #[test]
    fn local_listing_has_metadata_ownership() {
        // SAFETY: these process identity queries have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        assert!(local_owner_name(uid, false).is_some());
        assert!(local_owner_name(gid, true).is_some());
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("owned");
        std::fs::write(&file, b"test").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        let entries = LocalVfs.list(&VPath::local(dir.path()).unwrap()).unwrap();
        let entry = entries.iter().find(|entry| entry.name == "owned").unwrap();
        assert_eq!(entry.uid, Some(meta.uid()));
        assert_eq!(entry.gid, Some(meta.gid()));
    }
}
