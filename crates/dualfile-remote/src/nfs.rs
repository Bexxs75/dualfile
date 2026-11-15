//! NFSv3 (pure Rust, `nfs3_client`) uses the handle-based remote VFS like SMB: `NfsTransport`
//! implements `SftpTransport`, the async library is driven on the shared runtime with a timeout
//! per call. Authentication is AUTH_UNIX (uid/gid claimed by the client; there is no password).
//! No library error text escapes: every failure is mapped to a fixed `TransportError`.
use crate::error::{SftpStatus, TResult, TransportError};
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::transport::Timeouts;
use nfs3_client::error::Error as NfsError;
use nfs3_client::tokio::{TokioConnector, TokioIo};
use nfs3_client::{Nfs3Client, Nfs3ConnectionBuilder};
use nfs3_types::nfs3::{self, nfs_fh3, nfsstat3, Nfs3Option, Nfs3Result};
use nfs3_types::xdr_codec::Opaque;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type NfsVfs<T> = crate::sftpvfs::SftpVfs<T>;

/// Default NFS port.
pub const NFS_PORT: u16 = 2049;
const DEFAULT_IO: u32 = 32 * 1024;
const MAX_IO: u32 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct NfsProfile {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// The exported path on the server (`/srv/share`), mounted as the root of the connection.
    pub export: String,
    pub uid: u32,
    pub gid: u32,
    pub start_folder: Option<String>,
}

impl NfsProfile {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host.trim().is_empty()
            || self.host.len() > 253
            || self
                .host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\'))
        {
            return Err("Ungültiger Servername");
        }
        if self.port == 0 {
            return Err("Ungültiger Port");
        }
        if !valid_export(&self.export) {
            return Err("Ungültiger NFS-Exportpfad");
        }
        if let Some(path) = &self.start_folder {
            dualfile_core::vfs::VPath::nfs("check", path).map_err(|_| "Ungültiger Startordner")?;
        }
        Ok(())
    }
}

/// An absolute server path without control characters, backslashes or `..` components.
pub fn valid_export(export: &str) -> bool {
    export.starts_with('/')
        && export.len() <= 1024
        && !export.chars().any(|c| c.is_control() || c == '\\')
        && !export.split('/').any(|part| part == "..")
}

fn bounded<T>(timeout: Duration, future: impl Future<Output = TResult<T>>) -> TResult<T> {
    crate::russh_transport::runtime().block_on(async {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| TransportError::Timeout)?
    })
}

fn run<T>(timeout: Duration, future: impl Future<Output = Result<T, NfsError>>) -> TResult<T> {
    bounded(timeout, async { future.await.map_err(map_error) })
}

/// One NFS call: transport errors and non-OK NFS statuses both become a `TransportError`.
fn call<T, E>(
    timeout: Duration,
    future: impl Future<Output = Result<Nfs3Result<T, E>, NfsError>>,
) -> TResult<T> {
    match run(timeout, future)? {
        Nfs3Result::Ok(value) => Ok(value),
        Nfs3Result::Err((status, _)) => Err(map_status(status)),
    }
}

pub fn map_error(error: NfsError) -> TransportError {
    use nfs3_types::mount::mountstat3 as M;
    match error {
        NfsError::Io(_) => TransportError::Connection("NFS-Server nicht erreichbar".into()),
        NfsError::Rpc(nfs3_client::error::RpcError::Auth) => TransportError::Auth,
        NfsError::Rpc(_) | NfsError::Xdr(_) => TransportError::Protocol("NFS".into()),
        NfsError::Portmap(_) => TransportError::Connection("NFS-Dienst nicht verfügbar".into()),
        NfsError::MountError(M::MNT3ERR_NOENT) => {
            TransportError::status(SftpStatus::NoSuchFile, "")
        }
        NfsError::MountError(M::MNT3ERR_ACCES | M::MNT3ERR_PERM) => {
            TransportError::status(SftpStatus::PermissionDenied, "")
        }
        NfsError::MountError(_) => TransportError::Protocol("NFS-Einbindung abgelehnt".into()),
        NfsError::NfsError(status) => map_status(status),
    }
}

