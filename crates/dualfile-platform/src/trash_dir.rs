//! Trust check and handle-based removal for freedesktop trash directories, shared by
//! "empty trash" (dualfile-core) and the trash view (vendored `trash` crate).
//!
//! A trash directory is trusted only when its root, `files` and `info` are real directories
//! (no symlink anywhere on the path), owned by the effective user and not writable by group
//! or others. Everything done inside it goes through the opened handles.
use rustix::fs::{openat, statat, unlinkat, AtFlags, FileType, Mode, OFlags, CWD};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    path::{Component, Path},
};

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Opens an absolute directory path component by component without following any symlink.
pub fn open_dir_nofollow(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut fd = openat(CWD, "/", DIR_FLAGS, Mode::empty())?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => fd = openat(&fd, name, DIR_FLAGS, Mode::empty())?,
            _ => return Err(std::io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(fd.into())
}

/// Opens `name` inside `parent` as a directory, never following a symlink.
pub fn open_subdir_nofollow(parent: &File, name: impl AsRef<OsStr>) -> std::io::Result<File> {
    Ok(openat(parent, name.as_ref(), DIR_FLAGS, Mode::empty())?.into())
}

/// Why an owned trash directory was refused: it is writable by group or others.
#[derive(Debug)]
pub struct OtherWritable(pub u32);
impl std::fmt::Display for OtherWritable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "directory is writable by others (mode {:03o})", self.0)
    }
}
impl std::error::Error for OtherWritable {}

/// Why a trash directory is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untrusted {
    /// Root, `files` or `info` belongs to another user.
    NotOwned,
    /// Owned, but writable by group or others (the permission bits of the first such part).
    OtherWritable(u32),
}

/// Pure decision over `(owner uid, st_mode)` of root, `files` and `info`, in that order.
pub fn trust(parts: &[(u32, u32)], uid: u32) -> Result<(), Untrusted> {
    if parts.iter().any(|&(owner, _)| owner != uid) {
        return Err(Untrusted::NotOwned);
    }
    match parts.iter().find(|&&(_, mode)| mode & 0o022 != 0) {
        Some(&(_, mode)) => Err(Untrusted::OtherWritable(mode & 0o777)),
        None => Ok(()),
    }
}

/// Handles of a trusted trash directory.
#[derive(Debug)]
pub struct OwnedTrashDir {
    pub root: File,
    pub files: File,
    pub info: File,
}

/// Opens `path` as a trash directory and checks that it can be trusted (see module docs).
/// Not owned gives `PermissionDenied` without OS error; writable by others gives
/// `PermissionDenied` carrying [`OtherWritable`]; real open errors (`ELOOP`, `ENOTDIR`,
/// `EACCES`, ...) are passed through.
pub fn owned_trash_dir(path: &Path, uid: u32) -> std::io::Result<OwnedTrashDir> {
    use std::os::unix::fs::MetadataExt;
    let root = open_dir_nofollow(path)?;
    let files = open_subdir_nofollow(&root, "files")?;
    let info = open_subdir_nofollow(&root, "info")?;
    let mut parts = [(0, 0); 3];
    for (slot, dir) in parts.iter_mut().zip([&root, &files, &info]) {
        let meta = dir.metadata()?;
        *slot = (meta.uid(), meta.mode());
    }
    match trust(&parts, uid) {
        Ok(()) => Ok(OwnedTrashDir { root, files, info }),
        Err(Untrusted::NotOwned) => Err(std::io::ErrorKind::PermissionDenied.into()),
        Err(Untrusted::OtherWritable(mode)) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OtherWritable(mode),
        )),
    }
}

/// The user's own home trash: a trusted root (it may sit behind a symlinked home), opened
/// as it is, without the ownership and mode check.
pub fn home_trash_dir(path: &Path) -> std::io::Result<OwnedTrashDir> {
    let open = |path: &Path| -> std::io::Result<File> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        Ok(openat(CWD, path, flags, Mode::empty())?.into())
    };
    Ok(OwnedTrashDir {
        root: open(path)?,
        files: open(&path.join("files"))?,
        info: open(&path.join("info"))?,
    })
}

