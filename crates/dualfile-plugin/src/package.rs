//! Checking a plugin package on disk before anything of it runs.
//!
//! The package lives in `<base>/<id>/`. Every path component is opened with `O_NOFOLLOW`
//! relative to the previous one (so no symlink can redirect the lookup), every directory and
//! file must belong to the current user and must not be writable by group or others, only
//! regular single-link files are allowed, the folder holds exactly the files the manifest
//! lists, and every listed file is hashed from the descriptor that was opened. The executable
//! must be an ELF file. The verified descriptor of the executable is kept and is what is
//! started later (`execveat` on the descriptor, never a second lookup by path).
//!
//! Limits of that guarantee (honest): the descriptor is opened read-only, but the owner may
//! still write the file in place between the check and the start. Therefore the host hashes
//! the content again through the held descriptor (`pread`, no offset change) in the child
//! right before `execveat` and refuses with `Locked` on any difference in size or hash
//! (`Hasher::matches`). A running program is protected against in-place writes by the kernel
//! (`ETXTBSY`). What remains: a short window between that last hash and the exec, and an
//! attacker with the same user account (who can also replace the host itself). The goal is
//! protection against accidental or foreign changes, not against a malicious process of the
//! user. A sealed `memfd` copy was not adopted: see docs/PHASE-7.md.

use crate::error::{PluginError, PluginResult};
use crate::manifest::{
    is_valid_plugin_id, valid_package_path, Manifest, MANIFEST_FILE, MAX_MANIFEST_BYTES,
};
use rustix::fs::{openat, statat, AtFlags, Dir, FileType, Mode, OFlags, Stat, CWD};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Component, Path, PathBuf};

/// Most directory entries looked at in one package (a plugin is a handful of files).
const MAX_SCANNED_ENTRIES: usize = 512;
/// Largest single package file that is hashed.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// What the user confirmed when activating a plugin; kept by the caller (settings). A package
/// whose manifest differs (any change of rights, files or hashes) no longer matches and is
/// locked until the user confirms again.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PinnedState {
    pub id: String,
    /// SHA-256 (hex) of the exact bytes of `plugin.json`. The manifest holds the hashes of
    /// all other files, so this covers the whole package.
    pub manifest_sha256: String,
    /// The user chose to start this plugin without the sandbox (explicit, never automatic).
    pub unsandboxed: bool,
}

impl PinnedState {
    pub fn for_package(package: &VerifiedPackage, unsandboxed: bool) -> Self {
        Self {
            id: package.manifest.id.clone(),
            manifest_sha256: package.manifest_sha256.clone(),
            unsandboxed,
        }
    }

    /// `Locked` unless the package is exactly the one that was confirmed.
    pub fn check(&self, package: &VerifiedPackage) -> PluginResult<()> {
        if self.id == package.manifest.id && self.manifest_sha256 == package.manifest_sha256 {
            Ok(())
        } else {
            Err(PluginError::Locked)
        }
    }
}

/// A package that passed all checks. Holds the open plugin folder and the open, hashed
/// executable.
#[derive(Debug)]
pub struct VerifiedPackage {
    pub manifest: Manifest,
    pub manifest_sha256: String,
    pub(crate) dir: OwnedFd,
    pub(crate) exe: File,
    /// Size and SHA-256 of the executable as verified; checked again right before the start.
    pub(crate) exe_check: ExeCheck,
    pub(crate) dir_path: PathBuf,
}

/// The verified state of the executable (size in bytes and SHA-256 digest).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExeCheck {
    pub size: u64,
    pub digest: [u8; 32],
}

impl ExeCheck {
    /// Hashes the file behind `fd` with `pread` (the file offset stays untouched) and compares
    /// size and digest. Only plain system calls and stack memory: usable between `fork` and
    /// `exec`. Any read problem or difference counts as a mismatch.
    pub(crate) fn matches(&self, fd: std::os::fd::RawFd) -> bool {
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut offset: u64 = 0;
        loop {
            // SAFETY: `buf` is a valid writable buffer of the given length.
            let n = unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    offset as libc::off_t,
                )
            };
            if n < 0 {
                if unsafe { *libc::__errno_location() } == libc::EINTR {
                    continue;
                }
                return false;
            }
            if n == 0 {
                break;
            }
            offset += n as u64;
            if offset > self.size {
                return false;
            }
            hasher.update(&buf[..n as usize]);
        }
        offset == self.size && hasher.finalize().as_slice() == self.digest
    }
}

impl VerifiedPackage {
    /// The plugin folder (for display only; the plugin is always started from the descriptor).
    pub fn dir_path(&self) -> &Path {
        &self.dir_path
    }
}

/// The default plugin folder: `$XDG_DATA_HOME/dualfile/plugins` or `~/.local/share/dualfile/plugins`.
pub fn default_plugin_dir() -> Option<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(data.join("dualfile/plugins"))
}