pub fn map_status(status: nfsstat3) -> TransportError {
    use nfsstat3 as S;
    match status {
        S::NFS3ERR_NOENT | S::NFS3ERR_STALE | S::NFS3ERR_BADHANDLE => {
            TransportError::status(SftpStatus::NoSuchFile, "")
        }
        S::NFS3ERR_PERM | S::NFS3ERR_ACCES | S::NFS3ERR_ROFS => {
            TransportError::status(SftpStatus::PermissionDenied, "")
        }
        S::NFS3ERR_EXIST => TransportError::reply(550, "exists"),
        S::NFS3ERR_NOTDIR => TransportError::reply(550, "not a directory"),
        S::NFS3ERR_ISDIR => TransportError::reply(550, "is a directory"),
        S::NFS3ERR_NOTEMPTY => TransportError::reply(550, "not empty"),
        S::NFS3ERR_NOSPC | S::NFS3ERR_DQUOT | S::NFS3ERR_FBIG => {
            TransportError::reply(552, "no space")
        }
        S::NFS3ERR_JUKEBOX => TransportError::reply(450, "busy"),
        S::NFS3ERR_NAMETOOLONG | S::NFS3ERR_INVAL => TransportError::Rejected,
        S::NFS3ERR_NOTSUPP | S::NFS3ERR_BADTYPE => TransportError::Unsupported("NFS operation"),
        _ => TransportError::status(SftpStatus::Failure, ""),
    }
}

fn time(t: &nfs3::nfstime3) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::new(
        u64::from(t.seconds),
        t.nseconds.min(999_999_999),
    ))
}

fn attrs(a: &nfs3::fattr3) -> RawAttrs {
    RawAttrs {
        kind: match a.type_ {
            nfs3::ftype3::NF3REG => RawKind::File,
            nfs3::ftype3::NF3DIR => RawKind::Dir,
            nfs3::ftype3::NF3LNK => RawKind::Symlink,
            _ => RawKind::Other,
        },
        size: Some(a.size),
        mtime: time(&a.mtime),
        atime: time(&a.atime),
        permissions: Some(a.mode & 0o7777),
    }
}

fn nfs_time(t: SystemTime) -> TResult<nfs3::set_mtime> {
    nfs3::nfstime3::try_from(t)
        .map(nfs3::set_mtime::SET_TO_CLIENT_TIME)
        .map_err(|_| TransportError::Rejected)
}

enum Handle {
    Directory {
        fh: nfs_fh3,
        cookie: nfs3::cookie3,
        verifier: nfs3::cookieverf3,
        eof: bool,
    },
    Reader {
        fh: nfs_fh3,
    },
    Writer {
        fh: nfs_fh3,
        /// Bytes handed to `write` and accepted so far. The sequential-write check compares
        /// against this own count (never a server-confirmed one), so every chunk after the
        /// first passes.
        accepted: u64,
    },
}

pub struct NfsTransport {
    client: Nfs3Client<TokioIo<tokio::net::TcpStream>>,
    root: nfs_fh3,
    handles: HashMap<u64, Handle>,
    next: u64,
    timeout: Duration,
    read_size: u32,
    write_size: u32,
}

impl NfsTransport {
    pub fn connect(profile: &NfsProfile, timeouts: Timeouts) -> TResult<Self> {
        profile.validate().map_err(|_| TransportError::Rejected)?;
        let auth = nfs3_types::rpc::auth_unix {
            stamp: 0,
            machinename: Opaque::borrowed(b"dualfile"),
            uid: profile.uid,
            gid: profile.gid,
            gids: vec![],
        };
        // The NFS service port is fixed by the profile; the mount service is found through the
        // portmapper (111). No privileged local port (that needs root): servers that insist on
        // one need the `insecure` export option.
        let connection = bounded(timeouts.connect, async {
            Nfs3ConnectionBuilder::new(
                TokioConnector,
                profile.host.trim().to_owned(),
                profile.export.clone(),
            )
            .connect_from_privileged_port(false)
            .nfs3_port(profile.port)
            .credential(nfs3_types::rpc::opaque_auth::auth_unix(&auth))
            .mount()
            .await
            .map_err(map_error)
        })?;
        let root = connection.root_nfs_fh3();
        let mut client = connection.into_nfs3_client();
        let (mut read_size, mut write_size) = (DEFAULT_IO, DEFAULT_IO);
        // Best effort: use the transfer sizes the server prefers.
        if let Ok(info) = call(
            timeouts.idle,
            client.fsinfo(nfs3::FSINFO3args {
                fsroot: root.clone(),
            }),
        ) {
            read_size = info.rtmax.clamp(4096, MAX_IO);
            write_size = info.wtmax.clamp(4096, MAX_IO);
        }
        Ok(Self {
            client,
            root,
            handles: HashMap::new(),
            next: 0,
            timeout: timeouts.idle,
            read_size,
            write_size,
        })
    }

