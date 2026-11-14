//! Shared read-only local scan primitives. Unix paths are opened component by component
//! with NOFOLLOW; directory listings stream instead of collecting an unbounded folder.
//! Non-Unix uses symlink_metadata checks and standard reads: it does not offer Unix
//! protection against concurrent replacement of path components (TOCTOU).
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    pub len: u64,
    pub allocated: u64,
    pub dir: bool,
    pub file: bool,
    pub modified: Option<SystemTime>,
    pub identity: Option<(u64, u64)>,
    pub links: u64,
}
impl Meta {
    pub fn from_std(m: fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: m.len(),
            dir: m.is_dir(),
            file: m.is_file(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            allocated: m.blocks().saturating_mul(512),
            #[cfg(not(unix))]
            allocated: m.len(),
            #[cfg(unix)]
            identity: Some((m.dev(), m.ino())),
            #[cfg(not(unix))]
            identity: None,
            #[cfg(unix)]
            links: m.nlink(),
            #[cfg(not(unix))]
            links: 1,
        }
    }
}

/// Resolve a user-supplied root once; all subsequent access refuses symlinks.
pub(crate) fn root(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

#[cfg(unix)]
fn parent(path: &Path) -> io::Result<(File, &std::ffi::OsStr)> {
    use rustix::fs::{openat, Mode, OFlags, CWD};
    use std::path::Component;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(CWD, "/", flags, Mode::empty())?;
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name = path.file_name().unwrap_or(std::ffi::OsStr::new("."));
    if let Some(p) = path.parent() {
        for c in p.components() {
            match c {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(s) => fd = openat(&fd, s, flags, Mode::empty())?,
                _ => return Err(io::ErrorKind::InvalidInput.into()),
            }
        }
    }
    Ok((File::from(fd), name))
}

#[cfg(unix)]
pub(crate) fn metadata(path: &Path) -> io::Result<Meta> {
    use rustix::fs::{statat, AtFlags, FileType};
    use std::time::{Duration, UNIX_EPOCH};
    let (fd, name) = parent(path)?;
    let s = statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
    let modified = if s.st_mtime >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(s.st_mtime as u64, s.st_mtime_nsec as u32))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(s.st_mtime.unsigned_abs()))
            .and_then(|t| t.checked_add(Duration::from_nanos(s.st_mtime_nsec as u64)))
    };
    Ok(Meta {
        len: s.st_size as u64,
        allocated: (s.st_blocks as u64).saturating_mul(512),
        dir: FileType::from_raw_mode(s.st_mode) == FileType::Directory,
        file: FileType::from_raw_mode(s.st_mode) == FileType::RegularFile,
        modified,
        identity: Some((s.st_dev as u64, s.st_ino as u64)),
        links: s.st_nlink as u64,
    })
}
#[cfg(not(unix))]
pub(crate) fn metadata(path: &Path) -> io::Result<Meta> {
    fs::symlink_metadata(path).map(Meta::from_std)
}

#[cfg(unix)]
fn open(path: &Path, directory: bool) -> io::Result<File> {
    use rustix::fs::{openat, Mode, OFlags};
    let (fd, name) = parent(path)?;
    let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    Ok(File::from(openat(&fd, name, flags, Mode::empty())?))
}

pub(crate) fn open_file(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let file = open(path, false)?;
    #[cfg(not(unix))]
    let file = {
        if !metadata(path)?.file {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        File::open(path)?
    };
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

pub(crate) fn children(path: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<PathBuf>>>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let fd = open(path, true)?;
        let dir = rustix::fs::Dir::read_from(&fd)?;
        let base = path.to_path_buf();
        Ok(Box::new(dir.filter_map(move |item| match item {
            Err(e) => Some(Err(e.into())),
            Ok(e) => {
                let bytes = e.file_name().to_bytes();
                if bytes == b"." || bytes == b".." {
                    None
                } else {
                    Some(Ok(base.join(std::ffi::OsStr::from_bytes(bytes))))
                }
            }
        })))
    }
    #[cfg(not(unix))]
    {
        if !metadata(path)?.dir {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        Ok(Box::new(fs::read_dir(path)?.map(|e| e.map(|e| e.path()))))
    }
}
