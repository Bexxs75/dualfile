//! `FtpVfs`: an FTP/FTPS connection as a folder tree (`dualfile_core::Vfs`).
//!
//! One control connection serves listings and commands (reconnected within a fixed budget);
//! every transfer uses its own connection (taken from a small pool of idle ones or newly
//! opened), so a copy that reads and writes on the same server does not block itself.
//!
//! Safety rules kept here:
//! - no recycle bin: `trash` is `Unsupported` and is never replaced by deleting;
//! - FTP cannot refuse to overwrite: before a rename without replacing, and before the final
//!   rename of an upload, the target is looked up (`stat`); the short window between that
//!   check and `RNTO` stays and is documented, the user interface points it out;
//! - uploads go to a partial file (`<name>.dualfile-part`, see `dualfile_core::resume`) and
//!   appear under their name only after the server confirmed the whole file;
//! - names from listings are checked (`names`); marked entries are shown but never usable.

use crate::error::{TResult, TransportError};
use crate::listing::{parse_list_unix, parse_mlsd, ListedEntry};
use crate::names::{display_name, problems};
use crate::secret::Secret;
use crate::transport::{Features, FtpTransport, Limits, StoreMode, Timeouts};
use dualfile_core::resume::{
    classify_meta, decide_resume, meta_name, part_name, Ownership, PartMeta, ResumableWrite,
    ResumeDecision, SourceIdentity,
};
use dualfile_core::vfs::{
    Caps, Entry, EntryKind, FileStat, ReplaceMode, Scheme, VPath, Vfs, WriteFinish, WriteMode,
};
use dualfile_core::{Error, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Creates a new logged-in control connection (used for reconnecting and for transfers).
pub type Opener<T> = Arc<dyn Fn() -> TResult<T> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct FtpConfig {
    pub limits: Limits,
    pub timeouts: Timeouts,
    /// Transfer connections open at the same time.
    pub max_transfers: usize,
    /// How often the control connection may be re-established after it was lost.
    pub max_reconnects: u32,
    /// How long a listing is reused (0 = never). Only read paths use it; checks that decide
    /// about overwriting always ask the server.
    pub cache_ttl: Duration,
    /// Idle transfer connections kept for reuse.
    pub max_idle: usize,
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            timeouts: Timeouts::default(),
            max_transfers: 4,
            max_reconnects: 5,
            cache_ttl: Duration::from_secs(1),
            max_idle: 2,
        }
    }
}

struct Control<T> {
    transport: Option<T>,
    features: Features,
    /// Connections made so far (the first one included).
    connects: u32,
}

struct Cached {
    at: Instant,
    entries: Arc<Vec<Entry>>,
}

struct Shared<T: FtpTransport> {
    id: String,
    opener: Opener<T>,
    cfg: FtpConfig,
    secret: Option<Secret>,
    control: Mutex<Control<T>>,
    closed: std::sync::atomic::AtomicBool,
    open_transfers: AtomicUsize,
    idle: Mutex<Vec<T>>,
    cache: Mutex<HashMap<String, Cached>>,
    seq: AtomicUsize,
}

pub struct FtpVfs<T: FtpTransport> {
    shared: Arc<Shared<T>>,
}

impl<T: FtpTransport> std::fmt::Debug for FtpVfs<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpVfs")
            .field("id", &self.shared.id)
            .finish()
    }
}

impl<T: FtpTransport> Shared<T> {
    fn scrub(&self, error: TransportError) -> TransportError {
        match &self.secret {
            Some(secret) => error.scrubbed(secret),
            None => error,
        }
    }

