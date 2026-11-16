//! `SftpVfs`: an SFTP connection as a folder tree (`dualfile_core::Vfs`).
//!
//! One SSH connection with the `sftp` subsystem serves everything (SFTP multiplexes files over
//! one connection; requests are serialized by one lock and reconnected within a fixed budget).
//! A transfer is a file handle on that connection. If the connection has to be replaced, the
//! handles of the old one are gone: running transfers fail instead of continuing on a stranger.
//!
//! Safety rules kept here:
//! - no recycle bin: `trash` is `Unsupported` and is never replaced by deleting;
//! - links are shown as links (`lstat`), their targets are only displayed, never followed;
//! - writes go to a throw-away name (`create_write`) or a resumable partial file
//!   (`create_write_resumable`, see `dualfile_core::resume`), created with "must not exist" so
//!   nothing existing is touched, and appear under their name only after the size was
//!   confirmed;
//! - "rename without replacing": the target is looked up first, then the plain SFTP `rename`
//!   is used, which OpenSSH refuses for an existing target (atomic there). Other servers may
//!   replace silently: the window between the look and the rename stays and is documented
//!   (the user interface points it out). Replacing uses `posix-rename@openssh.com`; without
//!   it replacing is refused (`Unsupported`), the original is never deleted first;
//! - names from listings are checked (`names`); marked entries are shown but never usable.

use crate::error::{SftpStatus, TResult, TransportError};
use crate::ftpvfs::{entry_from, join_wire, wire, Opener};
use crate::listing::ListedEntry;
use crate::names::{name_problem, problems};
use crate::secret::Secret;
use crate::sftp_transport::{
    OpenWrite, RawAttrs, RawEntry, RawKind, SftpFeatures, SftpHandle, SftpTransport,
};
use crate::transport::{Limits, Timeouts};
use dualfile_core::resume::{
    classify_meta, decide_resume, meta_name, part_name, Ownership, PartMeta, ResumableWrite,
    ResumeDecision, SourceIdentity,
};
use dualfile_core::vfs::{
    Caps, Entry, EntryKind, FileStat, ReplaceMode, Scheme, VPath, Vfs, WriteFinish, WriteMode,
};
use dualfile_core::{Error, Result};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Longest server path accepted, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// Largest piece read per request.
const READ_CHUNK: usize = 64 * 1024;
/// Largest piece written per request.
const WRITE_CHUNK: usize = 32 * 1024;
/// Longest link target kept for display.
const MAX_LINK_TARGET: usize = 4096;

#[derive(Clone, Debug)]
pub struct SftpConfig {
    pub limits: Limits,
    pub timeouts: Timeouts,
    /// Files open for transfer at the same time.
    pub max_transfers: usize,
    /// How often the connection may be re-established after it was lost.
    pub max_reconnects: u32,
    /// A listing that takes longer than this ends with `Timeout`.
    pub list_deadline: Duration,
    /// Link targets read per listing (more links are shown without a target).
    pub max_link_reads: usize,
}

impl Default for SftpConfig {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            timeouts: Timeouts::default(),
            max_transfers: 4,
            max_reconnects: 5,
            list_deadline: Duration::from_secs(120),
            max_link_reads: 500,
        }
    }
}

struct Control<T> {
    transport: Option<T>,
    features: SftpFeatures,
    /// Connections made so far (the first one included).
    connects: u32,
    /// Counts the connections; a handle is only valid on the one it was opened on.
    generation: u64,
}

struct Shared<T: SftpTransport> {
    scheme: Scheme,
    id: String,
    opener: Opener<T>,
    cfg: SftpConfig,
    secret: Option<Secret>,
    control: Mutex<Control<T>>,
    closed: AtomicBool,
    open_transfers: AtomicUsize,
    seq: AtomicU64,
}

pub struct SftpVfs<T: SftpTransport> {
    shared: Arc<Shared<T>>,
}

impl<T: SftpTransport> std::fmt::Debug for SftpVfs<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpVfs")
            .field("id", &self.shared.id)
            .finish()
    }
}

enum ControlError {
    /// Already a core error (disconnected, connection lost, closed).
    Core(Error),
    /// The server refused the request.
    Transport(TransportError),
}

struct TransferSlot<T: SftpTransport> {
    shared: Arc<Shared<T>>,
}

impl<T: SftpTransport> Drop for TransferSlot<T> {
    fn drop(&mut self) {
        self.shared.open_transfers.fetch_sub(1, Ordering::SeqCst);
    }
}

fn is_missing(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::Status {
            code: SftpStatus::NoSuchFile,
            ..
        }
    )
}

impl<T: SftpTransport> Shared<T> {
    fn scrub(&self, error: TransportError) -> TransportError {
        match &self.secret {
            Some(secret) => error.scrubbed(secret),
            None => error,
        }
    }

