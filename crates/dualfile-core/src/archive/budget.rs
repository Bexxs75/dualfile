//! Cost accounting: budgets, the counting reader and the error mapping of I/O failures.

use super::index::ArchiveLimits;
use crate::error::{from_io, Error};
use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const MSG_INDEX_BYTES: &str = "Archiv zu groß für die Anzeige";
pub(crate) const MSG_INDEX_TIME: &str = "Archiv zu langsam für die Anzeige";
const MSG_READ_BYTES: &str = "Dekomprimierte Datenmenge über dem Limit (Lesevorgang)";
const MSG_READ_TIME: &str = "Zeitlimit beim Dekomprimieren überschritten";
const MSG_JOB_BYTES: &str = "Dekomprimierte Datenmenge über dem Limit (Auftrag)";
const MSG_JOB_TIME: &str = "Zeitlimit für den Auftrag überschritten";
const MSG_FILE_TOO_BIG: &str = "Datei im Archiv zu groß (Limit je Datei)";
const MSG_RATIO: &str = "Kompressionsverhältnis zu hoch (mögliche Zip-Bombe)";
pub(crate) const MSG_CORRUPT: &str = "Archiv ist beschädigt oder abgeschnitten";
pub(crate) const MSG_ENCRYPTED: &str = "verschlüsselt, nicht unterstützt";

/// A limit was hit while reading; carried inside an `io::Error` through decoders.
#[derive(Debug)]
pub(crate) struct LimitHit(pub &'static str);
impl std::fmt::Display for LimitHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for LimitHit {}

pub(crate) fn limit(message: &'static str) -> io::Error {
    io::Error::other(LimitHit(message))
}
pub(crate) fn corrupt(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Maps an I/O error from a decoder to a user-facing error: our limits keep their message,
/// decoder failures and truncation become "damaged archive", real I/O errors stay I/O.
pub(crate) fn map_io(error: io::Error, path: &Path) -> Error {
    let mut current: Option<&(dyn std::error::Error + 'static)> =
        error.get_ref().map(|inner| inner as _);
    while let Some(inner) = current {
        if let Some(hit) = inner.downcast_ref::<LimitHit>() {
            return Error::Archive(hit.0);
        }
        current = inner.source();
    }
    match error.kind() {
        io::ErrorKind::InvalidData
        | io::ErrorKind::InvalidInput
        | io::ErrorKind::UnexpectedEof
        | io::ErrorKind::Other => Error::Archive(MSG_CORRUPT),
        _ => from_io("Archiv lesen", path, error),
    }
}

struct BudgetInner {
    max_bytes: u64,
    max_time: Option<Duration>,
    start: Mutex<Instant>,
    used: AtomicU64,
    bytes_message: &'static str,
    time_message: &'static str,
}

/// Counter for actually decompressed bytes and elapsed time with upper bounds. Clones share
/// the counter, so one budget can span several archives (nesting levels) or a whole job.
#[derive(Clone)]
pub struct Budget(Arc<BudgetInner>);

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("used", &self.used_bytes())
            .field("max_bytes", &self.0.max_bytes)
            .field("max_time", &self.0.max_time)
            .finish()
    }
}

impl Budget {
    fn make(
        max_bytes: u64,
        max_time: Option<Duration>,
        bytes_message: &'static str,
        time_message: &'static str,
    ) -> Self {
        Self(Arc::new(BudgetInner {
            max_bytes,
            max_time,
            start: Mutex::new(Instant::now()),
            used: AtomicU64::new(0),
            bytes_message,
            time_message,
        }))
    }
    /// Generic budget for a read or search job; the clock starts now.
    pub fn new(max_bytes: u64, max_time: Option<Duration>) -> Self {
        Self::make(max_bytes, max_time, MSG_JOB_BYTES, MSG_JOB_TIME)
    }
    pub(crate) fn index(max_bytes: u64, max_time: Duration) -> Self {
        Self::make(max_bytes, Some(max_time), MSG_INDEX_BYTES, MSG_INDEX_TIME)
    }
    pub(crate) fn read_op(max_bytes: u64, max_time: Duration) -> Self {
        Self::make(max_bytes, Some(max_time), MSG_READ_BYTES, MSG_READ_TIME)
    }
    pub(crate) fn job(max_bytes: u64, max_time: Option<Duration>) -> Self {
        Self::make(max_bytes, max_time, MSG_JOB_BYTES, MSG_JOB_TIME)
    }
    pub(crate) fn unlimited() -> Self {
        Self::make(u64::MAX, None, MSG_READ_BYTES, MSG_READ_TIME)
    }

    pub fn used_bytes(&self) -> u64 {
        self.0.used.load(Ordering::Relaxed)
    }
    pub fn elapsed(&self) -> Duration {
        self.0.start.lock().unwrap().elapsed()
    }
    /// Restarts clock and counter.
    pub fn reset(&self) {
        *self.0.start.lock().unwrap() = Instant::now();
        self.0.used.store(0, Ordering::Relaxed);
    }

    pub(crate) fn check_time(&self) -> io::Result<()> {
        match self.0.max_time {
            Some(max) if self.elapsed() >= max => Err(limit(self.0.time_message)),
            _ => Ok(()),
        }
    }
    pub(crate) fn charge(&self, bytes: u64) -> io::Result<()> {
        self.check_time()?;
        let total = self
            .0
            .used
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        if total > self.0.max_bytes {
            return Err(limit(self.0.bytes_message));
        }
        Ok(())
    }
}

/// The per-operation budget of an open read cursor; replaced for every `read_range` call.
#[derive(Clone)]
pub(crate) struct BudgetSlot(Arc<Mutex<Budget>>);
impl BudgetSlot {
    pub(crate) fn new(budget: Budget) -> Self {
        Self(Arc::new(Mutex::new(budget)))
    }
    pub(crate) fn unbounded() -> Self {
        Self::new(Budget::unlimited())
    }
    pub(crate) fn set(&self, budget: Budget) {
        *self.0.lock().unwrap() = budget;
    }
    fn current(&self) -> Budget {
        self.0.lock().unwrap().clone()
    }
}

/// What a stream needs to account its cost.
pub(crate) struct ReadCtx {
    pub job: Budget,
    pub op: BudgetSlot,
    pub limits: ArchiveLimits,
}

/// Compressed bytes consumed and decompressed bytes produced by one decoder chain.
#[derive(Default)]
pub(crate) struct Stats {
    pub compressed: AtomicU64,
    pub produced: AtomicU64,
}

/// Counts what is read from the compressed source.
pub(crate) struct CountSrc<R> {
    pub inner: R,
    pub stats: Arc<Stats>,
}
impl<R: Read> Read for CountSrc<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.stats.compressed.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}
impl<R: Seek> Seek for CountSrc<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// The reader at the decompressed end of a decoder chain: charges every byte to the
/// budgets, enforces the per-file cap and the compression ratio, and (when the size is
/// declared) checks that exactly that many bytes arrive. The announced size is never
/// believed for anything but this cross-check.
pub(crate) struct Counted<R> {
    inner: R,
    budgets: Vec<Budget>,
    slot: Option<BudgetSlot>,
    stats: Arc<Stats>,
    ratio: Option<(u64, u64)>,
    cap: Option<u64>,
    declared: Option<u64>,
    produced: u64,
}

