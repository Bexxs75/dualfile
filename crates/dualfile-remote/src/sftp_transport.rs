//! The narrow interface between the file-system layer (`SftpVfs`) and a real or fake SFTP
//! session. One transport value is one logged-in SSH connection with the `sftp` subsystem
//! (and nothing else: no shell, no exec, no forwarding). SFTP multiplexes files over one
//! connection, so a transfer is a handle on the transport, driven with offsets.

use crate::error::TResult;
use std::time::SystemTime;

/// An open file or directory on the server (a number the transport maps to its own handle).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SftpHandle(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawKind {
    File,
    Dir,
    Symlink,
    /// Pipe, device, socket: shown but never opened.
    Other,
    /// The server did not tell (no permission bits in the attributes).
    Unknown,
}

/// The attributes of one entry as the server sent them (`lstat` style, links not followed).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawAttrs {
    pub kind: RawKind,
    pub size: Option<u64>,
    pub mtime: Option<SystemTime>,
    pub atime: Option<SystemTime>,
    /// The permission bits (`0o7777` part only).
    pub permissions: Option<u32>,
}

/// One name of a directory listing, as received (not checked yet).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawEntry {
    pub name: String,
    pub attrs: RawAttrs,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SftpFeatures {
    /// `posix-rename@openssh.com`: a rename that replaces an existing target atomically.
    pub posix_rename: bool,
}

/// How `open_write` treats an existing file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenWrite {
    /// Create; fails if the name exists (`SSH_FXF_EXCL`): never touches existing data.
    CreateNew,
    /// Create or empty (`SSH_FXF_CREAT | SSH_FXF_TRUNC`).
    CreateTruncate,
    /// The file exists already; writes go to the given offsets (resuming).
    Existing,
}

/// One SFTP session. Paths are absolute server paths without control characters (`SftpVfs`
/// checks that before it calls); implementations must still refuse CR, LF and NUL
/// (`TransportError::Rejected`). Every call ends within the idle limit (`Timeout`).
///
/// Dropping the transport closes the connection (open handles go with it).
pub trait SftpTransport: Send + 'static {
    fn space_free(&mut self, _path: &str) -> TResult<Option<dualfile_core::vfs::SpaceFree>> {
        Ok(None)
    }
    fn features(&self) -> SftpFeatures;
    fn realpath(&mut self, path: &str) -> TResult<String>;
    /// `SSH_FXP_LSTAT`; a missing path is a `Status { NoSuchFile }` error.
    fn lstat(&mut self, path: &str) -> TResult<RawAttrs>;
    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle>;
    /// The next batch of names; empty = end of the directory. Includes `.` and `..` if the
    /// server sends them.
    fn read_dir(&mut self, handle: SftpHandle) -> TResult<Vec<RawEntry>>;
    fn open_read(&mut self, path: &str) -> TResult<SftpHandle>;
    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle>;
    /// Reads up to `buf.len()` bytes at `offset`; 0 = end of the file. May return less.
    fn read(&mut self, handle: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize>;
    /// Writes all of `data` at `offset` or fails.
    fn write(&mut self, handle: SftpHandle, offset: u64, data: &[u8]) -> TResult<()>;
    fn fstat(&mut self, handle: SftpHandle) -> TResult<RawAttrs>;
    fn close(&mut self, handle: SftpHandle) -> TResult<()>;
    fn mkdir(&mut self, path: &str) -> TResult<()>;
    fn rmdir(&mut self, path: &str) -> TResult<()>;
    /// Removes a file or a link (never the target of a link).
    fn remove(&mut self, path: &str) -> TResult<()>;
    /// `SSH_FXP_RENAME`: the protocol says it must fail if the target exists; servers differ
    /// (OpenSSH refuses, others may replace). Callers look first.
    fn rename(&mut self, from: &str, to: &str) -> TResult<()>;
    /// `posix-rename@openssh.com` (only if `features().posix_rename`): replaces the target.
    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()>;
    fn readlink(&mut self, path: &str) -> TResult<String>;
    /// Creates the link `at` whose content is `target`.
    fn symlink(&mut self, target: &str, at: &str) -> TResult<()>;
    /// Sets permission bits and/or times (`mtime` and the access time together).
    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        atime: Option<SystemTime>,
    ) -> TResult<()>;
    fn quit(&mut self);
}

impl SftpTransport for Box<dyn SftpTransport> {
    fn space_free(&mut self, path: &str) -> TResult<Option<dualfile_core::vfs::SpaceFree>> {
        (**self).space_free(path)
    }
    fn features(&self) -> SftpFeatures {
        (**self).features()
    }
    fn realpath(&mut self, path: &str) -> TResult<String> {
        (**self).realpath(path)
    }
    fn lstat(&mut self, path: &str) -> TResult<RawAttrs> {
        (**self).lstat(path)
    }
    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle> {
        (**self).open_dir(path)
    }
    fn read_dir(&mut self, handle: SftpHandle) -> TResult<Vec<RawEntry>> {
        (**self).read_dir(handle)
    }
    fn open_read(&mut self, path: &str) -> TResult<SftpHandle> {
        (**self).open_read(path)
    }
    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle> {
        (**self).open_write(path, how)
    }
    fn read(&mut self, handle: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize> {
        (**self).read(handle, offset, buf)
    }
    fn write(&mut self, handle: SftpHandle, offset: u64, data: &[u8]) -> TResult<()> {
        (**self).write(handle, offset, data)
    }
    fn fstat(&mut self, handle: SftpHandle) -> TResult<RawAttrs> {
        (**self).fstat(handle)
    }
    fn close(&mut self, handle: SftpHandle) -> TResult<()> {
        (**self).close(handle)
    }
    fn mkdir(&mut self, path: &str) -> TResult<()> {
        (**self).mkdir(path)
    }
    fn rmdir(&mut self, path: &str) -> TResult<()> {
        (**self).rmdir(path)
    }
    fn remove(&mut self, path: &str) -> TResult<()> {
        (**self).remove(path)
    }
    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        (**self).rename(from, to)
    }
    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()> {
        (**self).posix_rename(from, to)
    }
    fn readlink(&mut self, path: &str) -> TResult<String> {
        (**self).readlink(path)
    }
    fn symlink(&mut self, target: &str, at: &str) -> TResult<()> {
        (**self).symlink(target, at)
    }
    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        atime: Option<SystemTime>,
    ) -> TResult<()> {
        (**self).setstat(path, permissions, mtime, atime)
    }
    fn quit(&mut self) {
        (**self).quit()
    }
}
