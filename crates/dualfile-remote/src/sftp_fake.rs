//! An SFTP server kept in memory, for tests of the layers above the wire (no sockets, no SSH).
//! It follows OpenSSH's behavior where `SftpVfs` depends on it: opening with "must not exist"
//! fails for an existing name, the plain `rename` refuses an existing target (switchable),
//! `posix-rename` replaces, every refusal that has no status of its own is the catch-all
//! failure, a lost connection keeps the bytes that had arrived, and everything the client
//! sends is logged (passwords as `***`). The host key check runs the same code as the real
//! connection (`sftp_trust`).

use crate::error::{SftpStatus, TResult, TransportError};
use crate::profile::{SftpAuth, SftpConnectParams};
use crate::secret::Secret;
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::trust::CertVerifier;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub enum SNode {
    Dir,
    File {
        data: Vec<u8>,
        mtime: u64,
        mode: u32,
    },
    Link {
        target: String,
    },
}

/// The host key the fake server presents.
#[derive(Clone, Debug)]
pub struct HostKey {
    /// The signature algorithm it proves the key with (`ssh-ed25519`, `rsa-sha2-256`, ...).
    pub signature_algorithm: String,
    /// The type of the key itself (`ssh-ed25519`, `ssh-rsa`, ...).
    pub key_type: String,
    pub blob: Vec<u8>,
}

impl HostKey {
    pub fn ed25519(seed: u8) -> Self {
        Self {
            signature_algorithm: "ssh-ed25519".into(),
            key_type: "ssh-ed25519".into(),
            blob: vec![seed; 51],
        }
    }
}

#[derive(Debug)]
pub struct SftpServerState {
    pub nodes: BTreeMap<String, SNode>,
    pub user: String,
    pub password: String,
    /// The passphrase a key file login must come with (`None` = any).
    pub key_passphrase: Option<String>,
    pub features: SftpFeatures,
    pub log: Vec<String>,
    /// Bump to cut every open connection.
    pub generation: u64,
    pub connects: u32,
    pub refuse_connects_after: Option<u32>,
    /// The next request never gets an answer.
    pub hang_next: bool,
    /// Writes lose the connection once this many bytes arrived in total (counted from now).
    pub cut_writes_after: Option<usize>,
    /// Names answered for every directory read instead of the real content.
    pub raw_names: Option<Vec<RawEntry>>,
    /// Names per `read_dir` batch.
    pub page_size: usize,
    /// The plain `rename` replaces an existing target (true) or fails (false, OpenSSH).
    pub rename_replaces: bool,
    /// Every error status repeats this text (to look for a leaking password).
    pub echo_in_errors: Option<String>,
    pub host_key: HostKey,
    /// Entries created by "another client" right before the next rename.
    pub inject_before_rename: Option<(String, Vec<u8>)>,
    /// The next request fails with this status.
    pub fail_next: Option<(SftpStatus, String)>,
    /// Bytes received by writes so far.
    pub written: usize,
    /// Handles open right now (all connections).
    pub open_handles: usize,
}

#[derive(Clone)]
pub struct FakeSftpServer(pub Arc<Mutex<SftpServerState>>);

