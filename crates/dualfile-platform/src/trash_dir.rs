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
/// Flags for path components that are only walked through: `O_PATH` needs search, not read
/// permission, so a parent with mode 0711 does not hide the trash below it.
#[cfg(any(target_os = "linux", target_os = "android"))]
const WALK_FLAGS: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const WALK_FLAGS: OFlags = DIR_FLAGS;

/// Walks an absolute path without following any symlink. Returns the handle of the parent
/// (walk-only, `None` for `/`) and the readable handle of the last directory.
fn walk(path: &Path) -> std::io::Result<(Option<File>, File)> {
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut names = Vec::new();
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => names.push(name),
            _ => return Err(std::io::ErrorKind::InvalidInput.into()),
        }
    }
    let Some((last, walked)) = names.split_last() else {
        return Ok((None, openat(CWD, "/", DIR_FLAGS, Mode::empty())?.into()));
    };
    let mut fd = openat(CWD, "/", WALK_FLAGS, Mode::empty())?;
    for name in walked {
        fd = openat(&fd, *name, WALK_FLAGS, Mode::empty())?;
    }
    let target = openat(&fd, *last, DIR_FLAGS, Mode::empty())?;
    Ok((Some(fd.into()), target.into()))
}

/// Opens an absolute directory path component by component without following any symlink.
/// Intermediate directories only need search permission.
pub fn open_dir_nofollow(path: &Path) -> std::io::Result<File> {
    walk(path).map(|(_, dir)| dir)
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

/// Pure decision after our own parts were set to 0700 (see [`owned_trash_dir_fixed`]).
/// `parts` are the modes *after* that attempt. If they are still writable by others, the
/// file system has fixed permissions (vfat, exfat, NTFS with `uid=`/`umask=`): the trash is
/// accepted only when the directory holding it (the drive root) belongs to us as well, i.e.
/// the whole drive is mounted for us. Foreign owners are always refused.
pub fn trust_fixed(
    parts: &[(u32, u32)],
    parent_owner: Option<u32>,
    uid: u32,
) -> Result<(), Untrusted> {
    match trust(parts, uid) {
        Err(Untrusted::OtherWritable(_)) if parent_owner == Some(uid) => Ok(()),
        other => other,
    }
}

/// Handles of a trusted trash directory.
#[derive(Debug)]
pub struct OwnedTrashDir {
    pub root: File,
    pub files: File,
    pub info: File,
}

fn owner_and_mode(dirs: [&File; 3]) -> std::io::Result<[(u32, u32); 3]> {
    use std::os::unix::fs::MetadataExt;
    let mut parts = [(0, 0); 3];
    for (slot, dir) in parts.iter_mut().zip(dirs) {
        let meta = dir.metadata()?;
        *slot = (meta.uid(), meta.mode());
    }
    Ok(parts)
}

fn refusal(reason: Untrusted) -> std::io::Error {
    match reason {
        Untrusted::NotOwned => std::io::ErrorKind::PermissionDenied.into(),
        Untrusted::OtherWritable(mode) => {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, OtherWritable(mode))
        }
    }
}

/// Opens `path` as a trash directory and checks that it can be trusted (see module docs).
/// Strict: nothing is changed. Not owned gives `PermissionDenied` without OS error; writable
/// by others gives `PermissionDenied` carrying [`OtherWritable`]; real open errors (`ELOOP`,
/// `ENOTDIR`, `EACCES`, ...) are passed through.
pub fn owned_trash_dir(path: &Path, uid: u32) -> std::io::Result<OwnedTrashDir> {
    let root = open_dir_nofollow(path)?;
    let files = open_subdir_nofollow(&root, "files")?;
    let info = open_subdir_nofollow(&root, "info")?;
    trust(&owner_and_mode([&root, &files, &info])?, uid).map_err(refusal)?;
    Ok(OwnedTrashDir { root, files, info })
}

