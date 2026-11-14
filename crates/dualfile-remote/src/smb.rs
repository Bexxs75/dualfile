//! Native SMB uses the handle-based remote VFS and its transfer/reconnect budgets.
//! Negotiation is checked BEFORE NTLM credentials are sent. No library error text escapes.
use crate::error::{SftpStatus, TResult, TransportError};
use crate::secret::Secret;
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::transport::Timeouts;
use smb2::client::{connection::Connection, shares, stream};
use smb2::{DirectoryReader, FileReader, FileWriter, Tree};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub type SmbVfs<T> = crate::sftpvfs::SftpVfs<T>;

#[derive(Clone, Debug)]
pub struct SmbProfile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub domain: String,
    pub share: String,
    pub guest: bool,
    pub start_folder: Option<String>,
}

impl SmbProfile {
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
        if !valid_share(&self.share) {
            return Err("Ungültige SMB-Freigabe");
        }
        if self.user.len() > 256
            || self.domain.len() > 256
            || self
                .user
                .chars()
                .chain(self.domain.chars())
                .any(char::is_control)
            || (!self.guest && (self.user.is_empty() || self.user.eq_ignore_ascii_case("guest")))
        {
            return Err("Ungültiger SMB-Benutzer (Gast ausdrücklich auswählen)");
        }
        if let Some(path) = &self.start_folder {
            dualfile_core::vfs::VPath::smb("check", path).map_err(|_| "Ungültiger Startordner")?;
        }
        Ok(())
    }
}

pub fn valid_share(share: &str) -> bool {
    share.len() <= 255
        && !matches!(share, "." | "..")
        && !share
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
}

fn bounded<T>(timeout: Duration, future: impl Future<Output = TResult<T>>) -> TResult<T> {
    crate::russh_transport::runtime().block_on(async {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| TransportError::Timeout)?
    })
}

fn run<T>(timeout: Duration, future: impl Future<Output = smb2::Result<T>>) -> TResult<T> {
    bounded(timeout, async { future.await.map_err(map_error) })
}

fn check_security(
    dialect: smb2::types::Dialect,
    signing_required: bool,
    guest: bool,
) -> TResult<()> {
    if dialect < smb2::types::Dialect::Smb2_1 || (guest && signing_required) {
        return Err(TransportError::SmbSecurity);
    }
    Ok(())
}

pub fn map_error(error: smb2::Error) -> TransportError {
    use smb2::ErrorKind as K;
    if matches!(
        error,
        smb2::Error::EncryptionRequired | smb2::Error::EncryptionUnavailable { .. }
    ) {
        return TransportError::SmbSecurity;
    }
    match error.kind() {
        K::AuthRequired | K::GuestSessionRefused => TransportError::SmbAuth,
        K::SignatureInvalid | K::SigningRequired | K::NegotiateValidationFailed => {
            TransportError::SmbSecurity
        }
        K::ConnectionLost | K::SessionExpired | K::Io => TransportError::SmbUnreachable,
        K::TimedOut => TransportError::Timeout,
        K::NotFound => TransportError::status(SftpStatus::NoSuchFile, ""),
        K::AccessDenied => TransportError::status(SftpStatus::PermissionDenied, ""),
        K::AlreadyExists => TransportError::reply(550, "exists"),
        K::NotADirectory => TransportError::reply(550, "not a directory"),
        K::Unsupported => TransportError::Unsupported("SMB operation"),
        _ => TransportError::SmbFailure,
    }
}

fn share_error(error: TransportError) -> TransportError {
    match error {
        TransportError::Status {
            code: SftpStatus::NoSuchFile,
            ..
        } => TransportError::SmbShareMissing,
        other => other,
    }
}

fn attrs(info: &smb2::FileInfo) -> RawAttrs {
    RawAttrs {
        kind: if info.is_link() {
            RawKind::Symlink
        } else if info.is_directory {
            RawKind::Dir
        } else {
            RawKind::File
        },
        size: Some(info.size),
        mtime: info.modified.to_system_time(),
        atime: info.accessed.to_system_time(),
        permissions: None,
    }
}
fn dir_attrs() -> RawAttrs {
    RawAttrs {
        kind: RawKind::Dir,
        size: Some(0),
        mtime: None,
        atime: None,
        permissions: None,
    }
}