impl FakeSftpServer {
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_owned(), SNode::Dir);
        Self(Arc::new(Mutex::new(SftpServerState {
            nodes,
            user: "user".into(),
            password: "secret".into(),
            key_passphrase: None,
            features: SftpFeatures { posix_rename: true },
            log: Vec::new(),
            generation: 0,
            connects: 0,
            refuse_connects_after: None,
            hang_next: false,
            cut_writes_after: None,
            raw_names: None,
            page_size: 1000,
            rename_replaces: false,
            echo_in_errors: None,
            host_key: HostKey::ed25519(1),
            inject_before_rename: None,
            fail_next: None,
            written: 0,
            open_handles: 0,
        })))
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, SftpServerState> {
        self.0.lock().unwrap()
    }

    pub fn put(&self, path: &str, data: &[u8]) {
        self.put_with_mtime(path, data, 1_700_000_000);
    }

    pub fn put_with_mtime(&self, path: &str, data: &[u8], mtime: u64) {
        let mut state = self.state();
        make_parents(&mut state.nodes, path);
        state.nodes.insert(
            path.to_owned(),
            SNode::File {
                data: data.to_vec(),
                mtime,
                mode: 0o644,
            },
        );
    }

    pub fn mkdirs(&self, path: &str) {
        let mut state = self.state();
        make_parents(&mut state.nodes, path);
        state.nodes.insert(path.to_owned(), SNode::Dir);
    }

    pub fn put_link(&self, path: &str, target: &str) {
        let mut state = self.state();
        make_parents(&mut state.nodes, path);
        state.nodes.insert(
            path.to_owned(),
            SNode::Link {
                target: target.to_owned(),
            },
        );
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self.state().nodes.get(path) {
            Some(SNode::File { data, .. }) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.state().nodes.contains_key(path)
    }

    pub fn names_in(&self, dir: &str) -> Vec<String> {
        let state = self.state();
        let prefix = if dir == "/" {
            "/".to_owned()
        } else {
            format!("{dir}/")
        };
        state
            .nodes
            .keys()
            .filter(|k| k.len() > prefix.len() && k.starts_with(&prefix))
            .filter(|k| !k[prefix.len()..].contains('/'))
            .map(|k| k[prefix.len()..].to_owned())
            .collect()
    }

    pub fn log(&self) -> Vec<String> {
        self.state().log.clone()
    }

    pub fn disconnect_everybody(&self) {
        self.state().generation += 1;
    }

    /// The login, after the host key was accepted.
    pub fn connect(
        &self,
        user: &str,
        auth: &SftpAuth,
        secret: &Secret,
    ) -> TResult<FakeSftpTransport> {
        let mut state = self.state();
        state.connects += 1;
        if let Some(limit) = state.refuse_connects_after {
            if state.connects > limit {
                return Err(TransportError::Connection("Verbindung abgelehnt".into()));
            }
        }
        state.log.push(format!("USER {user}"));
        let ok = user == state.user
            && match auth {
                SftpAuth::Password => {
                    state.log.push("PASSWORD ***".into());
                    secret.expose() == state.password
                }
                SftpAuth::KeyFile { .. } => {
                    state.log.push("PUBLICKEY ***".into());
                    state
                        .key_passphrase
                        .as_deref()
                        .is_none_or(|p| p == secret.expose())
                }
            };
        if !ok {
            return Err(TransportError::Auth);
        }
        state.log.push("SUBSYSTEM sftp".into());
        let generation = state.generation;
        let features = state.features;
        drop(state);
        Ok(FakeSftpTransport {
            server: self.clone(),
            generation,
            features,
            dead: false,
            quit: false,
            next_handle: 1,
            handles: HashMap::new(),
        })
    }
}

impl Default for FakeSftpServer {
    fn default() -> Self {
        Self::new()
    }
}

fn make_parents(nodes: &mut BTreeMap<String, SNode>, path: &str) {
    let mut at = String::new();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push('/');
        at.push_str(part);
        nodes.entry(at.clone()).or_insert(SNode::Dir);
    }
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

enum Handle {
    Dir { pending: Vec<RawEntry> },
    Read { path: String },
    Write { path: String },
}

pub struct FakeSftpTransport {
    server: FakeSftpServer,
    generation: u64,
    features: SftpFeatures,
    dead: bool,
    quit: bool,
    next_handle: u64,
    handles: HashMap<u64, Handle>,
}

impl std::fmt::Debug for FakeSftpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSftpTransport").finish_non_exhaustive()
    }
}

impl Drop for FakeSftpTransport {
    fn drop(&mut self) {
        let mut state = self.server.0.lock().unwrap();
        state.open_handles = state.open_handles.saturating_sub(self.handles.len());
    }
}

fn plain(path: &str) -> TResult<&str> {
    if path.chars().any(char::is_control) {
        Err(TransportError::Rejected)
    } else {
        Ok(path)
    }
}

fn attrs_of(node: &SNode) -> RawAttrs {
    match node {
        SNode::Dir => RawAttrs {
            kind: RawKind::Dir,
            size: Some(4096),
            mtime: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            atime: None,
            permissions: Some(0o755),
        },
        SNode::File { data, mtime, mode } => RawAttrs {
            kind: RawKind::File,
            size: Some(data.len() as u64),
            mtime: Some(UNIX_EPOCH + Duration::from_secs(*mtime)),
            atime: None,
            permissions: Some(*mode),
        },
        SNode::Link { target } => RawAttrs {
            kind: RawKind::Symlink,
            size: Some(target.len() as u64),
            mtime: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            atime: None,
            permissions: Some(0o777),
        },
    }
}