/// Whether `name` in `dir` is a real directory (a symlink is not followed).
pub fn is_dir_at(dir: &File, name: &OsStr) -> std::io::Result<bool> {
    let meta = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(FileType::from_raw_mode(meta.st_mode) == FileType::Directory)
}

/// Removes the non-directory entry `name` in `dir`.
pub fn unlink_at(dir: &File, name: &OsStr) -> std::io::Result<()> {
    Ok(unlinkat(dir, name, AtFlags::empty())?)
}

/// Moves the entry `name` out of `dir` to the absolute path `to`.
pub fn rename_out(dir: &File, name: &OsStr, to: &Path) -> std::io::Result<()> {
    Ok(rustix::fs::renameat(dir, name, CWD, to)?)
}

/// Device of an opened handle.
pub fn device(file: &File) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.dev())
}

/// Removes `name` inside `parent` (recursively for directories) through pinned directory
/// handles; symlinks are unlinked, never followed, and no other device is entered.
/// `stop` is asked before every unlink (`true` = stop, returns `Ok(false)`), `tick` is called
/// after each one.
pub fn remove_tree_at(
    parent: &File,
    name: &OsStr,
    root_device: u64,
    device: impl Fn(&File) -> std::io::Result<u64>,
    mut stop: impl FnMut() -> bool,
    mut tick: impl FnMut(),
) -> std::io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    struct Frame {
        directory: File,
        entries: rustix::fs::Dir,
        name: OsString,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut pending = Some(name.to_os_string());
    loop {
        if stop() {
            return Ok(false);
        }
        if let Some(name) = pending.take() {
            let parent = stack.last().map_or(parent, |frame| &frame.directory);
            let meta = statat(parent, &name, AtFlags::SYMLINK_NOFOLLOW)?;
            if FileType::from_raw_mode(meta.st_mode) == FileType::Directory {
                let child = open_subdir_nofollow(parent, &name)?;
                // Check the opened handle, not the preceding lstat: a mount or
                // replacement may have appeared between those two syscalls.
                if device(&child)? != root_device {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                let entries = rustix::fs::Dir::read_from(&child)?;
                stack.push(Frame {
                    directory: child,
                    entries,
                    name,
                });
            } else {
                if stop() {
                    return Ok(false);
                }
                unlinkat(parent, &name, AtFlags::empty())?;
                tick();
            }
        } else if let Some(frame) = stack.last_mut() {
            if let Some(entry) = frame.entries.next() {
                let entry = entry?;
                let name = entry.file_name().to_bytes();
                if name != b"." && name != b".." {
                    pending = Some(OsStr::from_bytes(name).to_os_string());
                }
            } else {
                let frame = stack.pop().expect("frame was present");
                let parent = stack.last().map_or(parent, |frame| &frame.directory);
                if stop() {
                    return Ok(false);
                }
                unlinkat(parent, &frame.name, AtFlags::REMOVEDIR)?;
                tick();
            }
        } else {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_requires_owner_and_no_group_or_other_write() {
        let dir = 0o040_000;
        assert_eq!(trust(&[(1000, dir | 0o700); 3], 1000), Ok(()));
        assert_eq!(trust(&[(1000, dir | 0o755); 3], 1000), Ok(()));
        // A foreign UID on any part, even with tight modes.
        for i in 0..3 {
            let mut parts = [(1000, dir | 0o700); 3];
            parts[i].0 = 1001;
            assert_eq!(trust(&parts, 1000), Err(Untrusted::NotOwned));
        }
        // World- or group-writable parts (e.g. 0777 or 1777 like /dev/shm).
        for mode in [0o777, 0o1777, 0o775, 0o757] {
            let parts = [(1000, dir | 0o700), (1000, dir | mode), (1000, dir | 0o700)];
            assert_eq!(
                trust(&parts, 1000),
                Err(Untrusted::OtherWritable(mode & 0o777))
            );
        }
        // Not owned wins over writable by others.
        let parts = [(1000, dir | 0o777), (0, dir | 0o700), (1000, dir | 0o700)];
        assert_eq!(trust(&parts, 1000), Err(Untrusted::NotOwned));
    }
}