    fn core(&self, error: TransportError, path: &VPath) -> Error {
        self.scrub(error).into_core(path.path())
    }

    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(Error::Disconnected)
        } else {
            Ok(())
        }
    }

    fn invalidate(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Runs `op` on the control connection. A lost connection is replaced within the
    /// reconnect budget; `retry` says whether `op` may be run again on the new connection
    /// (reads only: a command that may have been executed is never repeated blindly).
    /// `Err(Left(..))` is a protocol answer for the caller to map onto its path.
    fn run_control<R>(
        &self,
        retry: bool,
        mut op: impl FnMut(&mut T, &Features) -> TResult<R>,
    ) -> std::result::Result<R, ControlError> {
        self.check_open().map_err(ControlError::Core)?;
        let mut control = self.control.lock().unwrap();
        let mut attempts = 0;
        loop {
            attempts += 1;
            if control.transport.is_none() {
                if control.connects > self.cfg.max_reconnects {
                    return Err(ControlError::Core(Error::Disconnected));
                }
                control.connects += 1;
                match (self.opener)() {
                    Ok(transport) => {
                        control.features = transport.features();
                        control.transport = Some(transport);
                    }
                    Err(error) => {
                        return Err(ControlError::Core(
                            self.scrub(error).into_core(std::path::Path::new("/")),
                        ))
                    }
                }
            }
            let features = control.features;
            let transport = control.transport.as_mut().expect("connected above");
            match op(transport, &features) {
                Ok(value) => return Ok(value),
                Err(error) if error.is_connection_lost() => {
                    control.transport = None;
                    if retry && attempts < 2 {
                        continue;
                    }
                    return Err(ControlError::Core(
                        self.scrub(error).into_core(std::path::Path::new("/")),
                    ));
                }
                Err(error) => return Err(ControlError::Transport(error)),
            }
        }
    }

    /// `run_control`, with protocol answers mapped onto `path` (not found, exists, ...).
    fn with_control_path<R>(
        &self,
        path: &VPath,
        retry: bool,
        op: impl FnMut(&mut T, &Features) -> TResult<R>,
    ) -> Result<R> {
        match self.run_control(retry, op) {
            Ok(value) => Ok(value),
            Err(ControlError::Core(error)) => Err(error),
            Err(ControlError::Transport(error)) => Err(self.core(error, path)),
        }
    }

    fn take_transfer_slot(self: &Arc<Self>) -> Result<TransferSlot<T>> {
        let max = self.cfg.max_transfers.max(1);
        let mut current = self.open_transfers.load(Ordering::SeqCst);
        loop {
            if current >= max {
                return Err(Error::Remote(
                    "Zu viele gleichzeitige Übertragungen auf dieser Verbindung".into(),
                ));
            }
            match self.open_transfers.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(now) => current = now,
            }
        }
        Ok(TransferSlot {
            shared: self.clone(),
        })
    }

    /// A connection for one transfer: an idle one that still answers, or a new one.
    fn transfer_connection(self: &Arc<Self>) -> Result<(T, TransferSlot<T>)> {
        self.check_open()?;
        let slot = self.take_transfer_slot()?;
        loop {
            let pooled = self.idle.lock().unwrap().pop();
            match pooled {
                Some(mut transport) => {
                    if transport.noop().is_ok() {
                        return Ok((transport, slot));
                    }
                }
                None => break,
            }
        }
        match (self.opener)() {
            Ok(transport) => Ok((transport, slot)),
            Err(error) => Err(self.scrub(error).into_core(std::path::Path::new("/"))),
        }
    }

    fn give_back(&self, mut transport: T) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        if transport.noop().is_ok() {
            let mut idle = self.idle.lock().unwrap();
            if idle.len() < self.cfg.max_idle {
                idle.push(transport);
            }
        }
    }
}

enum ControlError {
    /// Already a core error (disconnected, connection lost, closed).
    Core(Error),
    /// The server refused the command.
    Transport(TransportError),
}

struct TransferSlot<T: FtpTransport> {
    shared: Arc<Shared<T>>,
}