    fn insert(&mut self, handle: Handle) -> SftpHandle {
        self.next += 1;
        self.handles.insert(self.next, handle);
        SftpHandle(self.next)
    }

    fn lookup_name(
        &mut self,
        dir: &nfs_fh3,
        name: &str,
    ) -> TResult<(nfs_fh3, Option<nfs3::fattr3>)> {
        let t = self.timeout;
        let found = call(
            t,
            self.client.lookup(nfs3::LOOKUP3args {
                what: nfs3::diropargs3 {
                    dir: dir.clone(),
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
            }),
        )?;
        let attrs = match found.obj_attributes {
            Nfs3Option::Some(a) => Some(a),
            Nfs3Option::None => None,
        };
        Ok((found.object, attrs))
    }

    fn getattr(&mut self, fh: &nfs_fh3) -> TResult<nfs3::fattr3> {
        let t = self.timeout;
        call(
            t,
            self.client
                .getattr(nfs3::GETATTR3args { object: fh.clone() }),
        )
        .map(|ok| ok.obj_attributes)
    }

    /// Walks the path from the mounted root (symbolic links are not followed).
    fn resolve(&mut self, path: &str) -> TResult<(nfs_fh3, nfs3::fattr3)> {
        let mut fh = self.root.clone();
        let mut known = None;
        for name in components(path)? {
            let (next, a) = self.lookup_name(&fh, name)?;
            fh = next;
            known = a;
        }
        let a = match known {
            Some(a) => a,
            None => self.getattr(&fh)?,
        };
        Ok((fh, a))
    }

    /// The directory handle of the parent and the last name of a non-root path.
    fn parent<'p>(&mut self, path: &'p str) -> TResult<(nfs_fh3, &'p str)> {
        let names = components(path)?;
        let Some((last, dirs)) = names.split_last() else {
            return Err(TransportError::Rejected);
        };
        let mut fh = self.root.clone();
        for name in dirs {
            fh = self.lookup_name(&fh, name)?.0;
        }
        Ok((fh, last))
    }

    fn create_file(&mut self, path: &str, how: OpenWrite) -> TResult<nfs_fh3> {
        let (dir, name) = self.parent(path)?;
        let mode = nfs3::sattr3 {
            mode: Nfs3Option::Some(0o644),
            ..Default::default()
        };
        let how = match how {
            OpenWrite::CreateNew => nfs3::createhow3::GUARDED(mode),
            OpenWrite::CreateTruncate => nfs3::createhow3::UNCHECKED(nfs3::sattr3 {
                size: Nfs3Option::Some(0),
                ..mode
            }),
            OpenWrite::Existing => return Err(TransportError::Unsupported("NFS resume")),
        };
        let t = self.timeout;
        let made = call(
            t,
            self.client.create(nfs3::CREATE3args {
                where_: nfs3::diropargs3 {
                    dir: dir.clone(),
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
                how,
            }),
        )?;
        match made.obj {
            Nfs3Option::Some(fh) => Ok(fh),
            Nfs3Option::None => Ok(self.lookup_name(&dir, name)?.0),
        }
    }

    fn rename_inner(&mut self, from: &str, to: &str, replace: bool) -> TResult<()> {
        let (from_dir, from_name) = self.parent(from)?;
        let (to_dir, to_name) = self.parent(to)?;
        if !replace {
            match self.lookup_name(&to_dir, to_name) {
                Ok(_) => return Err(TransportError::reply(550, "exists")),
                Err(TransportError::Status {
                    code: SftpStatus::NoSuchFile,
                    ..
                }) => {}
                Err(other) => return Err(other),
            }
        }
        let t = self.timeout;
        call(
            t,
            self.client.rename(nfs3::RENAME3args {
                from: nfs3::diropargs3 {
                    dir: from_dir,
                    name: nfs3::filename3(Opaque::borrowed(from_name.as_bytes())),
                },
                to: nfs3::diropargs3 {
                    dir: to_dir,
                    name: nfs3::filename3(Opaque::borrowed(to_name.as_bytes())),
                },
            }),
        )
        .map(|_| ())
    }
}

/// The path components (root = none). Control characters (CR, LF, NUL) and `..` are refused.
fn components(path: &str) -> TResult<Vec<&str>> {
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return Err(TransportError::Rejected);
    }
    let names: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if names.iter().any(|part| matches!(*part, "." | "..")) {
        return Err(TransportError::Rejected);
    }
    Ok(names)
}