enum Handle {
    Directory(Box<DirectoryReader>),
    Shares(Vec<RawEntry>),
    Reader(Box<FileReader>),
    Writer {
        writer: Box<FileWriter>,
        path: String,
        tree: Arc<Tree>,
    },
}

pub struct SmbTransport {
    conn: Connection,
    share: String,
    trees: HashMap<String, Arc<Tree>>,
    handles: HashMap<u64, Handle>,
    next: u64,
    timeout: Duration,
}

impl SmbTransport {
    pub fn connect(profile: &SmbProfile, secret: &Secret, timeouts: Timeouts) -> TResult<Self> {
        profile.validate().map_err(|_| TransportError::Rejected)?;
        let host = profile.host.trim_matches(['[', ']']);
        let addr = if host.contains(':') {
            format!("[{host}]:{}", profile.port)
        } else {
            format!("{host}:{}", profile.port)
        };
        let started = Instant::now();
        let conn = bounded(timeouts.connect, async {
            let mut conn = Connection::connect(&addr, timeouts.connect)
                .await
                .map_err(map_error)?;
            conn.set_response_timeout(Some(timeouts.idle));
            conn.negotiate().await.map_err(map_error)?;
            let params = conn.params().ok_or(TransportError::SmbSecurity)?;
            check_security(params.dialect, params.signing_required, profile.guest)?;
            smb2::Session::setup(
                &mut conn,
                if profile.guest {
                    "Guest"
                } else {
                    &profile.user
                },
                if profile.guest { "" } else { secret.expose() },
                &profile.domain,
            )
            .await
            .map_err(map_error)?;
            Ok(conn)
        })?;
        let mut result = Self {
            conn,
            share: profile.share.clone(),
            trees: HashMap::new(),
            handles: HashMap::new(),
            next: 0,
            timeout: timeouts.idle,
        };
        if !profile.share.is_empty() {
            let tree = run(
                timeouts.connect.saturating_sub(started.elapsed()),
                Tree::connect(&mut result.conn, &profile.share),
            )
            .map_err(share_error)?;
            result.trees.insert(profile.share.clone(), Arc::new(tree));
        }
        Ok(result)
    }

    fn insert(&mut self, handle: Handle) -> SftpHandle {
        self.next += 1;
        self.handles.insert(self.next, handle);
        SftpHandle(self.next)
    }

    fn location(&mut self, path: &str) -> TResult<(Arc<Tree>, String)> {
        let (share, relative) = split_path(&self.share, path)?;
        if !self.trees.contains_key(&share) {
            let tree =
                run(self.timeout, Tree::connect(&mut self.conn, &share)).map_err(share_error)?;
            self.trees.insert(share.clone(), Arc::new(tree));
        }
        Ok((self.trees[&share].clone(), relative))
    }
}

/// With no fixed share the first component selects a share; never accept traversal.
pub fn split_path(share: &str, path: &str) -> TResult<(String, String)> {
    let normalized =
        dualfile_core::vfs::VPath::smb("check", path).map_err(|_| TransportError::Rejected)?;
    let path = normalized
        .path()
        .to_str()
        .ok_or(TransportError::Rejected)?
        .trim_start_matches('/');
    let (share, path) = if share.is_empty() {
        path.split_once('/').unwrap_or((path, ""))
    } else {
        (share, path)
    };
    if share.is_empty() || !valid_share(share) {
        return Err(TransportError::Rejected);
    }
    Ok((share.to_owned(), path.to_owned()))
}