/// Refuses to run plugins as the administrator.
pub fn refuse_root(effective_uid: u32, real_uid: u32) -> PluginResult<()> {
    if effective_uid == 0 || real_uid == 0 {
        Err(PluginError::Root)
    } else {
        Ok(())
    }
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn unsafe_pkg(why: &'static str) -> PluginError {
    PluginError::UnsafePackage(why)
}

fn open_flags(directory: bool) -> OFlags {
    let base = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if directory {
        base | OFlags::DIRECTORY
    } else {
        base
    }
}

fn open_component(parent: BorrowedFd<'_>, name: &str, directory: bool) -> Result<OwnedFd, Errno> {
    openat(parent, name, open_flags(directory), Mode::empty())
}

fn map_open(error: Errno, missing: PluginError) -> PluginError {
    match error {
        Errno::NOENT => missing,
        Errno::LOOP | Errno::NOTDIR | Errno::MLINK => {
            unsafe_pkg("Symlink oder falscher Dateityp im Pfad")
        }
        _ => unsafe_pkg("Zugriff auf das Plugin-Paket nicht möglich"),
    }
}

fn owner_and_mode_ok(stat: &Stat, uid: u32) -> PluginResult<()> {
    if stat.st_uid != uid {
        return Err(unsafe_pkg("Eigentümer ist nicht der aktuelle Nutzer"));
    }
    if stat.st_mode & 0o022 != 0 {
        return Err(unsafe_pkg("Für Gruppe oder Andere beschreibbar"));
    }
    Ok(())
}

fn check_dir(fd: BorrowedFd<'_>, uid: u32) -> PluginResult<()> {
    let stat = rustix::fs::fstat(fd).map_err(|_| unsafe_pkg("Ordner nicht lesbar"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(unsafe_pkg("Kein Ordner"));
    }
    owner_and_mode_ok(&stat, uid)
}

fn check_file(fd: BorrowedFd<'_>, uid: u32) -> PluginResult<Stat> {
    let stat = rustix::fs::fstat(fd).map_err(|_| unsafe_pkg("Datei nicht lesbar"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(unsafe_pkg("Keine reguläre Datei"));
    }
    if stat.st_nlink != 1 {
        return Err(unsafe_pkg("Datei hat mehrere Verweise (Hardlink)"));
    }
    owner_and_mode_ok(&stat, uid)?;
    Ok(stat)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Opens `base` one component at a time without following any symlink and returns its fd.
fn open_base(base: &Path) -> PluginResult<OwnedFd> {
    let mut components = base.components();
    if components.next() != Some(Component::RootDir) {
        return Err(unsafe_pkg("Der Plugin-Basisordner ist nicht absolut"));
    }
    let mut fd = openat(CWD, "/", open_flags(true), Mode::empty())
        .map_err(|_| unsafe_pkg("Wurzelordner nicht lesbar"))?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(unsafe_pkg("Der Plugin-Basisordner ist nicht normalisiert"));
        };
        let name = name
            .to_str()
            .ok_or_else(|| unsafe_pkg("Der Plugin-Basisordner ist kein UTF-8"))?;
        fd = open_component(fd.as_fd(), name, true)
            .map_err(|e| map_open(e, PluginError::NotInstalled))?;
    }
    Ok(fd)
}

/// Opens `relative` (clean, validated) below `dir` component by component without following
/// symlinks; the last component is a regular file.
fn open_package_file(dir: BorrowedFd<'_>, relative: &str) -> PluginResult<OwnedFd> {
    let parts: Vec<&str> = relative.split('/').collect();
    let mut current: Option<OwnedFd> = None;
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        let parent = current.as_ref().map_or(dir, |fd| fd.as_fd());
        let fd =
            open_component(parent, part, !last).map_err(|e| map_open(e, PluginError::Locked))?;
        current = Some(fd);
    }
    current.ok_or_else(|| unsafe_pkg("Leerer Pfad"))
}

fn scan_dir(
    dir: BorrowedFd<'_>,
    prefix: &str,
    depth: usize,
    uid: u32,
    out: &mut BTreeSet<String>,
    count: &mut usize,
) -> PluginResult<()> {
    let listing = Dir::read_from(dir).map_err(|_| unsafe_pkg("Ordner nicht lesbar"))?;
    for entry in listing {
        let entry = entry.map_err(|_| unsafe_pkg("Ordner nicht lesbar"))?;
        let raw = entry.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        *count += 1;
        if *count > MAX_SCANNED_ENTRIES {
            return Err(unsafe_pkg("Zu viele Dateien im Plugin-Ordner"));
        }
        let name = std::str::from_utf8(raw)
            .ok()
            .filter(|name| valid_package_path(name))
            .ok_or_else(|| unsafe_pkg("Ungültiger Dateiname im Plugin-Ordner"))?;
        let stat = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| unsafe_pkg("Eintrag nicht lesbar"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => {
                out.insert(path);
            }
            FileType::Directory => {
                if depth + 1 >= crate::manifest::MAX_FILE_PATH_DEPTH {
                    return Err(unsafe_pkg("Zu tief verschachtelt"));
                }
                let sub = open_component(dir, name, true)
                    .map_err(|e| map_open(e, PluginError::Locked))?;
                check_dir(sub.as_fd(), uid)?;
                scan_dir(sub.as_fd(), &path, depth + 1, uid, out, count)?;
            }
            _ => return Err(unsafe_pkg("Symlink oder Sonderdatei im Plugin-Ordner")),
        }
    }
    Ok(())
}

/// Verifies the package `<base>/<id>` for the current user.
pub fn verify_package(base: &Path, id: &str) -> PluginResult<VerifiedPackage> {
    verify_package_as(base, id, current_uid())
}

/// Like `verify_package`, with the user id the files must belong to (a parameter so that the
/// ownership check can be tested).
pub fn verify_package_as(base: &Path, id: &str, owner_uid: u32) -> PluginResult<VerifiedPackage> {
    if !is_valid_plugin_id(id) {
        return Err(PluginError::NotInstalled);
    }
    let base_fd = open_base(base)?;
    check_dir(base_fd.as_fd(), owner_uid)?;
    let dir = open_component(base_fd.as_fd(), id, true)
        .map_err(|e| map_open(e, PluginError::NotInstalled))?;
    check_dir(dir.as_fd(), owner_uid)?;

    // the manifest: exactly these bytes are hashed and parsed
    let manifest_fd = open_component(dir.as_fd(), MANIFEST_FILE, false)
        .map_err(|e| map_open(e, PluginError::NotInstalled))?;
    let stat = check_file(manifest_fd.as_fd(), owner_uid)?;
    if stat.st_size < 0 || stat.st_size as u64 > MAX_MANIFEST_BYTES as u64 {
        return Err(unsafe_pkg("Das Manifest ist zu groß"));
    }
    let mut bytes = Vec::new();
    File::from(manifest_fd)
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unsafe_pkg("Das Manifest ist nicht lesbar"))?;
    let manifest_sha256 = sha256_hex(&bytes);
    let manifest = Manifest::parse(&bytes)?;
    if manifest.id != id {
        return Err(PluginError::Manifest(
            "Die Kennung passt nicht zum Ordnernamen",
        ));
    }

    // the folder holds exactly what the manifest lists
    let mut found = BTreeSet::new();
    scan_dir(dir.as_fd(), "", 0, owner_uid, &mut found, &mut 0)?;
    found.remove(MANIFEST_FILE);
    let listed: BTreeSet<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    if found != listed {
        return Err(PluginError::Locked);
    }

    let mut exe = None;
    for entry in &manifest.files {
        let fd = open_package_file(dir.as_fd(), &entry.path)?;
        let stat = check_file(fd.as_fd(), owner_uid)?;
        if stat.st_size < 0 || stat.st_size as u64 > MAX_FILE_BYTES {
            return Err(unsafe_pkg("Datei ist zu groß"));
        }
        let is_exe = entry.path == manifest.exec;
        if is_exe && stat.st_mode & 0o100 == 0 {
            return Err(unsafe_pkg(
                "Die ausführbare Datei hat kein Ausführungsrecht",
            ));
        }
        let mut file = File::from(fd);
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut head = [0u8; 4];
        let mut seen = 0usize;
        let mut total = 0u64;
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|_| unsafe_pkg("Datei nicht lesbar"))?;
            if n == 0 {
                break;
            }
            if seen < head.len() {
                let take = (head.len() - seen).min(n);
                head[seen..seen + take].copy_from_slice(&buf[..take]);
                seen += take;
            }
            total += n as u64;
            if total > MAX_FILE_BYTES {
                return Err(unsafe_pkg("Datei ist zu groß"));
            }
            hasher.update(&buf[..n]);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        if hex(&digest) != entry.sha256 {
            return Err(PluginError::Locked);
        }
        if is_exe {
            if head.starts_with(b"#!") {
                return Err(unsafe_pkg("Skripte sind nicht erlaubt, nur ELF-Programme"));
            }
            if seen < 4 || head != *b"\x7fELF" {
                return Err(unsafe_pkg("Die ausführbare Datei ist kein ELF-Programm"));
            }
            exe = Some((
                file,
                ExeCheck {
                    size: total,
                    digest,
                },
            ));
        }
    }
    let (exe, exe_check) = exe.ok_or(PluginError::Manifest("Die ausführbare Datei fehlt"))?;
    Ok(VerifiedPackage {
        dir_path: base.join(id),
        manifest,
        manifest_sha256,
        dir,
        exe,
        exe_check,
    })
}

/// One installed plugin as found in the plugin folder.
#[derive(Debug)]
pub struct Installed {
    pub id: String,
    /// The verified package (manifest and hash), or why it cannot be used.
    pub package: PluginResult<VerifiedPackage>,
}

/// Lists the plugin folders below `base` (names that are valid ids, sorted) and verifies each.
pub fn discover(base: &Path) -> Vec<Installed> {
    let Ok(listing) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = listing
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| is_valid_plugin_id(name))
        .take(256)
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|id| Installed {
            package: verify_package(base, &id),
            id,
        })
        .collect()
}