impl SftpTransport for NfsTransport {
    fn space_free(&mut self, _path: &str) -> TResult<Option<dualfile_core::vfs::SpaceFree>> {
        let t = self.timeout;
        let root = self.root.clone();
        let stat = call(t, self.client.fsstat(nfs3::FSSTAT3args { fsroot: root }))?;
        Ok(Some(dualfile_core::vfs::SpaceFree {
            available: stat.abytes,
            total: stat.tbytes,
        }))
    }
    fn features(&self) -> SftpFeatures {
        // NFS RENAME replaces an existing target (POSIX); `rename` checks first.
        SftpFeatures { posix_rename: true }
    }
    fn realpath(&mut self, _: &str) -> TResult<String> {
        Ok("/".into())
    }
    fn lstat(&mut self, path: &str) -> TResult<RawAttrs> {
        self.resolve(path).map(|(_, a)| attrs(&a))
    }
    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle> {
        let (fh, a) = self.resolve(path)?;
        if !matches!(a.type_, nfs3::ftype3::NF3DIR) {
            return Err(TransportError::reply(550, "not a directory"));
        }
        Ok(self.insert(Handle::Directory {
            fh,
            cookie: nfs3::cookie3::default(),
            verifier: nfs3::cookieverf3::default(),
            eof: false,
        }))
    }
    fn read_dir(&mut self, h: SftpHandle) -> TResult<Vec<RawEntry>> {
        let t = self.timeout;
        let Some(Handle::Directory {
            fh,
            cookie,
            verifier,
            eof,
        }) = self.handles.get_mut(&h.0)
        else {
            return Err(TransportError::Rejected);
        };
        if *eof {
            return Ok(Vec::new());
        }
        let dir = fh.clone();
        let page = call(
            t,
            self.client.readdirplus(nfs3::READDIRPLUS3args {
                dir: dir.clone(),
                cookie: *cookie,
                cookieverf: *verifier,
                dircount: 32 * 1024,
                maxcount: 128 * 1024,
            }),
        )?;
        let entries = page.reply.entries.into_inner();
        *verifier = page.cookieverf;
        // An empty page without `eof` would loop forever: treat it as the end.
        *eof = page.reply.eof || entries.is_empty();
        if let Some(last) = entries.last() {
            *cookie = last.cookie;
        }
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = String::from_utf8_lossy(&entry.name.0).into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let a = match entry.name_attributes {
                Nfs3Option::Some(a) => a,
                Nfs3Option::None => {
                    // The server sent no attributes for this name: ask for them.
                    match self.lookup_name(&dir, &name) {
                        Ok((_, Some(a))) => a,
                        Ok((fh, None)) => self.getattr(&fh)?,
                        // Removed between the two calls.
                        Err(TransportError::Status {
                            code: SftpStatus::NoSuchFile,
                            ..
                        }) => continue,
                        Err(other) => return Err(other),
                    }
                }
            };
            out.push(RawEntry {
                name,
                attrs: attrs(&a),
            });
        }
        Ok(out)
    }
    fn open_read(&mut self, path: &str) -> TResult<SftpHandle> {
        let (fh, a) = self.resolve(path)?;
        match a.type_ {
            nfs3::ftype3::NF3REG => Ok(self.insert(Handle::Reader { fh })),
            nfs3::ftype3::NF3DIR => Err(TransportError::reply(550, "is a directory")),
            _ => Err(TransportError::Unsupported("NFS special file")),
        }
    }
    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle> {
        let fh = self.create_file(path, how)?;
        Ok(self.insert(Handle::Writer { fh, accepted: 0 }))
    }
    fn read(&mut self, h: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize> {
        let Some(Handle::Reader { fh }) = self.handles.get(&h.0) else {
            return Err(TransportError::Rejected);
        };
        let count = u32::try_from(buf.len())
            .unwrap_or(u32::MAX)
            .min(self.read_size);
        let file = fh.clone();
        let t = self.timeout;
        let data = call(
            t,
            self.client.read(nfs3::READ3args {
                file,
                offset,
                count,
            }),
        )?;
        let n = data.data.len().min(buf.len());
        buf[..n].copy_from_slice(&data.data[..n]);
        Ok(n)
    }
    fn write(&mut self, h: SftpHandle, offset: u64, data: &[u8]) -> TResult<()> {
        let t = self.timeout;
        let chunk = self.write_size as usize;
        let Some(Handle::Writer { fh, accepted }) = self.handles.get_mut(&h.0) else {
            return Err(TransportError::Rejected);
        };
        // Writes are sequential; compare against the bytes we accepted ourselves.
        if offset != *accepted {
            return Err(TransportError::Rejected);
        }
        let mut done = 0usize;
        while done < data.len() {
            let part = &data[done..data.len().min(done + chunk)];
            let reply = call(
                t,
                self.client.write(nfs3::WRITE3args {
                    file: fh.clone(),
                    offset: offset + done as u64,
                    count: part.len() as u32,
                    stable: nfs3::stable_how::FILE_SYNC,
                    data: Opaque::borrowed(part),
                }),
            )?;
            if reply.count == 0 {
                return Err(TransportError::status(SftpStatus::Failure, ""));
            }
            done += (reply.count as usize).min(part.len());
        }
        *accepted += data.len() as u64;
        Ok(())
    }
    fn fstat(&mut self, h: SftpHandle) -> TResult<RawAttrs> {
        let fh = match self.handles.get(&h.0) {
            Some(
                Handle::Reader { fh } | Handle::Writer { fh, .. } | Handle::Directory { fh, .. },
            ) => fh.clone(),
            None => return Err(TransportError::Rejected),
        };
        self.getattr(&fh).map(|a| attrs(&a))
    }
    fn close(&mut self, h: SftpHandle) -> TResult<()> {
        // Writes are FILE_SYNC: nothing to commit.
        self.handles
            .remove(&h.0)
            .map(|_| ())
            .ok_or(TransportError::Rejected)
    }
    fn mkdir(&mut self, path: &str) -> TResult<()> {
        let (dir, name) = self.parent(path)?;
        let t = self.timeout;
        call(
            t,
            self.client.mkdir(nfs3::MKDIR3args {
                where_: nfs3::diropargs3 {
                    dir,
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
                attributes: nfs3::sattr3 {
                    mode: Nfs3Option::Some(0o755),
                    ..Default::default()
                },
            }),
        )
        .map(|_| ())
    }
    fn rmdir(&mut self, path: &str) -> TResult<()> {
        let (dir, name) = self.parent(path)?;
        let t = self.timeout;
        call(
            t,
            self.client.rmdir(nfs3::RMDIR3args {
                object: nfs3::diropargs3 {
                    dir,
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
            }),
        )
        .map(|_| ())
    }
    fn remove(&mut self, path: &str) -> TResult<()> {
        let (dir, name) = self.parent(path)?;
        let t = self.timeout;
        call(
            t,
            self.client.remove(nfs3::REMOVE3args {
                object: nfs3::diropargs3 {
                    dir,
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
            }),
        )
        .map(|_| ())
    }
    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        self.rename_inner(from, to, false)
    }
    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()> {
        self.rename_inner(from, to, true)
    }
    fn readlink(&mut self, path: &str) -> TResult<String> {
        let (fh, _) = self.resolve(path)?;
        let t = self.timeout;
        let link = call(t, self.client.readlink(nfs3::READLINK3args { symlink: fh }))?;
        String::from_utf8(link.data.0.to_vec()).map_err(|_| TransportError::Rejected)
    }
    fn symlink(&mut self, target: &str, at: &str) -> TResult<()> {
        if target.chars().any(char::is_control) {
            return Err(TransportError::Rejected);
        }
        let (dir, name) = self.parent(at)?;
        let t = self.timeout;
        call(
            t,
            self.client.symlink(nfs3::SYMLINK3args {
                where_: nfs3::diropargs3 {
                    dir,
                    name: nfs3::filename3(Opaque::borrowed(name.as_bytes())),
                },
                symlink: nfs3::symlinkdata3 {
                    symlink_attributes: nfs3::sattr3::default(),
                    symlink_data: nfs3::nfspath3(Opaque::borrowed(target.as_bytes())),
                },
            }),
        )
        .map(|_| ())
    }
    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        atime: Option<SystemTime>,
    ) -> TResult<()> {
        if permissions.is_none() && mtime.is_none() && atime.is_none() {
            return Ok(());
        }
        let (fh, _) = self.resolve(path)?;
        let mut new = nfs3::sattr3::default();
        if let Some(mode) = permissions {
            new.mode = Nfs3Option::Some(mode & 0o7777);
        }
        if let Some(t) = mtime {
            new.mtime = nfs_time(t)?;
        }
        if let Some(t) = atime {
            new.atime = match nfs_time(t)? {
                nfs3::set_mtime::SET_TO_CLIENT_TIME(v) => nfs3::set_atime::SET_TO_CLIENT_TIME(v),
                _ => nfs3::set_atime::DONT_CHANGE,
            };
        }
        let t = self.timeout;
        call(
            t,
            self.client.setattr(nfs3::SETATTR3args {
                object: fh,
                new_attributes: new,
                guard: Nfs3Option::None,
            }),
        )
        .map(|_| ())
    }
    fn quit(&mut self) {
        self.handles.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{components, map_error, map_status, valid_export, NfsProfile};
    use crate::error::{SftpStatus, TransportError};
    use dualfile_core::vfs::{format_ipc_path, parse_ipc_path, Scheme, VPath};

    fn profile() -> NfsProfile {
        NfsProfile {
            name: "NAS".into(),
            host: "nas".into(),
            port: 2049,
            export: "/srv/share".into(),
            uid: 1000,
            gid: 1000,
            start_folder: None,
        }
    }

    #[test]
    fn nfs_profile_validation_has_no_password_and_checks_export_host_port() {
        let mut p = profile();
        assert!(p.validate().is_ok());
        p.start_folder = Some("/a/b".into());
        assert!(p.validate().is_ok());
        p.start_folder = Some("/a/../b".into());
        assert!(p.validate().is_err());
        p.start_folder = None;
        for export in ["", "share", "/a\\b", "/a/../b", "/bad\n"] {
            p.export = export.into();
            assert!(p.validate().is_err(), "{export:?}");
        }
        assert!(valid_export("/"));
        p.export = "/srv".into();
        p.port = 0;
        assert!(p.validate().is_err());
        p.port = 2049;
        p.host.clear();
        assert!(p.validate().is_err());
        p.host = "a b".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn nfs_paths_and_ipc_round_trip() {
        assert_eq!(components("/").unwrap(), Vec::<&str>::new());
        assert_eq!(components("/a//b/").unwrap(), ["a", "b"]);
        for bad in ["a", "/a/../b", "/a\0b", "/a\nb", "/./x"] {
            assert!(components(bad).is_err(), "{bad:?}");
        }
        let p = parse_ipc_path("nfs:c1!/folder").unwrap();
        assert_eq!(p.scheme(), Scheme::Nfs);
        assert!(p.scheme().is_server() && p.scheme().is_remote());
        assert_eq!(format_ipc_path(&p), "nfs:c1!/folder");
        assert_eq!(VPath::nfs("c1", "/folder").unwrap(), p);
    }

    #[test]
    fn nfs_errors_map_to_fixed_texts() {
        use nfs3_types::nfs3::nfsstat3 as S;
        assert!(matches!(
            map_status(S::NFS3ERR_NOENT),
            TransportError::Status {
                code: SftpStatus::NoSuchFile,
                ..
            }
        ));
        assert!(matches!(
            map_status(S::NFS3ERR_ACCES),
            TransportError::Status {
                code: SftpStatus::PermissionDenied,
                ..
            }
        ));
        let e = map_error(nfs3_client::error::Error::Io(std::io::Error::other(
            "password-MARKER",
        )));
        assert!(e.is_connection_lost());
        assert!(!e.to_string().contains("password-MARKER"));
        assert!(matches!(
            map_error(nfs3_client::error::Error::Rpc(
                nfs3_client::error::RpcError::Auth
            )),
            TransportError::Auth
        ));
    }
}