impl<R> Counted<R> {
    /// For reading an entry: job budget, per-operation slot, per-file cap, ratio.
    pub(crate) fn for_entry(
        inner: R,
        ctx: &ReadCtx,
        stats: Arc<Stats>,
        declared: Option<u64>,
    ) -> Self {
        Self {
            inner,
            budgets: vec![ctx.job.clone()],
            slot: Some(ctx.op.clone()),
            stats,
            ratio: Some((ctx.limits.ratio_min_bytes, ctx.limits.ratio_max)),
            cap: Some(ctx.limits.max_file_bytes),
            declared,
            produced: 0,
        }
    }
    /// For building an index: the given budgets and (optionally) the ratio guard.
    pub(crate) fn for_index(
        inner: R,
        budgets: Vec<Budget>,
        stats: Arc<Stats>,
        ratio: Option<(u64, u64)>,
    ) -> Self {
        Self {
            inner,
            budgets,
            slot: None,
            stats,
            ratio,
            cap: None,
            declared: None,
            produced: 0,
        }
    }
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        for budget in &self.budgets {
            budget.check_time()?;
        }
        let n = self.inner.read(buf)?;
        if n == 0 {
            return match self.declared {
                Some(size) if size != self.produced => Err(corrupt(MSG_CORRUPT)),
                _ => Ok(0),
            };
        }
        let n64 = n as u64;
        self.produced += n64;
        self.stats.produced.fetch_add(n64, Ordering::Relaxed);
        if self.declared.is_some_and(|size| self.produced > size) {
            return Err(corrupt(MSG_CORRUPT));
        }
        if self.cap.is_some_and(|cap| self.produced > cap) {
            return Err(limit(MSG_FILE_TOO_BIG));
        }
        for budget in &self.budgets {
            budget.charge(n64)?;
        }
        if let Some(slot) = &self.slot {
            slot.current().charge(n64)?;
        }
        if let Some((min_bytes, max_ratio)) = self.ratio {
            let compressed = self.stats.compressed.load(Ordering::Relaxed).max(1);
            if self.produced > min_bytes && self.produced / compressed > max_ratio {
                return Err(limit(MSG_RATIO));
            }
        }
        Ok(n)
    }
}
impl<R: Seek> Seek for Counted<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Settings for the external RAR helper programs.
#[derive(Clone, Debug)]
pub struct RarConfig {
    /// Directories searched for `unrar`/`7zz`/`7z`; `None` means the `PATH` of the process.
    pub search_path: Option<OsString>,
    /// A helper that shows no progress (no output) for this long is killed.
    pub timeout: Duration,
    /// Upper bound for the listing output.
    pub output_limit: u64,
}

impl Default for RarConfig {
    fn default() -> Self {
        Self {
            search_path: None,
            timeout: Duration::from_secs(60),
            output_limit: 64 << 20,
        }
    }
}