fn begin<'a>(
    server: &'a FakeSftpServer,
    generation: u64,
    dead: &mut bool,
    quit: bool,
    command: String,
) -> TResult<std::sync::MutexGuard<'a, SftpServerState>> {
    if *dead || quit {
        return Err(TransportError::Connection("Verbindung getrennt".into()));
    }
    let mut state = server.0.lock().unwrap();
    if state.generation != generation {
        *dead = true;
        return Err(TransportError::Connection("Verbindung getrennt".into()));
    }
    if state.hang_next {
        state.hang_next = false;
        *dead = true;
        return Err(TransportError::Timeout);
    }
    state.log.push(command);
    if let Some((code, text)) = state.fail_next.take() {
        return Err(FakeSftpTransport::fail(&state, code, &text));
    }
    Ok(state)
}

impl FakeSftpTransport {
    fn fail(state: &SftpServerState, code: SftpStatus, text: &str) -> TransportError {
        match &state.echo_in_errors {
            Some(echo) => TransportError::status(code, &format!("{text} [{echo}]")),
            None => TransportError::status(code, text),
        }
    }

    fn missing(state: &SftpServerState) -> TransportError {
        Self::fail(state, SftpStatus::NoSuchFile, "No such file")
    }

    fn failure(state: &SftpServerState) -> TransportError {
        Self::fail(state, SftpStatus::Failure, "Failure")
    }

    fn new_handle(&mut self, state: &mut SftpServerState, handle: Handle) -> SftpHandle {
        let id = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(id, handle);
        state.open_handles += 1;
        SftpHandle(id)
    }
}

impl SftpTransport for FakeSftpTransport {
    fn features(&self) -> SftpFeatures {
        self.features
    }