    fn scrub_text(&self, text: &str) -> String {
        match &self.secret {
            Some(secret) => secret.scrub(text),
            None => text.to_owned(),
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

    /// Runs `op` on the connection. Without `only_generation` a lost connection is replaced
    /// within the reconnect budget, and `retry` says whether `op` may be run again on the new
    /// connection (reads only: a request that may have been executed is never repeated
    /// blindly). With `only_generation` (a running transfer) nothing is reconnected: the
    /// handle belongs to that very connection.
    fn run_control<R>(
        &self,
        only_generation: Option<u64>,
        retry: bool,
        op: impl FnMut(&mut T, &SftpFeatures) -> TResult<R>,
    ) -> std::result::Result<R, ControlError> {
        self.run_control_g(only_generation, retry, op)
            .map(|(value, _)| value)
    }

    /// Like `run_control`, also giving the generation of the connection `op` ran on (needed
    /// when `op` opens a handle).
    fn run_control_g<R>(
        &self,
        only_generation: Option<u64>,
        retry: bool,
        mut op: impl FnMut(&mut T, &SftpFeatures) -> TResult<R>,
    ) -> std::result::Result<(R, u64), ControlError> {
        self.check_open().map_err(ControlError::Core)?;
        let mut control = self.control.lock().unwrap();
        let mut attempts = 0;
        loop {
            attempts += 1;
            if let Some(generation) = only_generation {
                if control.transport.is_none() || control.generation != generation {
                    return Err(ControlError::Core(Error::Disconnected));
                }
            }
            if control.transport.is_none() {
                if control.connects > self.cfg.max_reconnects {
                    return Err(ControlError::Core(Error::Disconnected));
                }
                control.connects += 1;
                match (self.opener)() {
                    Ok(transport) => {
                        control.features = transport.features();
                        control.transport = Some(transport);
                        control.generation += 1;
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
                Ok(value) => return Ok((value, control.generation)),
                Err(error) if error.is_connection_lost() => {
                    control.transport = None;
                    if retry && only_generation.is_none() && attempts < 2 {
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

    fn with_control_path<R>(
        &self,
        path: &VPath,
        retry: bool,
        op: impl FnMut(&mut T, &SftpFeatures) -> TResult<R>,
    ) -> Result<R> {
        match self.run_control(None, retry, op) {
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

    /// Closes a handle (best effort; only on the connection it belongs to).
    fn close_handle(&self, generation: u64, handle: SftpHandle) {
        let _ = self.run_control(Some(generation), false, |t, _| t.close(handle));
    }
}

// ------------------------------------------------------------------------------ helpers

fn path_text(path: &VPath) -> Result<String> {
    let text = wire(path)?;
    if text.len() > MAX_PATH_BYTES {
        return Err(Error::InvalidPath(
            "Der Pfad auf dem Server ist zu lang".into(),
        ));
    }
    Ok(text)
}

fn file_name_of(path: &VPath) -> Result<String> {
    path.path()
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))
}

fn parent_of(path: &VPath) -> Result<VPath> {
    path.parent()
        .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad auf dem Server".into()))
}

fn kind_of(attrs: &RawAttrs) -> EntryKind {
    match attrs.kind {
        RawKind::Dir => EntryKind::Dir,
        RawKind::Symlink => EntryKind::Symlink,
        RawKind::File | RawKind::Other | RawKind::Unknown => EntryKind::File,
    }
}

const SPECIAL_FILE: &str = "Der Eintrag ist eine Sonderdatei (Pipe, Gerät, Socket)";

/// A link target for display: control characters replaced, bounded.
fn clean_target(text: &str) -> String {
    let mut shown: String = text
        .chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect();
    if shown.len() > MAX_LINK_TARGET {
        let mut end = MAX_LINK_TARGET;
        while !shown.is_char_boundary(end) {
            end -= 1;
        }
        shown.truncate(end);
    }
    shown
}

fn listed_from(raw: &RawEntry, link_target: Option<String>) -> ListedEntry {
    ListedEntry {
        name: raw.name.clone(),
        kind: kind_of(&raw.attrs),
        size: raw.attrs.size.unwrap_or(0),
        modified: raw.attrs.mtime,
        mode: raw.attrs.permissions,
        link_target,
    }
}

type Listed = Vec<(ListedEntry, Option<&'static str>)>;

/// Lists `dir` on `transport` within the limits. Too many entries is an error, never a
/// silently shortened listing. `.` and `..` that servers send are dropped.
fn list_on<T: SftpTransport>(
    transport: &mut T,
    dir: &str,
    cfg: &SftpConfig,
) -> TResult<(Listed, Vec<RawKind>)> {
    let handle = transport.open_dir(dir)?;
    let started = Instant::now();
    let mut raws: Vec<RawEntry> = Vec::new();
    let mut bytes = 0usize;
    let outcome = 'listing: loop {
        if started.elapsed() > cfg.list_deadline {
            break Err(TransportError::TooLarge("Das Auflisten dauert zu lange"));
        }
        match transport.read_dir(handle) {
            Ok(batch) if batch.is_empty() => break Ok(()),
            Ok(batch) => {
                for raw in batch {
                    if raw.name == "." || raw.name == ".." {
                        continue;
                    }
                    bytes += raw.name.len() + 64;
                    if raws.len() >= cfg.limits.max_entries || bytes > cfg.limits.max_list_bytes {
                        break 'listing Err(TransportError::TooLarge(
                            "Das Verzeichnis hat zu viele Einträge",
                        ));
                    }
                    raws.push(raw);
                }
            }
            Err(error) => break Err(error),
        }
    };
    let _ = transport.close(handle);
    outcome?;
    // link targets: only for usable names, bounded
    let mut link_reads = 0usize;
    let mut listed: Vec<ListedEntry> = Vec::with_capacity(raws.len());
    for raw in &raws {
        let mut target = None;
        if raw.attrs.kind == RawKind::Symlink
            && name_problem(&raw.name).is_none()
            && link_reads < cfg.max_link_reads
        {
            link_reads += 1;
            match transport.readlink(&join_wire(dir, &raw.name)) {
                Ok(text) => target = Some(clean_target(&text)),
                Err(error) if error.is_connection_lost() => return Err(error),
                Err(_) => {}
            }
        }
        listed.push(listed_from(raw, target));
    }
    let marks = problems(&listed);
    let kinds: Vec<RawKind> = raws.iter().map(|r| r.attrs.kind).collect();
    Ok((
        listed
            .into_iter()
            .zip(marks)
            .zip(&kinds)
            .map(|((entry, mark), kind)| {
                let mark = mark.or((*kind == RawKind::Other).then_some(SPECIAL_FILE));
                (entry, mark)
            })
            .collect(),
        kinds,
    ))
}

/// `lstat` that turns "not there" into `None`.
fn lstat_opt<T: SftpTransport>(transport: &mut T, path: &str) -> TResult<Option<RawAttrs>> {
    match transport.lstat(path) {
        Ok(attrs) => Ok(Some(attrs)),
        Err(error) if is_missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_small<T: SftpTransport>(transport: &mut T, path: &str, max: usize) -> TResult<Vec<u8>> {
    let handle = transport.open_read(path)?;
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    let outcome = loop {
        match transport.read(handle, data.len() as u64, &mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.len() > max {
                    break Err(TransportError::TooLarge("Hilfsdatei-Vermerk"));
                }
            }
            Err(error) => break Err(error),
        }
    };
    let _ = transport.close(handle);
    outcome?;
    Ok(data)
}

fn write_small<T: SftpTransport>(
    transport: &mut T,
    path: &str,
    data: &[u8],
    how: OpenWrite,
) -> TResult<()> {
    let handle = transport.open_write(path, how)?;
    let outcome = transport.write(handle, 0, data);
    let closed = transport.close(handle);
    outcome.and(closed)
}

// ------------------------------------------------------------------------------ SftpVfs

impl<T: SftpTransport> SftpVfs<T> {
    /// `first` is the connection the login was checked on. `secret` (if given) is only used
    /// to scrub error texts. `opener` makes new connections.
    pub fn new(
        id: &str,
        first: T,
        opener: Opener<T>,
        cfg: SftpConfig,
        secret: Option<Secret>,
    ) -> Result<Arc<Self>> {
        Self::with_scheme(id, first, opener, cfg, secret, Scheme::Sftp)
    }

    pub(crate) fn with_scheme(
        id: &str,
        first: T,
        opener: Opener<T>,
        cfg: SftpConfig,
        secret: Option<Secret>,
        scheme: Scheme,
    ) -> Result<Arc<Self>> {
        if !dualfile_core::vfs::is_valid_connection_id(id) {
            return Err(Error::InvalidInput("Ungültige Verbindungskennung"));
        }
        let features = first.features();
        Ok(Arc::new(Self {
            shared: Arc::new(Shared {
                scheme,
                id: id.to_owned(),
                opener,
                cfg,
                secret,
                control: Mutex::new(Control {
                    transport: Some(first),
                    features,
                    connects: 1,
                    generation: 1,
                }),
                closed: AtomicBool::new(false),
                open_transfers: AtomicUsize::new(0),
                seq: AtomicU64::new(0),
            }),
        }))
    }

    pub fn id(&self) -> &str {
        &self.shared.id
    }

    /// Closes the connection: later calls give `Error::Disconnected`.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        if let Some(mut transport) = self.shared.control.lock().unwrap().transport.take() {
            transport.quit();
        }
    }

    /// Whether the last transport operation left a usable control connection.
    pub fn is_live(&self) -> bool {
        !self.is_closed() && self.shared.control.lock().unwrap().transport.is_some()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Number of connections made so far (for the reconnect budget).
    pub fn connects(&self) -> u32 {
        self.shared.control.lock().unwrap().connects
    }

    /// The server's start folder (`realpath(".")`), for profiles without a start folder.
    pub fn home_dir(&self) -> Result<String> {
        let found = match self.shared.run_control(None, true, |t, _| t.realpath(".")) {
            Ok(text) => text,
            Err(ControlError::Core(error)) => return Err(error),
            Err(ControlError::Transport(error)) => {
                return Err(Error::Remote(self.shared.scrub(error).to_string()))
            }
        };
        if found.starts_with('/')
            && found.len() <= MAX_PATH_BYTES
            && !found.chars().any(char::is_control)
        {
            Ok(found)
        } else {
            Err(Error::Remote("Ungültiger Startordner vom Server".into()))
        }
    }

    fn own(&self, path: &VPath) -> Result<()> {
        if path.scheme() == self.shared.scheme && path.conn_id() == Some(self.shared.id.as_str()) {
            Ok(())
        } else {
            Err(Error::Disconnected)
        }
    }

    fn lookup(&self, path: &VPath) -> Result<Option<Entry>> {
        self.own(path)?;
        let text = path_text(path)?;
        if text == "/" {
            return Ok(Some(Entry {
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
        let name = file_name_of(path)?;
        let found = self.shared.with_control_path(path, true, |t, _| {
            let Some(attrs) = lstat_opt(t, &text)? else {
                return Ok(None);
            };
            let target = if attrs.kind == RawKind::Symlink {
                t.readlink(&text).ok().map(|s| clean_target(&s))
            } else {
                None
            };
            Ok(Some((attrs, target)))
        })?;
        Ok(found.map(|(attrs, target)| {
            let raw = RawEntry {
                name: name.clone(),
                attrs,
            };
            let listed = listed_from(&raw, target);
            let mark = problems(std::slice::from_ref(&listed))[0]
                .or((raw.attrs.kind == RawKind::Other).then_some(SPECIAL_FILE));
            entry_from(&listed, mark)
        }))
    }

    fn require(&self, path: &VPath) -> Result<Entry> {
        self.lookup(path)?
            .ok_or_else(|| Error::NotFound(path.path().to_path_buf()))
    }

    fn usable_file(&self, path: &VPath) -> Result<Entry> {
        let entry = self.require(path)?;
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
        self.own(path)?;
        self.usable_file(path)?;
        let text = path_text(path)?;
        self.shared.check_open()?;
        let slot = self.shared.take_transfer_slot()?;
        let opened = self
            .shared
            .run_control_g(None, false, |t, _| t.open_read(&text));
        let (handle, generation) = match opened {
            Ok(found) => found,
            Err(ControlError::Core(error)) => return Err(error),
            Err(ControlError::Transport(error)) => return Err(self.shared.core(error, path)),
        };
        Ok(Box::new(SftpReader {
            shared: self.shared.clone(),
            generation,
            handle,
            offset,
            finished: false,
            _slot: slot,
        }))
    }

    fn unique_tmp_name(&self, name: &str) -> String {
        let n = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let candidate = format!("{name}.dualfile-tmp-{n}-{stamp:x}");
        if candidate.len() <= 255 {
            candidate
        } else {
            format!(".dualfile-tmp-{n}-{stamp:x}")
        }
    }
}

// ------------------------------------------------------------------------------ reading

struct SftpReader<T: SftpTransport> {
    shared: Arc<Shared<T>>,
    generation: u64,
    handle: SftpHandle,
    offset: u64,
    finished: bool,
    _slot: TransferSlot<T>,
}

impl<T: SftpTransport> Read for SftpReader<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min(READ_CHUNK);
        let (handle, offset) = (self.handle, self.offset);
        let outcome = self
            .shared
            .run_control(Some(self.generation), false, |t, _| {
                t.read(handle, offset, &mut buf[..want])
            });
        match outcome {
            Ok(0) => {
                self.finished = true;
                Ok(0)
            }
            Ok(n) => {
                self.offset += n as u64;
                Ok(n)
            }
            Err(error) => {
                self.finished = true;
                Err(match error {
                    ControlError::Core(error) => std::io::Error::new(
                        if matches!(error, Error::Disconnected) {
                            std::io::ErrorKind::ConnectionAborted
                        } else {
                            std::io::ErrorKind::Other
                        },
                        self.shared.scrub_text(&error.to_string()),
                    ),
                    ControlError::Transport(error) => {
                        let kind = if error.is_connection_lost() {
                            std::io::ErrorKind::ConnectionAborted
                        } else {
                            std::io::ErrorKind::Other
                        };
                        std::io::Error::new(kind, self.shared.scrub(error).to_string())
                    }
                })
            }
        }
    }
}

impl<T: SftpTransport> Drop for SftpReader<T> {
    fn drop(&mut self) {
        self.shared.close_handle(self.generation, self.handle);
    }
}

// ------------------------------------------------------------------------------ writing

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

struct SftpWrite<T: SftpTransport> {
    shared: Arc<Shared<T>>,
    generation: u64,
    handle: Option<SftpHandle>,
    _slot: TransferSlot<T>,
    part_path: String,
    target: VPath,
    target_wire: String,
    mode: WriteMode,
    kind: PartKind,
    /// Bytes in the partial file before this write started.
    existing: u64,
    written: u64,
    done: bool,
    broken: bool,
}

impl<T: SftpTransport> SftpWrite<T> {
    fn control_error(&self, error: ControlError) -> Error {
        match error {
            ControlError::Core(error) => error,
            ControlError::Transport(error) => self.shared.core(error, &self.target),
        }
    }

    fn commit_inner(&mut self) -> Result<()> {
        let handle = self
            .handle
            .ok_or_else(|| Error::Remote("Der Schreibvorgang wurde bereits beendet".into()))?;
        let expected = self.existing + self.written;
        let part = self.part_path.clone();
        let target = self.target_wire.clone();
        let mode = self.mode;
        let meta = match &self.kind {
            PartKind::Resumable { meta_path, .. } => Some(meta_path.clone()),
            PartKind::Temporary => None,
        };
        let target_path = self.target.path().to_path_buf();
        let secret = self.shared.secret.as_ref();
        // everything runs on the connection of this transfer, in one go
        let outcome = self
            .shared
            .run_control(Some(self.generation), false, |t, features| {
                let attrs = t.fstat(handle)?;
                t.close(handle)?;
                Ok(commit_steps(
                    t,
                    features,
                    attrs.size,
                    expected,
                    &part,
                    &target,
                    mode,
                    meta.as_deref(),
                    &target_path,
                    secret,
                ))
            });
        self.handle = None;
        match outcome {
            Ok(Ok(())) => {
                self.done = true;
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(error) => {
                self.broken = true;
                Err(self.control_error(error))
            }
        }
    }
}

/// The part of a commit after the data is in: size check, then the rename. The inner `Err`
/// is the final answer for the caller (a protocol error that ended the transfer is the outer
/// one).
#[allow(clippy::too_many_arguments)]
fn commit_steps<T: SftpTransport>(
    t: &mut T,
    features: &SftpFeatures,
    size: Option<u64>,
    expected: u64,
    part: &str,
    target: &str,
    mode: WriteMode,
    meta: Option<&str>,
    target_path: &std::path::Path,
    secret: Option<&crate::secret::Secret>,
) -> Result<()> {
    let exists = Error::Exists(target_path.to_path_buf());
    // A server may echo the password in its reply; the error reaches the UI and the log file.
    let map = |e: TransportError| {
        match secret {
            Some(secret) => e.scrubbed(secret),
            None => e,
        }
        .into_core(target_path)
    };
    if size != Some(expected) {
        return Err(Error::SizeMismatch(target_path.to_path_buf()));
    }
    let present = lstat_opt(t, target).map_err(map)?.is_some();
    match mode {
        WriteMode::NoReplace => {
            // plain rename after a look: the window between the two stays
            if present {
                return Err(exists);
            }
            if let Err(error) = t.rename(part, target) {
                // servers answer "exists" with the catch-all failure code
                return match lstat_opt(t, target) {
                    Ok(Some(_)) => Err(exists),
                    _ => Err(map(error)),
                };
            }
        }
        WriteMode::Replace => {
            if features.posix_rename {
                t.posix_rename(part, target).map_err(map)?;
            } else if !present {
                t.rename(part, target).map_err(map)?;
            } else {
                // never delete the original first
                return Err(Error::Unsupported(
                    "Ersetzen auf diesem Server (kein posix-rename)",
                ));
            }
        }
    }
    if let Some(meta) = meta {
        let _ = t.remove(meta);
    }
    Ok(())
}

impl<T: SftpTransport> Write for SftpWrite<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let Some(handle) = self.handle.filter(|_| !self.done) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Der Schreibvorgang wurde bereits beendet",
            ));
        };
        if buf.is_empty() {
            return Ok(0);
        }
        let piece = &buf[..buf.len().min(WRITE_CHUNK)];
        let offset = self.existing + self.written;
        let outcome = self
            .shared
            .run_control(Some(self.generation), false, |t, _| {
                t.write(handle, offset, piece)
            });
        match outcome {
            Ok(()) => {
                self.written += piece.len() as u64;
                Ok(piece.len())
            }
            Err(error) => {
                self.broken = true;
                Err(match error {
                    ControlError::Core(error) => std::io::Error::new(
                        if matches!(error, Error::Disconnected) {
                            std::io::ErrorKind::ConnectionAborted
                        } else {
                            std::io::ErrorKind::Other
                        },
                        self.shared.scrub_text(&error.to_string()),
                    ),
                    ControlError::Transport(error) => std::io::Error::new(
                        if error.is_connection_lost() {
                            std::io::ErrorKind::ConnectionAborted
                        } else {
                            std::io::ErrorKind::Other
                        },
                        self.shared.scrub(error).to_string(),
                    ),
                })
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T: SftpTransport> WriteFinish for SftpWrite<T> {
    fn commit(&mut self) -> Result<()> {
        if self.done {
            return Ok(());
        }
        self.commit_inner()
    }
}

/// Tidies up after a write that was not committed, on `t`: a throw-away file is removed, a
/// resumable one keeps its data and gets a meta file with the length that really arrived.
fn tidy<T: SftpTransport>(
    t: &mut T,
    handle: Option<SftpHandle>,
    part: &str,
    kind: &PartKind,
) -> TResult<()> {
    let mut size = None;
    if let Some(handle) = handle {
        size = t.fstat(handle).ok().and_then(|a| a.size);
        let _ = t.close(handle);
    }
    match kind {
        PartKind::Temporary => {
            let _ = t.remove(part);
        }
        PartKind::Resumable {
            identity,
            meta_path,
        } => {
            if size.is_none() {
                size = lstat_opt(t, part)?.and_then(|a| a.size);
            }
            if let Some(size) = size {
                let meta = PartMeta::new(identity.clone(), size);
                write_small(t, meta_path, &meta.to_bytes(), OpenWrite::CreateTruncate)?;
            }
        }
    }
    Ok(())
}

impl<T: SftpTransport> Drop for SftpWrite<T> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let handle = self.handle.take();
        let part = self.part_path.clone();
        let kind = self.kind.clone();
        let mut tidied = false;
        if !self.broken && !self.shared.closed.load(Ordering::SeqCst) {
            tidied = self
                .shared
                .run_control(Some(self.generation), false, |t, _| {
                    tidy(t, handle, &part, &kind)
                })
                .is_ok();
        }
        if !tidied && !self.shared.closed.load(Ordering::SeqCst) {
            // the connection broke: a fresh one records what arrived / removes the leftover
            if let Ok(mut fresh) = (self.shared.opener)() {
                let _ = tidy(&mut fresh, None, &part, &kind);
                fresh.quit();
            }
        }
    }
}

// ------------------------------------------------------------------------------ Vfs

impl<T: SftpTransport> Vfs for SftpVfs<T> {
    fn space_free(&self, path: &VPath) -> Option<dualfile_core::vfs::SpaceFree> {
        self.own(path).ok()?;
        let text = path_text(path).ok()?;
        // Status refresh must not queue behind transfers or reconnect a session.
        let mut control = self.shared.control.try_lock().ok()?;
        control.transport.as_mut()?.space_free(&text).ok().flatten()
    }

    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        self.own(p)?;
        let dir = path_text(p)?;
        let cfg = self.shared.cfg.clone();
        let (listed, _) = self
            .shared
            .with_control_path(p, true, |t, _| list_on(t, &dir, &cfg))?;
        Ok(listed
            .iter()
            .map(|(entry, problem)| entry_from(entry, *problem))
            .collect())
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        self.require(p)
    }

    fn stat_file(&self, p: &VPath) -> Result<FileStat> {
        let entry = self.usable_file(p)?;
        Ok(FileStat {
            size: entry.size,
            modified: entry.modified,
        })
    }

    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        self.start_download(p, 0)
    }

    fn open_read_from(&self, p: &VPath, offset: u64) -> Result<Box<dyn Read + Send>> {
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
        let target_wire = path_text(p)?;
        let name = file_name_of(p)?;
        if let Some(problem) = name_problem(&name) {
            return Err(Error::InvalidName(problem.to_owned()));
        }
        let parent_wire = path_text(&parent_of(p)?)?;
        self.shared.check_open()?;
        let slot = self.shared.take_transfer_slot()?;
        // a name nobody uses yet, created with "must not exist": nothing existing is touched
        let mut opened = None;
        for _ in 0..20 {
            let candidate = join_wire(&parent_wire, &self.unique_tmp_name(&name));
            let result = self.shared.run_control_g(None, false, |t, _| {
                match t.open_write(&candidate, OpenWrite::CreateNew) {
                    Ok(handle) => Ok(Some(handle)),
                    Err(error) if is_missing(&error) => Err(error),
                    Err(error) => {
                        // "exists" and other refusals are the catch-all failure
                        if lstat_opt(t, &candidate)?.is_some() {
                            Ok(None)
                        } else {
                            Err(error)
                        }
                    }
                }
            });
            match result {
                Ok((Some(handle), generation)) => {
                    opened = Some((candidate, handle, generation));
                    break;
                }
                Ok((None, _)) => continue,
                Err(ControlError::Core(error)) => return Err(error),
                Err(ControlError::Transport(error)) => return Err(self.shared.core(error, p)),
            }
        }
        let (part_path, handle, generation) =
            opened.ok_or_else(|| Error::Remote("Kein freier Name für die Hilfsdatei".into()))?;
        Ok(Box::new(SftpWrite {
            shared: self.shared.clone(),
            generation,
            handle: Some(handle),
            _slot: slot,
            part_path,
            target: p.clone(),
            target_wire,
            mode,
            kind: PartKind::Temporary,
            existing: 0,
            written: 0,
            done: false,
            broken: false,
        }))
    }

    fn create_write_resumable(
        &self,
        p: &VPath,
        expected_source: SourceIdentity,
    ) -> Result<ResumableWrite> {
        if self.shared.scheme == Scheme::Nfs {
            return Err(Error::Unsupported("NFS resume"));
        }
        if self.shared.scheme == Scheme::Smb {
            return Err(Error::Unsupported("SMB resume"));
        }

        self.own(p)?;
        let target_wire = path_text(p)?;
        let name = file_name_of(p)?;
        if let Some(problem) = name_problem(&name) {
            return Err(Error::InvalidName(problem.to_owned()));
        }
        let parent_wire = path_text(&parent_of(p)?)?;
        self.shared.check_open()?;
        let slot = self.shared.take_transfer_slot()?;
        let identity = expected_source.clone();
        let chosen = self.shared.run_control_g(None, false, |t, _| {
            for attempt in 0..20 {
                let part = part_name(&name, attempt).map_err(|_| TransportError::Rejected)?;
                let meta = meta_name(&part);
                let part_path = join_wire(&parent_wire, &part);
                let meta_path = join_wire(&parent_wire, &meta);
                let part_attrs = lstat_opt(t, &part_path)?;
                let meta_attrs = lstat_opt(t, &meta_path)?;
                let meta_bytes = match &meta_attrs {
                    Some(a) if a.kind == RawKind::File && a.size.unwrap_or(u64::MAX) <= 4096 => {
                        Some(read_small(t, &meta_path, 4096)?)
                    }
                    // a meta name that is a folder, a link or huge is not ours
                    Some(_) => continue,
                    None => None,
                };
                let ownership = classify_meta(meta_bytes.as_deref());
                let part_is_file = part_attrs
                    .as_ref()
                    .map(|a| a.kind == RawKind::File)
                    .unwrap_or(true);
                if !part_is_file {
                    continue;
                }
                // (existing length, identity matched, open mode, meta mode)
                let plan = match (&part_attrs, ownership) {
                    (None, Ownership::Foreign) if meta_attrs.is_none() => {
                        (0, false, OpenWrite::CreateNew, OpenWrite::CreateNew)
                    }
                    // a leftover meta of ours without partial file: start over
                    (None, Ownership::Ours(_) | Ownership::Damaged) => {
                        (0, false, OpenWrite::CreateNew, OpenWrite::CreateTruncate)
                    }
                    // somebody else's file(s): never touched, try the next name
                    (None, Ownership::Foreign) | (Some(_), Ownership::Foreign) => continue,
                    (Some(_), Ownership::Damaged) => (
                        0,
                        false,
                        OpenWrite::CreateTruncate,
                        OpenWrite::CreateTruncate,
                    ),
                    (Some(attrs), Ownership::Ours(found)) => {
                        match decide_resume(&found, &identity, attrs.size) {
                            ResumeDecision::Resume { len } => {
                                (len, true, OpenWrite::Existing, OpenWrite::CreateTruncate)
                            }
                            ResumeDecision::Fresh(_) => (
                                0,
                                false,
                                OpenWrite::CreateTruncate,
                                OpenWrite::CreateTruncate,
                            ),
                        }
                    }
                };
                let (existing, matches, how, meta_how) = plan;
                // record the identity first, then continue or start the partial file
                let meta_now = PartMeta::new(identity.clone(), existing);
                write_small(t, &meta_path, &meta_now.to_bytes(), meta_how)?;
                let handle = t.open_write(&part_path, how)?;
                return Ok(Some((part_path, meta_path, existing, matches, handle)));
            }
            Ok(None)
        });
        let (chosen, generation) = match chosen {
            Ok((Some(chosen), generation)) => (chosen, generation),
            Ok((None, _)) => {
                return Err(Error::Remote("Kein freier Name für die Hilfsdatei".into()))
            }
            Err(ControlError::Core(error)) => return Err(error),
            Err(ControlError::Transport(error)) => return Err(self.shared.core(error, p)),
        };
        let (part_path, meta_path, existing, matches, handle) = chosen;
        Ok(ResumableWrite {
            existing_len: existing,
            identity_matches: matches,
            writer: Box::new(SftpWrite {
                shared: self.shared.clone(),
                generation,
                handle: Some(handle),
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
                done: false,
                broken: false,
            }),
        })
    }

    fn discard_resumable(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let name = file_name_of(p)?;
        let parent_wire = path_text(&parent_of(p)?)?;
        self.shared.with_control_path(p, false, |t, _| {
            for attempt in 0..20 {
                let Ok(part) = part_name(&name, attempt) else {
                    break;
                };
                let meta_path = join_wire(&parent_wire, &meta_name(&part));
                let part_path = join_wire(&parent_wire, &part);
                let Some(meta_attrs) = lstat_opt(t, &meta_path)? else {
                    continue;
                };
                if meta_attrs.kind != RawKind::File || meta_attrs.size.unwrap_or(u64::MAX) > 4096 {
                    continue;
                }
                let bytes = read_small(t, &meta_path, 4096)?;
                // only what the meta file proves to be ours is removed
                if matches!(
                    classify_meta(Some(&bytes)),
                    Ownership::Ours(_) | Ownership::Damaged
                ) {
                    if let Some(a) = lstat_opt(t, &part_path)? {
                        if a.kind == RawKind::File {
                            t.remove(&part_path)?;
                        }
                    }
                    t.remove(&meta_path)?;
                }
            }
            Ok(())
        })
    }

    fn mkdir(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let path = path_text(p)?;
        let outcome = self
            .shared
            .with_control_path(p, false, |t, _| t.mkdir(&path));
        match outcome {
            Err(Error::Remote(_)) | Err(Error::NotFound(_)) => {
                // the catch-all failure also means "exists": ask
                match self.lookup(p)? {
                    Some(_) => Err(Error::Exists(p.path().to_path_buf())),
                    None => outcome,
                }
            }
            other => other,
        }
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        self.own(from)?;
        self.own(to)?;
        let (a, b) = (path_text(from)?, path_text(to)?);
        let target_path = to.path().to_path_buf();
        let outcome = self.shared.run_control(None, false, |t, features| {
            let present = lstat_opt(t, &b)?.is_some();
            match mode {
                ReplaceMode::NoReplace => {
                    if present {
                        return Ok(Err(Error::Exists(target_path.clone())));
                    }
                    match t.rename(&a, &b) {
                        Ok(()) => Ok(Ok(())),
                        Err(error) => match lstat_opt(t, &b) {
                            Ok(Some(_)) => Ok(Err(Error::Exists(target_path.clone()))),
                            _ => Err(error),
                        },
                    }
                }
                ReplaceMode::Replace => {
                    if features.posix_rename {
                        t.posix_rename(&a, &b).map(Ok)
                    } else if !present {
                        t.rename(&a, &b).map(Ok)
                    } else {
                        Ok(Err(Error::Unsupported(
                            "Ersetzen auf diesem Server (kein posix-rename)",
                        )))
                    }
                }
            }
        });
        match outcome {
            Ok(inner) => inner,
            Err(ControlError::Core(error)) => Err(error),
            Err(ControlError::Transport(error)) => Err(self.shared.core(error, from)),
        }
    }

    fn read_link(&self, p: &VPath) -> Result<String> {
        self.own(p)?;
        let path = path_text(p)?;
        let found = self.shared.with_control_path(p, true, |t, _| {
            let attrs = t.lstat(&path)?;
            if attrs.kind != RawKind::Symlink {
                return Ok(None);
            }
            t.readlink(&path).map(Some)
        })?;
        found
            .map(|text| clean_target(&text))
            .ok_or(Error::Unsupported("Linkziel lesen"))
    }

    fn create_symlink(&self, target: &str, at: &VPath) -> Result<()> {
        self.own(at)?;
        if target.is_empty()
            || target.len() > MAX_LINK_TARGET
            || target.chars().any(char::is_control)
        {
            return Err(Error::InvalidPath("Ungültiges Linkziel".into()));
        }
        let path = path_text(at)?;
        let result = self.shared.run_control(None, false, |t, _| {
            if lstat_opt(t, &path)?.is_some() {
                return Ok(false);
            }
            t.symlink(target, &path).map(|()| true)
        });
        match result {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::Exists(at.path().to_path_buf())),
            Err(ControlError::Core(error)) => Err(error),
            Err(ControlError::Transport(error)) => Err(self.shared.core(error, at)),
        }
    }

    fn trash(&self, _p: &VPath) -> Result<()> {
        // never replaced by a permanent delete
        Err(Error::Unsupported("Papierkorb auf Servern"))
    }

    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        let path = path_text(p)?;
        let entry = self.require(p)?;
        let outcome = self.shared.with_control_path(p, false, |t, _| {
            if entry.kind == EntryKind::Dir {
                t.rmdir(&path)
            } else {
                // a link is removed itself, never its target
                t.remove(&path)
            }
        });
        match outcome {
            Err(error) if entry.kind == EntryKind::Dir => {
                // the catch-all failure also means "not empty": ask
                if let Ok(children) = self.list(p) {
                    if !children.is_empty() {
                        return Err(Error::DirectoryNotEmpty(p.path().to_path_buf()));
                    }
                }
                Err(error)
            }
            other => other,
        }
    }

    fn set_metadata(
        &self,
        p: &VPath,
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) -> Result<()> {
        self.own(p)?;
        if mode.is_none() && modified.is_none() {
            return Ok(());
        }
        let path = path_text(p)?;
        self.shared.with_control_path(p, false, |t, _| {
            let attrs = t.lstat(&path)?;
            if attrs.kind == RawKind::Symlink {
                // links are never touched
                return Ok(());
            }
            let atime = modified.map(|m| attrs.atime.unwrap_or(m));
            t.setstat(&path, mode.map(|m| m & 0o7777), modified, atime)
        })
    }

    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: false,
            can_rename_in_place: true,
        }
    }

    fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
        Some((self.shared.scheme, &self.shared.id))
    }
}