impl SftpTransport for SmbTransport {
    fn features(&self) -> SftpFeatures {
        SftpFeatures { posix_rename: true }
    }
    fn realpath(&mut self, _: &str) -> TResult<String> {
        Ok("/".into())
    }
    fn lstat(&mut self, path: &str) -> TResult<RawAttrs> {
        if self.share.is_empty() && path == "/" {
            return Ok(dir_attrs());
        }
        let (tree, path) = self.location(path)?;
        run(self.timeout, tree.stat(&mut self.conn, &path)).map(|i| attrs(&i))
    }
    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle> {
        if self.share.is_empty() && path == "/" {
            let entries = run(self.timeout, shares::list_shares(&mut self.conn))?
                .into_iter()
                .map(|s| RawEntry {
                    name: s.name,
                    attrs: dir_attrs(),
                })
                .collect();
            return Ok(self.insert(Handle::Shares(entries)));
        }
        let (tree, path) = self.location(path)?;
        let reader = run(
            self.timeout,
            tree.open_directory_reader(self.conn.clone(), &path),
        )?;
        Ok(self.insert(Handle::Directory(Box::new(reader))))
    }
    fn read_dir(&mut self, h: SftpHandle) -> TResult<Vec<RawEntry>> {
        match self.handles.get_mut(&h.0) {
            Some(Handle::Shares(entries)) => Ok(std::mem::take(entries)),
            Some(Handle::Directory(reader)) => Ok(run(self.timeout, reader.next_batch())?
                .unwrap_or_default()
                .into_iter()
                .map(|e| {
                    let kind = if e.is_link() {
                        RawKind::Symlink
                    } else if e.is_directory {
                        RawKind::Dir
                    } else {
                        RawKind::File
                    };
                    RawEntry {
                        name: e.name,
                        attrs: RawAttrs {
                            kind,
                            size: Some(e.size),
                            mtime: e.modified.to_system_time(),
                            atime: None,
                            permissions: None,
                        },
                    }
                })
                .collect()),
            _ => Err(TransportError::Rejected),
        }
    }
    fn open_read(&mut self, path: &str) -> TResult<SftpHandle> {
        let (tree, path) = self.location(path)?;
        let reader = run(
            self.timeout,
            stream::open_file_reader(tree, self.conn.clone(), &path),
        )?;
        Ok(self.insert(Handle::Reader(Box::new(reader))))
    }
    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle> {
        let (tree, path) = self.location(path)?;
        let writer = match how {
            OpenWrite::CreateNew => run(
                self.timeout,
                stream::open_file_writer_exclusive(tree.clone(), self.conn.clone(), &path),
            )?,
            OpenWrite::CreateTruncate => run(
                self.timeout,
                stream::open_file_writer(tree.clone(), self.conn.clone(), &path),
            )?,
            OpenWrite::Existing => return Err(TransportError::Unsupported("SMB resume")),
        };
        Ok(self.insert(Handle::Writer {
            writer: Box::new(writer),
            path,
            tree,
        }))
    }
    fn read(&mut self, h: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize> {
        let Some(Handle::Reader(reader)) = self.handles.get(&h.0) else {
            return Err(TransportError::Rejected);
        };
        let data = run(self.timeout, reader.read_at(offset, buf.len() as u64))?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
    fn write(&mut self, h: SftpHandle, offset: u64, data: &[u8]) -> TResult<()> {
        let Some(Handle::Writer { writer, .. }) = self.handles.get_mut(&h.0) else {
            return Err(TransportError::Rejected);
        };
        if offset != writer.bytes_written() {
            return Err(TransportError::Rejected);
        }
        run(self.timeout, writer.write_chunk(data))
    }
    fn fstat(&mut self, h: SftpHandle) -> TResult<RawAttrs> {
        match self.handles.get_mut(&h.0) {
            Some(Handle::Reader(reader)) => Ok(attrs(reader.info())),
            Some(Handle::Writer { writer, tree, path }) => {
                run(self.timeout, writer.flush_checkpoint())?;
                run(self.timeout, tree.stat(&mut self.conn, path)).map(|i| attrs(&i))
            }
            _ => Err(TransportError::Rejected),
        }
    }
    fn close(&mut self, h: SftpHandle) -> TResult<()> {
        match self.handles.remove(&h.0) {
            Some(Handle::Reader(r)) => run(self.timeout, (*r).close()),
            Some(Handle::Writer { writer, .. }) => {
                run(self.timeout, (*writer).finish()).map(|_| ())
            }
            Some(Handle::Directory(r)) => run(self.timeout, (*r).close()),
            Some(Handle::Shares(_)) => Ok(()),
            None => Err(TransportError::Rejected),
        }
    }
    fn mkdir(&mut self, path: &str) -> TResult<()> {
        let (tree, path) = self.location(path)?;
        if path.is_empty() {
            return Err(TransportError::Rejected);
        }
        run(self.timeout, tree.create_directory(&mut self.conn, &path))
    }
    fn rmdir(&mut self, path: &str) -> TResult<()> {
        let (tree, path) = self.location(path)?;
        if path.is_empty() {
            return Err(TransportError::Rejected);
        }
        run(self.timeout, tree.delete_directory(&mut self.conn, &path))
    }
    fn remove(&mut self, path: &str) -> TResult<()> {
        let (tree, path) = self.location(path)?;
        if path.is_empty() {
            return Err(TransportError::Rejected);
        }
        let info = run(self.timeout, tree.stat(&mut self.conn, &path))?;
        if info.is_link() {
            run(self.timeout, tree.delete_link(&mut self.conn, &path))
        } else {
            run(self.timeout, tree.delete_file(&mut self.conn, &path))
        }
    }
    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        self.rename_inner(from, to, false)
    }
    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()> {
        self.rename_inner(from, to, true)
    }
    fn readlink(&mut self, _: &str) -> TResult<String> {
        Err(TransportError::Unsupported("SMB link target"))
    }
    fn symlink(&mut self, _: &str, _: &str) -> TResult<()> {
        Err(TransportError::Unsupported("SMB symlink"))
    }
    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        atime: Option<SystemTime>,
    ) -> TResult<()> {
        if permissions.is_some() {
            return Err(TransportError::Unsupported("SMB POSIX permissions"));
        }
        let (tree, path) = self.location(path)?;
        let mut times = smb2::FileTimes::new();
        if let Some(t) = mtime {
            times = times.set_modified(t);
        }
        if let Some(t) = atime {
            times = times.set_accessed(t);
        }
        run(self.timeout, tree.set_times(&mut self.conn, &path, times))
    }
    fn quit(&mut self) {
        self.handles.clear();
        self.trees.clear();
    }
}