    fn realpath(&mut self, path: &str) -> TResult<String> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let _state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("REALPATH {path}"),
        )?;
        Ok(if path == "." {
            "/home/user".into()
        } else {
            path
        })
    }

    fn lstat(&mut self, path: &str) -> TResult<RawAttrs> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("LSTAT {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(node) => Ok(attrs_of(node)),
            None => Err(Self::missing(&state)),
        }
    }

    fn open_dir(&mut self, path: &str) -> TResult<SftpHandle> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("OPENDIR {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(SNode::Dir) => {}
            Some(_) => return Err(Self::failure(&state)),
            None => return Err(Self::missing(&state)),
        }
        let mut pending: Vec<RawEntry> = Vec::new();
        if let Some(raw) = state.raw_names.clone() {
            pending = raw;
        } else {
            pending.push(RawEntry {
                name: ".".into(),
                attrs: attrs_of(&SNode::Dir),
            });
            pending.push(RawEntry {
                name: "..".into(),
                attrs: attrs_of(&SNode::Dir),
            });
            let prefix = if path == "/" {
                "/".to_owned()
            } else {
                format!("{path}/")
            };
            for (key, node) in &state.nodes {
                if key.len() > prefix.len()
                    && key.starts_with(&prefix)
                    && !key[prefix.len()..].contains('/')
                {
                    pending.push(RawEntry {
                        name: key[prefix.len()..].to_owned(),
                        attrs: attrs_of(node),
                    });
                }
            }
        }
        pending.reverse();
        let handle = Handle::Dir { pending };
        Ok(self.new_handle(&mut state, handle))
    }

    fn read_dir(&mut self, handle: SftpHandle) -> TResult<Vec<RawEntry>> {
        let server = self.server.clone();
        let state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("READDIR {}", handle.0),
        )?;
        let page = state.page_size.max(1);
        drop(state);
        match self.handles.get_mut(&handle.0) {
            Some(Handle::Dir { pending }) => {
                let mut batch = Vec::new();
                while batch.len() < page {
                    match pending.pop() {
                        Some(entry) => batch.push(entry),
                        None => break,
                    }
                }
                Ok(batch)
            }
            _ => Err(TransportError::status(
                SftpStatus::Failure,
                "Invalid handle",
            )),
        }
    }

    fn open_read(&mut self, path: &str) -> TResult<SftpHandle> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("OPEN-R {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(SNode::File { .. }) => {}
            Some(_) => return Err(Self::failure(&state)),
            None => return Err(Self::missing(&state)),
        }
        Ok(self.new_handle(&mut state, Handle::Read { path }))
    }

    fn open_write(&mut self, path: &str, how: OpenWrite) -> TResult<SftpHandle> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("OPEN-W {how:?} {path}"),
        )?;
        let parent = parent_of(&path);
        if !matches!(state.nodes.get(&parent), Some(SNode::Dir)) {
            return Err(Self::missing(&state));
        }
        let exists = state.nodes.get(&path).cloned();
        match (how, exists) {
            (OpenWrite::CreateNew, Some(_)) => return Err(Self::failure(&state)),
            (OpenWrite::CreateNew, None) | (OpenWrite::CreateTruncate, None) => {
                state.nodes.insert(
                    path.clone(),
                    SNode::File {
                        data: Vec::new(),
                        mtime: 1_700_000_100,
                        mode: 0o644,
                    },
                );
            }
            (OpenWrite::CreateTruncate, Some(SNode::File { .. })) => {
                state.nodes.insert(
                    path.clone(),
                    SNode::File {
                        data: Vec::new(),
                        mtime: 1_700_000_100,
                        mode: 0o644,
                    },
                );
            }
            (OpenWrite::Existing, Some(SNode::File { .. })) => {}
            (OpenWrite::Existing, None) => return Err(Self::missing(&state)),
            (_, Some(_)) => return Err(Self::failure(&state)),
        }
        Ok(self.new_handle(&mut state, Handle::Write { path }))
    }

    fn read(&mut self, handle: SftpHandle, offset: u64, buf: &mut [u8]) -> TResult<usize> {
        let server = self.server.clone();
        let state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("READ {} {offset}", handle.0),
        )?;
        let Some(Handle::Read { path }) = self.handles.get(&handle.0) else {
            return Err(Self::failure(&state));
        };
        match state.nodes.get(path) {
            Some(SNode::File { data, .. }) => {
                let start = (offset as usize).min(data.len());
                let n = buf.len().min(data.len() - start);
                buf[..n].copy_from_slice(&data[start..start + n]);
                Ok(n)
            }
            _ => Err(Self::missing(&state)),
        }
    }

    fn write(&mut self, handle: SftpHandle, offset: u64, data: &[u8]) -> TResult<()> {
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("WRITE {} {offset} {}", handle.0, data.len()),
        )?;
        let Some(Handle::Write { path }) = self.handles.get(&handle.0) else {
            return Err(Self::failure(&state));
        };
        let path = path.clone();
        let take = match state.cut_writes_after {
            Some(left) if left < data.len() => {
                // keep what arrived, then the connection is gone
                Some(left)
            }
            Some(left) => {
                state.cut_writes_after = Some(left - data.len());
                None
            }
            None => None,
        };
        let part = take.map_or(data, |n| &data[..n]);
        let end = offset as usize + part.len();
        match state.nodes.get_mut(&path) {
            Some(SNode::File { data: stored, .. }) => {
                if stored.len() < end {
                    stored.resize(end, 0);
                }
                stored[offset as usize..end].copy_from_slice(part);
            }
            _ => return Err(Self::missing(&state)),
        }
        state.written += part.len();
        if take.is_some() {
            state.cut_writes_after = None;
            state.generation += 1;
            self.dead = true;
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        Ok(())
    }

    fn fstat(&mut self, handle: SftpHandle) -> TResult<RawAttrs> {
        let server = self.server.clone();
        let state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("FSTAT {}", handle.0),
        )?;
        let path = match self.handles.get(&handle.0) {
            Some(Handle::Read { path } | Handle::Write { path }) => path.clone(),
            _ => return Err(Self::failure(&state)),
        };
        match state.nodes.get(&path) {
            Some(node) => Ok(attrs_of(node)),
            None => Err(Self::missing(&state)),
        }
    }

    fn close(&mut self, handle: SftpHandle) -> TResult<()> {
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("CLOSE {}", handle.0),
        )?;
        if self.handles.remove(&handle.0).is_some() {
            state.open_handles = state.open_handles.saturating_sub(1);
            Ok(())
        } else {
            Err(Self::failure(&state))
        }
    }

    fn mkdir(&mut self, path: &str) -> TResult<()> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("MKDIR {path}"),
        )?;
        if !matches!(state.nodes.get(&parent_of(&path)), Some(SNode::Dir)) {
            return Err(Self::missing(&state));
        }
        if state.nodes.contains_key(&path) {
            return Err(Self::failure(&state));
        }
        state.nodes.insert(path, SNode::Dir);
        Ok(())
    }

    fn rmdir(&mut self, path: &str) -> TResult<()> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("RMDIR {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(SNode::Dir) => {}
            Some(_) => return Err(Self::failure(&state)),
            None => return Err(Self::missing(&state)),
        }
        let prefix = format!("{path}/");
        if state.nodes.keys().any(|k| k.starts_with(&prefix)) {
            return Err(Self::failure(&state));
        }
        state.nodes.remove(&path);
        Ok(())
    }

    fn remove(&mut self, path: &str) -> TResult<()> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("REMOVE {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(SNode::File { .. } | SNode::Link { .. }) => {
                state.nodes.remove(&path);
                Ok(())
            }
            Some(SNode::Dir) => Err(Self::failure(&state)),
            None => Err(Self::missing(&state)),
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        let (from, to) = (plain(from)?.to_owned(), plain(to)?.to_owned());
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("RENAME {from} {to}"),
        )?;
        rename_nodes(&mut state, &from, &to, false)
    }

    fn posix_rename(&mut self, from: &str, to: &str) -> TResult<()> {
        let (from, to) = (plain(from)?.to_owned(), plain(to)?.to_owned());
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("POSIX-RENAME {from} {to}"),
        )?;
        if !state.features.posix_rename {
            return Err(Self::fail(&state, SftpStatus::OpUnsupported, "unsupported"));
        }
        rename_nodes(&mut state, &from, &to, true)
    }

    fn readlink(&mut self, path: &str) -> TResult<String> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("READLINK {path}"),
        )?;
        match state.nodes.get(&path) {
            Some(SNode::Link { target }) => Ok(target.clone()),
            Some(_) => Err(Self::failure(&state)),
            None => Err(Self::missing(&state)),
        }
    }

    fn symlink(&mut self, target: &str, at: &str) -> TResult<()> {
        let (target, at) = (plain(target)?.to_owned(), plain(at)?.to_owned());
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("SYMLINK {target} {at}"),
        )?;
        if !matches!(state.nodes.get(&parent_of(&at)), Some(SNode::Dir)) {
            return Err(Self::missing(&state));
        }
        if state.nodes.contains_key(&at) {
            return Err(Self::failure(&state));
        }
        state.nodes.insert(at, SNode::Link { target });
        Ok(())
    }

    fn setstat(
        &mut self,
        path: &str,
        permissions: Option<u32>,
        mtime: Option<SystemTime>,
        _atime: Option<SystemTime>,
    ) -> TResult<()> {
        let path = plain(path)?.to_owned();
        let server = self.server.clone();
        let mut state = begin(
            &server,
            self.generation,
            &mut self.dead,
            self.quit,
            format!("SETSTAT {path}"),
        )?;
        match state.nodes.get_mut(&path) {
            Some(SNode::File { mode, mtime: m, .. }) => {
                if let Some(p) = permissions {
                    *mode = p;
                }
                if let Some(t) = mtime {
                    *m = t
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                }
                Ok(())
            }
            Some(_) => Ok(()),
            None => Err(Self::missing(&state)),
        }
    }

    fn quit(&mut self) {
        self.quit = true;
    }
}