impl<T: FtpTransport> Drop for TransferSlot<T> {
    fn drop(&mut self) {
        self.shared.open_transfers.fetch_sub(1, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------------------ helpers

pub(crate) fn wire(path: &VPath) -> Result<String> {
    let text = path
        .path()
        .to_str()
        .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
    if text.chars().any(char::is_control) {
        return Err(Error::InvalidPath("Ungültiger Pfad auf dem Server".into()));
    }
    Ok(text.to_owned())
}

fn wire_str(text: &str) -> TResult<&str> {
    if text.chars().any(char::is_control) {
        Err(TransportError::Rejected)
    } else {
        Ok(text)
    }
}

pub(crate) fn join_wire(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn entry_from(listed: &ListedEntry, problem: Option<&'static str>) -> Entry {
    let hidden = listed.name.starts_with('.');
    Entry {
        uid: listed.uid,
        gid: listed.gid,
        name: display_name(&listed.name),
        kind: listed.kind,
        size: listed.size,
        modified: listed.modified,
        mode: listed.mode,
        hidden,
        link_target: listed.link_target.clone(),
        broken_link: false,
        error: problem.map(str::to_owned),
    }
}

/// Lists `dir` on `transport` (no cache): `MLSD` if offered (falling back to `LIST` when the
/// server rejects it), parsed within the limits. Too many entries is an error, never a
/// silently shortened listing.
fn list_on<T: FtpTransport>(
    transport: &mut T,
    features: &Features,
    dir: &str,
    limits: &Limits,
) -> TResult<Vec<(ListedEntry, Option<&'static str>)>> {
    wire_str(dir)?;
    let mut use_mlsd = features.mlsd;
    let lines = loop {
        match transport.list(dir, use_mlsd, limits) {
            Ok(lines) => break lines,
            Err(TransportError::Reply {
                code: 500 | 501 | 502 | 504,
                ..
            }) if use_mlsd => {
                use_mlsd = false;
            }
            Err(error) => return Err(error),
        }
    };
    let parsed = if use_mlsd {
        parse_mlsd(&lines, limits)
    } else {
        parse_list_unix(&lines, limits, SystemTime::now())
    };
    if parsed.truncated {
        return Err(TransportError::TooLarge(
            "Das Verzeichnis hat zu viele Einträge",
        ));
    }
    let marks = problems(&parsed.entries);
    Ok(parsed.entries.into_iter().zip(marks).collect())
}

/// Looks up one path (`None` = not there), uncached.
fn lookup_on<T: FtpTransport>(
    transport: &mut T,
    features: &Features,
    path: &str,
    limits: &Limits,
) -> TResult<Option<ListedEntry>> {
    if path == "/" {
        return Ok(Some(ListedEntry {
            uid: None,
            gid: None,
            name: "/".into(),
            kind: EntryKind::Dir,
            size: 0,
            modified: None,
            mode: None,
            link_target: None,
        }));
    }
    let (parent, name) = path.rsplit_once('/').expect("absolute path");
    let parent = if parent.is_empty() { "/" } else { parent };
    if features.mlst {
        match transport.mlst(path) {
            Ok(line) => {
                let facts = line.trim_start().split(' ').next().unwrap_or("");
                let parsed = parse_mlsd(&[format!("{facts} x")], limits);
                if let Some(mut entry) = parsed.entries.into_iter().next() {
                    entry.name = name.to_owned();
                    return Ok(Some(entry));
                }
            }
            Err(TransportError::Reply { code: 550, .. }) => return Ok(None),
            Err(TransportError::Reply {
                code: 500..=504, ..
            }) => {}
            Err(error) => return Err(error),
        }
    }
    match list_on(transport, features, parent, limits) {
        Ok(entries) => Ok(entries
            .into_iter()
            .find(|(entry, _)| entry.name == name)
            .map(|(entry, _)| entry)),
        Err(TransportError::Reply { code: 550, .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn download_small<T: FtpTransport>(transport: &mut T, path: &str, max: usize) -> TResult<Vec<u8>> {
    transport.start_download(path, 0)?;
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match transport.read_data(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.len() > max {
                    transport.abort_transfer();
                    return Err(TransportError::TooLarge("Hilfsdatei-Vermerk"));
                }
            }
            Err(error) => {
                transport.abort_transfer();
                return Err(error);
            }
        }
    }
    transport.finish_download()?;
    Ok(data)
}

fn upload_small<T: FtpTransport>(transport: &mut T, path: &str, data: &[u8]) -> TResult<()> {
    transport.start_upload(path, StoreMode::Create)?;
    transport.write_data(data)?;
    transport.finish_upload()
}

// ------------------------------------------------------------------------------ FtpVfs

impl<T: FtpTransport> FtpVfs<T> {
    /// `first` is the connection the login was checked on. `secret` (if given) is only used
    /// to scrub error texts; it is not kept anywhere else. `opener` makes new connections.
    pub fn new(
        id: &str,
        first: T,
        opener: Opener<T>,
        cfg: FtpConfig,
        secret: Option<Secret>,
    ) -> Result<Arc<Self>> {
        if !dualfile_core::vfs::is_valid_connection_id(id) {
            return Err(Error::InvalidInput("Ungültige Verbindungskennung"));
        }
        let features = first.features();
        Ok(Arc::new(Self {
            shared: Arc::new(Shared {
                id: id.to_owned(),
                opener,
                cfg,
                secret,
                control: Mutex::new(Control {
                    transport: Some(first),
                    features,
                    connects: 1,
                }),
                closed: std::sync::atomic::AtomicBool::new(false),
                open_transfers: AtomicUsize::new(0),
                idle: Mutex::new(Vec::new()),
                cache: Mutex::new(HashMap::new()),
                seq: AtomicUsize::new(0),
            }),
        }))
    }

    pub fn id(&self) -> &str {
        &self.shared.id
    }

    /// Closes the connection: later calls give `Error::Disconnected`; connections are dropped.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        if let Some(mut transport) = self.shared.control.lock().unwrap().transport.take() {
            transport.quit();
        }
        for mut transport in self.shared.idle.lock().unwrap().drain(..) {
            transport.quit();
        }
        self.shared.invalidate();
    }

    /// Whether the last transport operation left a usable control connection.
    pub fn is_live(&self) -> bool {
        !self.is_closed() && self.shared.control.lock().unwrap().transport.is_some()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Number of control connections made so far (for the reconnect budget).
    pub fn connects(&self) -> u32 {
        self.shared.control.lock().unwrap().connects
    }

    fn own(&self, path: &VPath) -> Result<()> {
        if path.scheme() == Scheme::Ftp && path.conn_id() == Some(self.shared.id.as_str()) {
            Ok(())
        } else {
            Err(Error::Disconnected)
        }
    }

    fn cached_listing(&self, dir: &VPath, fresh: bool) -> Result<Arc<Vec<Entry>>> {
        self.own(dir)?;
        let key = wire(dir)?;
        let ttl = self.shared.cfg.cache_ttl;
        if !fresh && !ttl.is_zero() {
            if let Some(cached) = self.shared.cache.lock().unwrap().get(&key) {
                if cached.at.elapsed() < ttl {
                    return Ok(cached.entries.clone());
                }
            }
        }
        let limits = self.shared.cfg.limits;
        let listed = self
            .shared
            .with_control_path(dir, true, |transport, features| {
                list_on(transport, features, &key, &limits)
            })?;
        let entries: Arc<Vec<Entry>> = Arc::new(
            listed
                .iter()
                .map(|(entry, problem)| entry_from(entry, *problem))
                .collect(),
        );
        // the raw names are needed for lookups; keep marked names out of the cache lookup
        self.shared.cache.lock().unwrap().insert(
            key,
            Cached {
                at: Instant::now(),
                entries: entries.clone(),
            },
        );
        Ok(entries)
    }

    fn lookup(&self, path: &VPath, fresh: bool) -> Result<Option<Entry>> {
        self.own(path)?;
        let wire_path = wire(path)?;
        if wire_path == "/" {
            return Ok(Some(Entry {
                uid: None,
                gid: None,
                name: "/".into(),
                kind: EntryKind::Dir,
                size: 0,
                modified: None,
                mode: None,
                hidden: false,
                link_target: None,
                broken_link: false,
                error: None,
            }));
        }
        let name = path
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?
            .to_owned();
        let parent = path
            .parent()
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
        if !fresh {
            // a cached parent listing answers without a command
            let key = wire(&parent)?;
            let ttl = self.shared.cfg.cache_ttl;
            if !ttl.is_zero() {
                if let Some(cached) = self.shared.cache.lock().unwrap().get(&key) {
                    if cached.at.elapsed() < ttl {
                        return Ok(cached.entries.iter().find(|e| e.name == name).cloned());
                    }
                }
            }
        }
        let limits = self.shared.cfg.limits;
        let found = self
            .shared
            .with_control_path(path, true, |transport, features| {
                lookup_on(transport, features, &wire_path, &limits)
            })?;
        Ok(found.map(|listed| {
            let marks = problems(std::slice::from_ref(&listed));
            entry_from(&listed, marks[0])
        }))
    }

    fn require(&self, path: &VPath, fresh: bool) -> Result<Entry> {
        self.lookup(path, fresh)?
            .ok_or_else(|| Error::NotFound(path.path().to_path_buf()))
    }

    fn usable_file(&self, path: &VPath) -> Result<Entry> {
        let entry = self.require(path, false)?;
        if let Some(problem) = &entry.error {
            return Err(Error::InvalidName(problem.clone()));
        }
        match entry.kind {
            EntryKind::File => Ok(entry),
            EntryKind::Dir => Err(Error::IsADirectory(path.path().to_path_buf())),
            // a link on the server is shown, never followed
            EntryKind::Symlink => Err(Error::Unsupported("Verknüpfungen auf dem Server folgen")),
        }
    }

    fn start_download(&self, path: &VPath, offset: u64) -> Result<Box<dyn Read + Send>> {
        self.usable_file(path)?;
        let wire_path = wire(path)?;
        let (mut transport, slot) = self.shared.transfer_connection()?;
        let features = transport.features();
        // without REST support the first `offset` bytes are read and dropped
        let (rest, skip) = if offset > 0 && !features.rest_stream {
            (0, offset)
        } else {
            (offset, 0)
        };
        if let Err(error) = transport.start_download(&wire_path, rest) {
            return Err(self.shared.core(error, path));
        }
        let mut reader = FtpReader {
            shared: self.shared.clone(),
            transport: Some(transport),
            _slot: slot,
            finished: false,
        };
        if skip > 0 {
            std::io::copy(&mut (&mut reader).take(skip), &mut std::io::sink())
                .map_err(|e| Error::Remote(self.shared.scrub_text(&e.to_string())))?;
        }
        Ok(Box::new(reader))
    }

    fn unique_part_name(&self, name: &str) -> Result<String> {
        let n = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let candidate = format!("{name}.dualfile-tmp-{n}");
        if candidate.len() <= 255 {
            Ok(candidate)
        } else {
            Ok(format!(".dualfile-tmp-{n}"))
        }
    }
}

impl<T: FtpTransport> Shared<T> {
    fn scrub_text(&self, text: &str) -> String {
        match &self.secret {
            Some(secret) => secret.scrub(text),
            None => text.to_owned(),
        }
    }
}

// ------------------------------------------------------------------------------ reading

struct FtpReader<T: FtpTransport> {
    shared: Arc<Shared<T>>,
    transport: Option<T>,
    _slot: TransferSlot<T>,
    finished: bool,
}

impl<T: FtpTransport> Read for FtpReader<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.finished {
            return Ok(0);
        }
        let Some(transport) = self.transport.as_mut() else {
            return Ok(0);
        };
        let io_error = |shared: &Shared<T>, error: TransportError| {
            let kind = if error.is_connection_lost() {
                std::io::ErrorKind::ConnectionAborted
            } else {
                std::io::ErrorKind::Other
            };
            std::io::Error::new(kind, shared.scrub(error).to_string())
        };
        match transport.read_data(buf) {
            Ok(0) => {
                self.finished = true;
                let mut transport = self.transport.take().expect("present");
                match transport.finish_download() {
                    Ok(()) => {
                        self.shared.give_back(transport);
                        Ok(0)
                    }
                    Err(error) => Err(io_error(&self.shared, error)),
                }
            }
            Ok(n) => Ok(n),
            Err(error) => {
                self.finished = true;
                self.transport = None;
                Err(io_error(&self.shared, error))
            }
        }
    }
}

impl<T: FtpTransport> Drop for FtpReader<T> {
    fn drop(&mut self) {
        if let Some(mut transport) = self.transport.take() {
            // abandoned before the end: stop the transfer, keep the connection if it
            // still answers
            transport.abort_transfer();
            self.shared.give_back(transport);
        }
    }
}

// ------------------------------------------------------------------------------ writing

/// What happens to the partial file when the write is dropped without `commit`.
#[derive(Clone)]
enum PartKind {
    /// A throw-away name; removed again.
    Temporary,
    /// A resumable partial file with a meta file; both stay.
    Resumable {
        identity: SourceIdentity,
        meta_path: String,
    },
}

struct FtpWrite<T: FtpTransport> {
    shared: Arc<Shared<T>>,
    transport: Option<T>,
    _slot: TransferSlot<T>,
    part_path: String,
    target: VPath,
    target_wire: String,
    mode: WriteMode,
    kind: PartKind,
    /// Bytes in the partial file before this write started.
    existing: u64,
    written: u64,
    started: bool,
    store_mode: StoreMode,
    done: bool,
    broken: bool,
}

impl<T: FtpTransport> FtpWrite<T> {
    fn ensure_started(&mut self) -> std::io::Result<()> {
        if self.started {
            return Ok(());
        }
        let transport = self.transport.as_mut().expect("present until commit");
        transport
            .start_upload(&self.part_path, self.store_mode)
            .map_err(|e| self.io_error(e))?;
        self.started = true;
        Ok(())
    }

    fn io_error(&self, error: TransportError) -> std::io::Error {
        std::io::Error::new(
            if error.is_connection_lost() {
                std::io::ErrorKind::ConnectionAborted
            } else {
                std::io::ErrorKind::Other
            },
            self.shared.scrub(error).to_string(),
        )
    }

    fn core_error(&self, error: TransportError) -> Error {
        self.shared.core(error, &self.target)
    }

    fn commit_inner(&mut self) -> Result<()> {
        let mut transport = self.transport.take().expect("present until commit");
        let result = self.commit_on(&mut transport);
        match result {
            Ok(()) => {
                self.shared.invalidate();
                self.done = true;
                self.shared.give_back(transport);
                Ok(())
            }
            Err(error) => {
                // put it back so Drop can still tidy up / keep the partial file
                self.transport = Some(transport);
                Err(error)
            }
        }
    }

    fn commit_on(&mut self, transport: &mut T) -> Result<()> {
        if !self.started && self.store_mode == StoreMode::Create {
            // nothing was written (an empty file): create it
            transport
                .start_upload(&self.part_path, StoreMode::Create)
                .map_err(|e| self.core_error(e))?;
            self.started = true;
        }
        if self.started {
            transport.finish_upload().map_err(|e| {
                self.broken = true;
                self.core_error(e)
            })?;
            self.started = false;
        }
        let expected = self.existing + self.written;
        match transport.size(&self.part_path) {
            Ok(size) if size == expected => {}
            Ok(_) => return Err(Error::SizeMismatch(self.target.path().to_path_buf())),
            Err(error) => return Err(self.core_error(error)),
        }
        let features = transport.features();
        let limits = self.shared.cfg.limits;
        if self.mode == WriteMode::NoReplace {
            // FTP has no "rename unless exists": look first. The window between this look
            // and `RNTO` remains.
            match lookup_on(transport, &features, &self.target_wire, &limits) {
                Ok(None) => {}
                Ok(Some(_)) => return Err(Error::Exists(self.target.path().to_path_buf())),
                Err(error) => return Err(self.core_error(error)),
            }
        }
        transport
            .rename(&self.part_path, &self.target_wire)
            .map_err(|e| self.core_error(e))?;
        if let PartKind::Resumable { meta_path, .. } = &self.kind {
            let _ = transport.delete(meta_path);
        }
        Ok(())
    }
}

impl<T: FtpTransport> Write for FtpWrite<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.done || self.transport.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Der Schreibvorgang wurde bereits beendet",
            ));
        }
        self.ensure_started()?;
        let transport = self.transport.as_mut().expect("checked");
        match transport.write_data(buf) {
            Ok(()) => {
                self.written += buf.len() as u64;
                Ok(buf.len())
            }
            Err(error) => {
                self.broken = true;
                Err(self.io_error(error))
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T: FtpTransport> WriteFinish for FtpWrite<T> {
    fn commit(&mut self) -> Result<()> {
        if self.done {
            return Ok(());
        }
        if self.transport.is_none() {
            return Err(Error::Remote(
                "Der Schreibvorgang wurde bereits beendet".into(),
            ));
        }
        self.commit_inner()
    }
}

impl<T: FtpTransport> Drop for FtpWrite<T> {
    fn drop(&mut self) {
        let Some(mut transport) = self.transport.take() else {
            return;
        };
        if self.done {
            return;
        }
        // an upload that was not committed: close the data connection first (the server
        // then holds exactly the bytes that arrived)
        let mut usable = !self.broken;
        if usable && self.started {
            usable = transport.finish_upload().is_ok();
        }
        match &self.kind {
            PartKind::Temporary => {
                if usable {
                    let _ = transport.delete(&self.part_path);
                } else {
                    // the connection may be gone; try on a fresh one
                    if let Ok(mut fresh) = (self.shared.opener)() {
                        let _ = fresh.delete(&self.part_path);
                    }
                }
            }
            PartKind::Resumable {
                identity,
                meta_path,
            } => {
                // record how far the partial file really is, so a later copy can continue
                if usable {
                    if let Ok(size) = transport.size(&self.part_path) {
                        let meta = PartMeta::new(identity.clone(), size);
                        let _ = upload_small(&mut transport, meta_path, &meta.to_bytes());
                    }
                } else if let Ok(mut fresh) = (self.shared.opener)() {
                    // the transfer connection broke: a fresh one records what arrived
                    if let Ok(size) = fresh.size(&self.part_path) {
                        let meta = PartMeta::new(identity.clone(), size);
                        let _ = upload_small(&mut fresh, meta_path, &meta.to_bytes());
                    }
                }
            }
        }
        self.shared.invalidate();
    }
}

// ------------------------------------------------------------------------------ Vfs

impl<T: FtpTransport> Vfs for FtpVfs<T> {
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        let entries = self.cached_listing(p, false)?;
        Ok(entries.as_ref().clone())
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        self.require(p, false)
    }

    fn stat_file(&self, p: &VPath) -> Result<FileStat> {
        let entry = self.usable_file(p)?;
        Ok(FileStat {
            size: entry.size,
            modified: entry.modified,
        })
    }

    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        self.own(p)?;
        self.start_download(p, 0)
    }

    fn open_read_from(&self, p: &VPath, offset: u64) -> Result<Box<dyn Read + Send>> {
        self.own(p)?;
        self.start_download(p, offset)
    }

    fn read_range(&self, p: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.own(p)?;
        let entry = self.usable_file(p)?;
        if offset >= entry.size || len == 0 {
            return Ok(Vec::new());
        }
        let reader = self.start_download(p, offset)?;
        let mut data = Vec::with_capacity(len.min(1 << 20));
        reader
            .take(len as u64)
            .read_to_end(&mut data)
            .map_err(|e| Error::Remote(self.shared.scrub_text(&e.to_string())))?;
        Ok(data)
    }

    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        self.own(p)?;
        let target_wire = wire(p)?;
        let name = p
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
        let parent = p
            .parent()
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
        let parent_wire = wire(&parent)?;
        let (mut transport, slot) = self.shared.transfer_connection()?;
        let features = transport.features();
        let limits = self.shared.cfg.limits;
        // a name nobody uses yet: an existing file is never touched
        let mut part_path = None;
        for _ in 0..20 {
            let candidate = join_wire(&parent_wire, &self.unique_part_name(name)?);
            match lookup_on(&mut transport, &features, &candidate, &limits) {
                Ok(None) => {
                    part_path = Some(candidate);
                    break;
                }
                Ok(Some(_)) => continue,
                Err(error) => return Err(self.shared.core(error, p)),
            }
        }
        let part_path =
            part_path.ok_or_else(|| Error::Remote("Kein freier Name für die Hilfsdatei".into()))?;
        Ok(Box::new(FtpWrite {
            shared: self.shared.clone(),
            transport: Some(transport),
            _slot: slot,
            part_path,
            target: p.clone(),
            target_wire,
            mode,
            kind: PartKind::Temporary,
            existing: 0,
            written: 0,
            started: false,
            store_mode: StoreMode::Create,
            done: false,
            broken: false,
        }))
    }

    fn create_write_resumable(
        &self,
        p: &VPath,
        expected_source: SourceIdentity,
    ) -> Result<ResumableWrite> {
        self.own(p)?;
        let target_wire = wire(p)?;
        let name = p
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?
            .to_owned();
        let parent = p
            .parent()
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
        let parent_wire = wire(&parent)?;
        let (mut transport, slot) = self.shared.transfer_connection()?;
        let features = transport.features();
        let limits = self.shared.cfg.limits;
        let listing = list_on(&mut transport, &features, &parent_wire, &limits)
            .map_err(|e| self.shared.core(e, p))?;
        let find = |wanted: &str| {
            listing
                .iter()
                .find(|(entry, _)| entry.name == wanted)
                .map(|(entry, _)| entry.clone())
        };
        for attempt in 0..20 {
            let part = part_name(&name, attempt)?;
            let meta = meta_name(&part);
            let part_entry = find(&part);
            let meta_entry = find(&meta);
            let part_path = join_wire(&parent_wire, &part);
            let meta_path = join_wire(&parent_wire, &meta);
            let meta_bytes = match &meta_entry {
                Some(entry) if entry.kind == EntryKind::File && entry.size <= 4096 => {
                    match download_small(&mut transport, &meta_path, 4096) {
                        Ok(bytes) => Some(bytes),
                        Err(error) => return Err(self.shared.core(error, p)),
                    }
                }
                // a meta name that is a folder, a link or huge is not ours
                Some(_) => {
                    continue;
                }
                None => None,
            };
            let ownership = classify_meta(meta_bytes.as_deref());
            let (existing, matches) = match (&part_entry, ownership) {
                // nothing there: take this name
                (None, Ownership::Foreign) if meta_entry.is_none() => (0, false),
                // a leftover meta of ours without partial file: start over
                (None, Ownership::Ours(_) | Ownership::Damaged) => (0, false),
                // somebody else's file(s): never touched, try the next name
                (None, Ownership::Foreign) | (Some(_), Ownership::Foreign) => continue,
                (Some(entry), _) if entry.kind != EntryKind::File => continue,
                (Some(_), Ownership::Damaged) => (0, false),
                (Some(entry), Ownership::Ours(found)) => {
                    match decide_resume(&found, &expected_source, Some(entry.size)) {
                        ResumeDecision::Resume { len } => (len, true),
                        ResumeDecision::Fresh(_) => (0, false),
                    }
                }
            };
            // record the identity first, then continue or start the partial file
            let meta_now = PartMeta::new(expected_source.clone(), existing);
            upload_small(&mut transport, &meta_path, &meta_now.to_bytes())
                .map_err(|e| self.shared.core(e, p))?;
            self.shared.invalidate();
            let store_mode = if matches {
                StoreMode::Append
            } else {
                StoreMode::Create
            };
            return Ok(ResumableWrite {
                existing_len: existing,
                identity_matches: matches,
                writer: Box::new(FtpWrite {
                    shared: self.shared.clone(),
                    transport: Some(transport),
                    _slot: slot,
                    part_path,
                    target: p.clone(),
                    target_wire,
                    mode: WriteMode::NoReplace,
                    kind: PartKind::Resumable {
                        identity: expected_source,
                        meta_path,
                    },
                    existing,
                    written: 0,
                    started: false,
                    store_mode,
                    done: false,
                    broken: false,
                }),
            });
        }
        Err(Error::Remote("Kein freier Name für die Hilfsdatei".into()))
    }

    fn discard_resumable(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let name = p
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?
            .to_owned();
        let parent = p
            .parent()
            .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))?;
        let parent_wire = wire(&parent)?;
        let limits = self.shared.cfg.limits;
        let result = self
            .shared
            .with_control_path(p, false, |transport, features| {
                let listing = list_on(transport, features, &parent_wire, &limits)?;
                for attempt in 0..20 {
                    let Ok(part) = part_name(&name, attempt) else {
                        break;
                    };
                    let meta = meta_name(&part);
                    let Some((meta_entry, _)) = listing.iter().find(|(e, _)| e.name == meta) else {
                        continue;
                    };
                    if meta_entry.kind != EntryKind::File || meta_entry.size > 4096 {
                        continue;
                    }
                    let bytes = download_small(transport, &join_wire(&parent_wire, &meta), 4096)?;
                    // only what the meta file proves to be ours is removed
                    if matches!(
                        classify_meta(Some(&bytes)),
                        Ownership::Ours(_) | Ownership::Damaged
                    ) {
                        if listing
                            .iter()
                            .any(|(e, _)| e.name == part && e.kind == EntryKind::File)
                        {
                            transport.delete(&join_wire(&parent_wire, &part))?;
                        }
                        transport.delete(&join_wire(&parent_wire, &meta))?;
                    }
                }
                Ok(())
            });
        self.shared.invalidate();
        result
    }

    fn mkdir(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let path = wire(p)?;
        let outcome = self
            .shared
            .with_control_path(p, false, |transport, _| transport.mkdir(&path));
        self.shared.invalidate();
        match outcome {
            Err(Error::NotFound(_)) | Err(Error::Exists(_)) => {
                // MKD answers 550 for "exists" and for "no such parent"; ask which one
                match self.lookup(p, true)? {
                    Some(_) => Err(Error::Exists(p.path().to_path_buf())),
                    None => Err(Error::NotFound(p.path().to_path_buf())),
                }
            }
            other => other,
        }
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        self.own(from)?;
        self.own(to)?;
        let (a, b) = (wire(from)?, wire(to)?);
        if mode == ReplaceMode::NoReplace {
            // no atomic "rename unless exists" in FTP: look first (the remaining window is
            // between this look and RNTO)
            if self.lookup(to, true)?.is_some() {
                return Err(Error::Exists(to.path().to_path_buf()));
            }
        }
        let outcome = self
            .shared
            .with_control_path(from, false, |transport, _| transport.rename(&a, &b));
        self.shared.invalidate();
        outcome
    }

    fn read_link(&self, p: &VPath) -> Result<String> {
        let entry = self.require(p, false)?;
        entry
            .link_target
            .ok_or(Error::Unsupported("Linkziel lesen"))
    }

    fn create_symlink(&self, _target: &str, _at: &VPath) -> Result<()> {
        Err(Error::Unsupported("Verknüpfungen auf Servern anlegen"))
    }

    fn trash(&self, _p: &VPath) -> Result<()> {
        // never replaced by a permanent delete
        Err(Error::Unsupported("Papierkorb auf Servern"))
    }

    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let path = wire(p)?;
        let entry = self.require(p, true)?;
        let outcome = self.shared.with_control_path(p, false, |transport, _| {
            if entry.kind == EntryKind::Dir {
                transport.rmdir(&path)
            } else {
                transport.delete(&path)
            }
        });
        self.shared.invalidate();
        match outcome {
            Err(error) if entry.kind == EntryKind::Dir => {
                // RMD answers 550 for several reasons; a folder with content is the one
                // the caller must tell apart
                if self.shared.cfg.limits.max_entries > 0 {
                    if let Ok(children) = self.cached_listing(p, true) {
                        if !children.is_empty() {
                            return Err(Error::DirectoryNotEmpty(p.path().to_path_buf()));
                        }
                    }
                }
                Err(error)
            }
            other => other,
        }
    }

    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: false,
            can_rename_in_place: true,
        }
    }

    fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
        Some((Scheme::Ftp, &self.shared.id))
    }
}