impl SmbTransport {
    fn rename_inner(&mut self, from: &str, to: &str, replace: bool) -> TResult<()> {
        let (a, from) = split_path(&self.share, from)?;
        let (b, to) = split_path(&self.share, to)?;
        if a != b || from.is_empty() || to.is_empty() {
            return Err(TransportError::Rejected);
        }
        let full = if self.share.is_empty() {
            format!("/{a}/{from}")
        } else {
            format!("/{from}")
        };
        let (tree, _) = self.location(&full)?;
        run(
            self.timeout,
            tree.rename_with(
                &mut self.conn,
                &from,
                &to,
                smb2::RenameOptions::new().replace_if_exists(replace),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{map_error, split_path, SmbProfile, SmbVfs};
    use crate::error::TransportError;
    use crate::profile::SftpAuth;
    use crate::secret::Secret;
    use crate::sftp_fake::FakeSftpServer;
    use crate::sftpvfs::SftpConfig;
    use dualfile_core::vfs::{
        format_ipc_path, parse_ipc_path, ReplaceMode, Scheme, VPath, Vfs, WriteMode,
    };
    use std::io::{Read, Write};
    use std::sync::Arc;

    #[test]
    fn smb_listing_budget_rejects_an_oversized_single_batch() {
        let server = FakeSftpServer::new();
        server.put("/one", b"1");
        server.put("/two", b"2");
        let first = server
            .connect("user", &SftpAuth::Password, &Secret::new("secret"))
            .unwrap();
        let copy = server.clone();
        let opener =
            Arc::new(move || copy.connect("user", &SftpAuth::Password, &Secret::new("secret")));
        let mut cfg = SftpConfig::default();
        cfg.limits.max_entries = 1;
        let vfs = SmbVfs::with_scheme("budget", first, opener, cfg, None, Scheme::Smb).unwrap();
        assert!(vfs.list(&VPath::smb("budget", "/").unwrap()).is_err());
    }

    #[test]
    fn smb_security_policy_and_timeouts_are_enforced_without_network() {
        use smb2::types::Dialect;
        use std::time::Duration;
        assert!(super::check_security(Dialect::Smb2_0_2, false, false).is_err());
        assert!(super::check_security(Dialect::Smb2_1, true, false).is_ok());
        assert!(super::check_security(Dialect::Smb3_1_1, true, true).is_err());
        assert!(super::check_security(Dialect::Smb3_1_1, false, true).is_ok());
        let result: super::TResult<()> =
            super::bounded(Duration::from_millis(1), std::future::pending());
        assert!(matches!(result, Err(TransportError::Timeout)));
    }

    #[test]
    fn smb_paths_keep_the_share_boundary_and_connection_identity() {
        assert_eq!(
            split_path("team", "/a/b").unwrap(),
            ("team".into(), "a/b".into())
        );
        assert_eq!(
            split_path("", "/team/a").unwrap(),
            ("team".into(), "a".into())
        );
        for path in ["/../x", "relative", "/a/../b", "/a\0b"] {
            assert!(split_path("team", path).is_err());
        }
        assert!(split_path("", "/").is_err());
        let p = parse_ipc_path("smb:c1!/folder").unwrap();
        assert_eq!(p.scheme(), Scheme::Smb);
        assert!(p.scheme().is_server());
        assert_eq!(format_ipc_path(&p), "smb:c1!/folder");
    }

    #[test]
    fn smb_auth_is_explicit_and_profile_has_no_password() {
        let mut p = SmbProfile {
            name: "NAS".into(),
            host: "nas".into(),
            port: 445,
            user: "user".into(),
            domain: "WORKGROUP".into(),
            share: "".into(),
            guest: false,
            start_folder: None,
        };
        assert!(p.validate().is_ok());
        p.user.clear();
        assert!(p.validate().is_err());
        p.guest = true;
        assert!(p.validate().is_ok());
        for share in ["a/b", "a\\b", "..", "bad\n"] {
            p.share = share.into();
            assert!(p.validate().is_err());
        }
        p.share.clear();
        p.port = 0;
        assert!(p.validate().is_err());
        p.port = 445;
        p.host.clear();
        assert!(p.validate().is_err());
    }

    #[test]
    fn smb_errors_never_echo_server_supplied_secrets() {
        let e = map_error(smb2::Error::Io(std::io::Error::other("password-MARKER")));
        assert!(e.is_connection_lost());
        assert!(matches!(
            super::share_error(TransportError::status(
                crate::error::SftpStatus::NoSuchFile,
                "password-MARKER"
            )),
            TransportError::SmbShareMissing
        ));
        assert!(matches!(
            map_error(smb2::Error::Auth {
                message: "password-MARKER".into()
            }),
            TransportError::SmbAuth
        ));
        assert!(!e.to_string().contains("password-MARKER"));
        assert!(matches!(
            map_error(smb2::Error::Timeout),
            TransportError::Timeout
        ));
        assert!(matches!(
            map_error(smb2::Error::EncryptionRequired),
            TransportError::SmbSecurity
        ));
    }

    #[test]
    fn smb_vfs_fake_transfers_abort_rename_delete_and_disconnect() {
        let server = FakeSftpServer::new();
        server.mkdirs("/team");
        server.put("/team/read", b"content");
        let first = server
            .connect("user", &SftpAuth::Password, &Secret::new("secret"))
            .unwrap();
        let copy = server.clone();
        let opener =
            Arc::new(move || copy.connect("user", &SftpAuth::Password, &Secret::new("secret")));
        let vfs = SmbVfs::with_scheme(
            "smb1",
            first,
            opener,
            SftpConfig::default(),
            Some(Secret::new("secret")),
            Scheme::Smb,
        )
        .unwrap();
        let p = |path| VPath::smb("smb1", path).unwrap();
        assert_eq!(vfs.remote_endpoint(), Some((Scheme::Smb, "smb1")));
        assert!(vfs.list(&VPath::sftp("smb1", "/").unwrap()).is_err());
        assert_eq!(vfs.list(&p("/team")).unwrap().len(), 1);
        let mut data = Vec::new();
        vfs.open_read(&p("/team/read"))
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(data, b"content");
        let mut writer = vfs
            .create_write(&p("/team/new"), WriteMode::NoReplace)
            .unwrap();
        writer.write_all(b"new").unwrap();
        writer.commit().unwrap();
        let mut aborted = vfs
            .create_write(&p("/team/aborted"), WriteMode::NoReplace)
            .unwrap();
        aborted.write_all(b"aborted").unwrap();
        drop(aborted);
        assert_eq!(server.names_in("/team"), ["new", "read"]);
        vfs.rename(&p("/team/new"), &p("/team/renamed"), ReplaceMode::NoReplace)
            .unwrap();
        assert_eq!(server.get("/team/renamed").unwrap(), b"new");
        assert!(!vfs.capabilities().can_trash);
        vfs.remove_permanent(&p("/team/renamed")).unwrap();
        server.state().refuse_connects_after = Some(1);
        server.disconnect_everybody();
        assert!(vfs.list(&p("/team")).is_err());
        assert!(!vfs.is_live());
        vfs.close();
        assert!(!vfs.is_live());
        assert!(vfs.list(&p("/team")).is_err());
    }
}