/// Like [`owned_trash_dir`], but our own parts that are writable by group or others (e.g.
/// created under umask 002) are set to 0700 through their opened handles first. Where that
/// has no effect (fixed-permission file systems), [`trust_fixed`] decides. Foreign owners and
/// symlinks stay refused; nothing is changed in a directory that is not ours.
pub fn owned_trash_dir_fixed(path: &Path, uid: u32) -> std::io::Result<OwnedTrashDir> {
    use std::os::unix::fs::MetadataExt;
    let (parent, root) = walk(path)?;
    let files = open_subdir_nofollow(&root, "files")?;
    let info = open_subdir_nofollow(&root, "info")?;
    let dirs = [&root, &files, &info];
    let before = owner_and_mode(dirs)?;
    match trust(&before, uid) {
        Ok(()) => return Ok(OwnedTrashDir { root, files, info }),
        Err(Untrusted::NotOwned) => return Err(refusal(Untrusted::NotOwned)),
        Err(Untrusted::OtherWritable(_)) => {}
    }
    for (dir, (_, mode)) in dirs.iter().zip(before) {
        if mode & 0o022 != 0 {
            // Fails or is ignored on fixed-permission file systems; checked again below.
            let _ = rustix::fs::fchmod(*dir, Mode::from_raw_mode(0o700));
        }
    }
    let parent_owner = match &parent {
        Some(parent) => Some(parent.metadata()?.uid()),
        None => None,
    };
    trust_fixed(&owner_and_mode(dirs)?, parent_owner, uid).map_err(refusal)?;
    Ok(OwnedTrashDir { root, files, info })
}

/// Prepares a per-drive trash for moving a file into it: the root must be a real directory
/// of ours; missing `files`/`info` are created with 0700 inside it, then
/// [`owned_trash_dir_fixed`] decides. Every refusal is `PermissionDenied`, so the caller can
/// fall back to the home trash.
pub fn prepare_trash_dir(path: &Path, uid: u32) -> std::io::Result<OwnedTrashDir> {
    use std::os::unix::fs::MetadataExt;
    // A symlink on the way (ELOOP, ENOTDIR) is a refusal as well.
    let denied = |e: std::io::Error| {
        let symlink = [rustix::io::Errno::LOOP, rustix::io::Errno::NOTDIR]
            .iter()
            .any(|errno| e.raw_os_error() == Some(errno.raw_os_error()));
        if symlink {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)
        } else {
            e
        }
    };
    let root = open_dir_nofollow(path).map_err(denied)?;
    if root.metadata()?.uid() != uid {
        return Err(refusal(Untrusted::NotOwned));
    }
    for name in ["files", "info"] {
        match rustix::fs::mkdirat(&root, name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
    }
    owned_trash_dir_fixed(path, uid).map_err(denied)
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

    #[test]
    fn fixed_permissions_are_accepted_only_on_a_drive_of_our_own() {
        let dir = 0o040_000;
        // vfat/exfat/NTFS with uid=1000,umask=000: chmod has no effect, all parts stay 0777.
        let fixed = [(1000, dir | 0o777); 3];
        assert_eq!(trust_fixed(&fixed, Some(1000), 1000), Ok(()));
        // The same modes below a drive root of somebody else (e.g. /dev/shm, owned by root).
        assert_eq!(
            trust_fixed(&fixed, Some(0), 1000),
            Err(Untrusted::OtherWritable(0o777))
        );
        assert_eq!(
            trust_fixed(&fixed, None, 1000),
            Err(Untrusted::OtherWritable(0o777))
        );
        // A foreign owner is never accepted, not even on a drive of our own.
        let foreign = [
            (1000, dir | 0o700),
            (1001, dir | 0o700),
            (1000, dir | 0o700),
        ];
        assert_eq!(
            trust_fixed(&foreign, Some(1000), 1000),
            Err(Untrusted::NotOwned)
        );
        assert_eq!(
            trust_fixed(&[(1000, dir | 0o700); 3], Some(0), 1000),
            Ok(())
        );
    }
}