fn rename_nodes(state: &mut SftpServerState, from: &str, to: &str, replace: bool) -> TResult<()> {
    if let Some((path, data)) = state.inject_before_rename.take() {
        state.nodes.insert(
            path,
            SNode::File {
                data,
                mtime: 1_700_000_000,
                mode: 0o644,
            },
        );
    }
    if !state.nodes.contains_key(from) {
        return Err(FakeSftpTransport::missing(state));
    }
    if !matches!(state.nodes.get(&parent_of(to)), Some(SNode::Dir)) {
        return Err(FakeSftpTransport::missing(state));
    }
    if state.nodes.contains_key(to) && !(replace || state.rename_replaces) {
        return Err(FakeSftpTransport::failure(state));
    }
    // move the node and everything below it
    let prefix = format!("{from}/");
    let moved: Vec<String> = state
        .nodes
        .keys()
        .filter(|k| k.as_str() == from || k.starts_with(&prefix))
        .cloned()
        .collect();
    let mut taken = Vec::new();
    for key in moved {
        let node = state.nodes.remove(&key).expect("listed above");
        taken.push((format!("{to}{}", &key[from.len()..]), node));
    }
    for (key, node) in taken {
        state.nodes.insert(key, node);
    }
    Ok(())
}

/// Opens connections to a `FakeSftpServer`: the host key is judged first by the same policy
/// the real connection uses, then the login is checked.
pub struct FakeSftpConnector {
    pub server: FakeSftpServer,
}

impl crate::manager::SftpConnector for FakeSftpConnector {
    type Transport = FakeSftpTransport;

    fn connect(
        &self,
        params: &SftpConnectParams,
        secret: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<FakeSftpTransport> {
        let key = self.server.state().host_key.clone();
        crate::sftp_trust::check_host_key(
            verifier.as_ref(),
            &params.profile.host,
            params.profile.port,
            &key.signature_algorithm,
            &key.key_type,
            &key.blob,
        )?;
        self.server
            .connect(&params.profile.user, &params.profile.auth, secret)
    }
}
