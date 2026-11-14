//! Read-only access to archive files as a virtual file system (`ArchiveVfs`).
//!
//! The index of an archive is built once in memory (with hard limits, in a separate thread
//! guarded against panics). Sizes announced inside an archive are never trusted: what counts
//! is what was actually decompressed. Entries that could leave the archive (`..`, absolute
//! paths, NUL, backslash or drive tricks) are kept in the index but marked unsafe and can
//! never be read or copied. Links are listed as links and never followed.

mod budget;
mod external;
mod index;
mod pack;
mod rar;
mod sevenz;
mod tarfmt;
mod zipfmt;

pub(crate) use budget::{corrupt, map_io, BudgetSlot, ReadCtx, MSG_CORRUPT};
pub use budget::{Budget, RarConfig};
pub use external::{ExternalEntry, ExternalPacker, PackerBinding};
pub use index::ArchiveLimits;
use index::{Flag, IndexBuilder, Loc, Node};
pub use pack::{can_write as can_pack, pack};

use crate::error::{Error, Result};
use crate::vfs::{
    Caps, Entry, EntryKind, FileStat, ReplaceMode, VPath, Vfs, WriteFinish, WriteMode,
};
use std::io::{Read, Seek};
use std::panic::AssertUnwindSafe;
use std::path::{Component, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const MSG_READ_ONLY: &str = "Archive sind schreibgeschützt";
const MSG_PARSER_PANIC: &str = "Archiv beschädigt oder nicht lesbar (interner Parserfehler)";
pub(crate) const MSG_NOT_AN_ARCHIVE: &str = "Die Datei ist kein erkanntes Archiv";

pub(crate) trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Where the bytes of an archive come from: a local file (re-opened for every access) or the
/// memory of an archive that was itself read from another archive.
#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Memory(Arc<Vec<u8>>),
}

impl Source {
    pub(crate) fn open(&self) -> std::io::Result<Box<dyn ReadSeek>> {
        match self {
            Source::File(path) => Ok(Box::new(std::fs::File::open(path)?)),
            Source::Memory(data) => Ok(Box::new(std::io::Cursor::new(MemoryBytes(data.clone())))),
        }
    }
    pub(crate) fn len(&self) -> std::io::Result<u64> {
        match self {
            Source::File(path) => Ok(std::fs::metadata(path)?.len()),
            Source::Memory(data) => Ok(data.len() as u64),
        }
    }
}

struct MemoryBytes(Arc<Vec<u8>>);
impl AsRef<[u8]> for MemoryBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TarCompression {
    None,
    Gzip,
    Bzip2,
    Xz,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Zip,
    Tar(TarCompression),
    SevenZ,
    Rar,
    /// Served by a plugin packer (see `ExternalPacker`).
    Plugin,
}

fn has_tar_extension(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tbz", ".tar.xz", ".txz",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// A tar header block: valid checksum over the 512 bytes.
fn looks_like_tar_header(block: &[u8]) -> bool {
    if block.len() < 512 {
        return false;
    }
    let stored = std::str::from_utf8(&block[148..156])
        .ok()
        .map(|text| text.trim_matches(|c| c == '\0' || c == ' '))
        .and_then(|text| u32::from_str_radix(text, 8).ok());
    let Some(stored) = stored else { return false };
    let sum: u32 = block[..148]
        .iter()
        .chain(&[b' '; 8])
        .chain(&block[156..512])
        .map(|byte| u32::from(*byte))
        .sum();
    sum == stored
}

/// Detects the format from the signature (first bytes) AND the file name. The signature
/// decides for ZIP/7Z/RAR; compressed streams count as tar when the extension says so or
/// the decompressed content starts with a valid tar header; a plain tar needs `ustar` or
/// the `.tar` extension with a valid header. Returns `None` for everything else.
pub fn detect_format(name: &str, source: &Source) -> Result<Option<Format>> {
    let mut head = Vec::new();
    source
        .open()
        .and_then(|file| file.take(560).read_to_end(&mut head))
        .map_err(|e| map_io(e, std::path::Path::new(name)))?;
    let starts = |magic: &[u8]| head.starts_with(magic);
    let compression = if starts(&[0x1f, 0x8b]) {
        Some(TarCompression::Gzip)
    } else if starts(b"BZh") {
        Some(TarCompression::Bzip2)
    } else if starts(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Some(TarCompression::Xz)
    } else {
        None
    };
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
        return Ok(Some(Format::Zip));
    }
    if starts(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        return Ok(Some(Format::SevenZ));
    }
    if starts(b"Rar!\x1a\x07") {
        return Ok(Some(Format::Rar));
    }
    if let Some(compression) = compression {
        if has_tar_extension(name) {
            return Ok(Some(Format::Tar(compression)));
        }
        // Without a tar extension only a tar stream counts (a bare .gz is no archive).
        let first_block = tarfmt::first_block(source, compression).unwrap_or_default();
        return Ok(looks_like_tar_header(&first_block).then_some(Format::Tar(compression)));
    }
    let ustar = head.len() >= 262 && &head[257..262] == b"ustar";
    if ustar || (name.to_lowercase().ends_with(".tar") && looks_like_tar_header(&head)) {
        return Ok(Some(Format::Tar(TarCompression::None)));
    }
    Ok(None)
}

/// Runs `job` in its own thread; a panic inside (corrupt data hitting a parser bug)
/// becomes an error instead of taking the application down.
fn guarded<T: Send + 'static>(job: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    let handle = std::thread::Builder::new()
        .name("dualfile-archive-index".into())
        .stack_size(8 << 20)
        .spawn(move || std::panic::catch_unwind(AssertUnwindSafe(job)))
        .map_err(|_| Error::Archive(MSG_PARSER_PANIC))?;
    match handle.join() {
        Ok(Ok(result)) => result,
        _ => Err(Error::Archive(MSG_PARSER_PANIC)),
    }
}

/// Reader that turns a panic in a decoder into an I/O error.
struct Guarded(Box<dyn Read + Send>);
impl Read for Guarded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::panic::catch_unwind(AssertUnwindSafe(|| self.0.read(buf)))
            .unwrap_or_else(|_| Err(corrupt(MSG_PARSER_PANIC)))
    }
}

enum Backend {
    Zip(zipfmt::ZipBackend),
    Tar(tarfmt::TarBackend),
    SevenZ(sevenz::SevenBackend),
    Rar(rar::RarBackend),
    External(external::ExternalBackend),
}

impl Backend {
    fn open_entry(&self, node: &Node, ctx: &ReadCtx) -> Result<Box<dyn Read + Send>> {
        let path = std::path::Path::new(&node.name);
        if let (Backend::External(b), Loc::External(member)) = (self, &node.loc) {
            return b.open_entry(member, node.size, ctx);
        }
        let stream = match (self, &node.loc) {
            (Backend::Zip(b), Loc::Zip(loc)) => b.open_entry(loc, ctx),
            (Backend::Tar(b), Loc::Tar(loc)) => b.open_entry(loc, ctx),
            (Backend::SevenZ(b), Loc::SevenZ(loc)) => b.open_entry(loc, ctx),
            (Backend::Rar(b), Loc::Rar(loc)) => b.open_entry(loc, node.size, ctx),
            _ => Err(corrupt(MSG_CORRUPT)),
        };
        stream.map_err(|e| map_io(e, path))
    }
}

struct Cursor {
    entry: usize,
    pos: u64,
    slot: BudgetSlot,
    reader: Box<dyn Read + Send>,
}

/// A read-only `Vfs` over one archive file. Paths are `VPath::archive(outer, "/inner")` with
/// `outer` equal to the location this archive was opened from.
pub struct ArchiveVfs {
    outer: VPath,
    format: Format,
    limits: ArchiveLimits,
    nodes: Vec<Node>,
    backend: Backend,
    job: Mutex<Budget>,
    cursor: Mutex<Option<Cursor>>,
}

impl ArchiveVfs {
    /// Budget for building the index; pass the same one to every nesting level of a chain.
    pub fn new_index_budget(limits: &ArchiveLimits) -> Budget {
        Budget::index(limits.index_bytes_tar, limits.index_time)
    }

    /// Opens a local archive file with the default limits.
    pub fn open_local(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_local_with(path, ArchiveLimits::default())
    }

    pub fn open_local_with(
        path: impl AsRef<std::path::Path>,
        limits: ArchiveLimits,
    ) -> Result<Self> {
        let outer = VPath::local(path.as_ref())?;
        let budget = Self::new_index_budget(&limits);
        Self::open(
            outer,
            Source::File(path.as_ref().to_path_buf()),
            limits,
            budget,
        )
    }

    /// Builds the index. Fails with a clear message when the format is unknown, the archive
    /// is damaged, too big or too slow to index, or a limit is exceeded.
    pub fn open(
        outer: VPath,
        source: Source,
        limits: ArchiveLimits,
        index_budget: Budget,
    ) -> Result<Self> {
        Self::open_with(outer, source, limits, index_budget, &[])
    }

    /// Like `open`; a file the built-in formats do not recognize is offered to `packers`
    /// (by file name extension).
    pub fn open_with(
        outer: VPath,
        source: Source,
        limits: ArchiveLimits,
        index_budget: Budget,
        packers: &[PackerBinding],
    ) -> Result<Self> {
        let packers = packers.to_vec();
        let name = outer
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let job_limits = limits.clone();
        let (format, nodes, backend) = guarded(move || {
            let detected = detect_format(&name, &source)?;
            let plugin = match detected {
                Some(_) => None,
                None => external::find(&packers, &name),
            };
            let format = match (detected, plugin) {
                (Some(format), _) => format,
                (None, Some(_)) => Format::Plugin,
                (None, None) => return Err(Error::Archive(MSG_NOT_AN_ARCHIVE)),
            };
            let mut builder = IndexBuilder::new(&limits, index_budget.clone());
            let backend = match format {
                Format::Plugin => Backend::External(external::index(
                    &source,
                    &name,
                    plugin.expect("a plugin format has its binding"),
                    &limits,
                    &mut builder,
                )?),
                Format::Zip => Backend::Zip(zipfmt::index(
                    &source,
                    &limits,
                    &index_budget,
                    &mut builder,
                )?),
                Format::Tar(compression) => Backend::Tar(tarfmt::index(
                    &source,
                    compression,
                    &limits,
                    &index_budget,
                    &mut builder,
                )?),
                Format::SevenZ => Backend::SevenZ(sevenz::index(
                    &source,
                    &limits,
                    &index_budget,
                    &mut builder,
                )?),
                Format::Rar => Backend::Rar(rar::index(&source, &limits, &mut builder)?),
            };
            Ok((format, builder.finish(), backend))
        })?;
        let job = Budget::job(job_limits.job_bytes, None);
        Ok(Self {
            outer,
            format,
            limits: job_limits,
            nodes,
            backend,
            job: Mutex::new(job),
            cursor: Mutex::new(None),
        })
    }

    pub fn outer(&self) -> &VPath {
        &self.outer
    }
    pub fn format(&self) -> Format {
        self.format
    }
    pub fn root(&self) -> VPath {
        VPath::archive(self.outer.clone(), "/").expect("archive root path is valid")
    }

    /// Number of index nodes (entries plus implicit folders, without the root).
    pub fn node_count(&self) -> usize {
        self.nodes.len() - 1
    }

    /// Replaces the cost budget of the current job (a copy or a search); the default is
    /// `limits.job_bytes` without a time limit. Open read cursors are dropped.
    pub fn set_job_budget(&self, budget: Budget) {
        *self.job.lock().unwrap() = budget;
        *self.cursor.lock().unwrap() = None;
    }
    /// Starts a fresh cost budget with the default job limits.
    pub fn reset_job_budget(&self) {
        self.set_job_budget(Budget::job(self.limits.job_bytes, None));
    }
    /// Actually decompressed bytes and elapsed time of the current job.
    pub fn cost(&self) -> (u64, Duration) {
        let job = self.job.lock().unwrap().clone();
        (job.used_bytes(), job.elapsed())
    }

    fn check_owner(&self, p: &VPath) -> Result<()> {
        if p.outer() != Some(&self.outer) {
            return Err(Error::InvalidPath(
                "Der Pfad gehört nicht zu diesem Archiv".into(),
            ));
        }
        Ok(())
    }

    fn lookup(&self, p: &VPath) -> Result<usize> {
        self.check_owner(p)?;
        let mut current = 0;
        for component in p.path().components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let name = name
                        .to_str()
                        .ok_or_else(|| Error::InvalidPath("Ungültiger Pfad im Archiv".into()))?;
                    let node = &self.nodes[current];
                    if node.kind != EntryKind::Dir {
                        return Err(Error::NotADirectory(p.path().to_path_buf()));
                    }
                    current = *node
                        .children
                        .get(name)
                        .ok_or_else(|| Error::NotFound(p.path().to_path_buf()))?;
                }
                _ => return Err(Error::InvalidPath("Ungültiger Pfad im Archiv".into())),
            }
        }
        Ok(current)
    }

    fn entry_of(&self, index: usize) -> Entry {
        let node = &self.nodes[index];
        let name = if index == 0 {
            self.outer
                .path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into())
        } else {
            node.name.clone()
        };
        Entry {
            hidden: index != 0 && name.starts_with('.'),
            name,
            kind: node.kind,
            size: if node.kind == EntryKind::File {
                node.size
            } else {
                0
            },
            modified: node.modified,
            mode: node.mode,
            link_target: node.link_target.clone(),
            broken_link: false,
            error: match node.flag {
                Flag::Ok => None,
                Flag::Unsafe(reason) | Flag::Unreadable(reason) => Some(reason.to_string()),
            },
        }
    }

    /// Index of a node that may be read as a regular file; refuses everything else.
    fn readable_file(&self, p: &VPath) -> Result<usize> {
        let index = self.lookup(p)?;
        let node = &self.nodes[index];
        match (node.kind, node.flag) {
            (_, Flag::Unsafe(_)) => Err(Error::InvalidPath("Unsicherer Archiveintrag".into())),
            (EntryKind::Dir, _) => Err(Error::IsADirectory(p.path().to_path_buf())),
            (EntryKind::Symlink, _) => Err(Error::Unsupported(
                "Verknüpfungen im Archiv werden nicht verfolgt",
            )),
            (_, Flag::Unreadable(reason)) => Err(Error::Archive(reason)),
            (EntryKind::File, Flag::Ok) => Ok(index),
        }
    }

    fn open_stream(&self, index: usize, slot: &BudgetSlot) -> Result<Box<dyn Read + Send>> {
        let ctx = ReadCtx {
            job: self.job.lock().unwrap().clone(),
            op: slot.clone(),
            limits: self.limits.clone(),
        };
        let stream = self.backend.open_entry(&self.nodes[index], &ctx)?;
        Ok(Box::new(Guarded(stream)))
    }

    fn denied<T>(&self) -> Result<T> {
        Err(Error::Unsupported(MSG_READ_ONLY))
    }
}

impl Vfs for ArchiveVfs {
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        let index = self.lookup(p)?;
        let node = &self.nodes[index];
        if node.kind != EntryKind::Dir {
            return Err(Error::NotADirectory(p.path().to_path_buf()));
        }
        Ok(node
            .children
            .values()
            .map(|child| self.entry_of(*child))
            .collect())
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        Ok(self.entry_of(self.lookup(p)?))
    }

    fn stat_file(&self, p: &VPath) -> Result<FileStat> {
        let index = self.readable_file(p)?;
        let node = &self.nodes[index];
        Ok(FileStat {
            size: node.size,
            modified: node.modified,
        })
    }

    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        let index = self.readable_file(p)?;
        self.open_stream(index, &BudgetSlot::unbounded())
    }

    /// Reads from one forward-running decoder per open entry: an offset behind the cursor
    /// skips forward (and counts against the budgets), an offset before it restarts the
    /// entry (and counts again).
    fn read_range(&self, p: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let index = self.readable_file(p)?;
        if offset >= self.nodes[index].size || len == 0 {
            return Ok(Vec::new());
        }
        let path = p.path();
        let op = Budget::read_op(self.limits.read_bytes, self.limits.read_time);
        let reusable = {
            let mut guard = self.cursor.lock().unwrap();
            match guard.take() {
                Some(cursor) if cursor.entry == index && cursor.pos <= offset => Some(cursor),
                _ => None,
            }
        };
        let mut cursor = match reusable {
            Some(cursor) => {
                cursor.slot.set(op);
                cursor
            }
            None => {
                let slot = BudgetSlot::new(op);
                Cursor {
                    entry: index,
                    pos: 0,
                    reader: self.open_stream(index, &slot)?,
                    slot,
                }
            }
        };
        let mut buffer = vec![0u8; 64 * 1024];
        while cursor.pos < offset {
            let want = (offset - cursor.pos).min(buffer.len() as u64) as usize;
            let n = cursor
                .reader
                .read(&mut buffer[..want])
                .map_err(|e| map_io(e, path))?;
            if n == 0 {
                return Ok(Vec::new());
            }
            cursor.pos += n as u64;
        }
        let mut out = Vec::with_capacity(len.min(1 << 20));
        while out.len() < len {
            let want = (len - out.len()).min(buffer.len());
            let n = cursor
                .reader
                .read(&mut buffer[..want])
                .map_err(|e| map_io(e, path))?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buffer[..n]);
            cursor.pos += n as u64;
        }
        *self.cursor.lock().unwrap() = Some(cursor);
        Ok(out)
    }

    fn read_link(&self, p: &VPath) -> Result<String> {
        let index = self.lookup(p)?;
        let node = &self.nodes[index];
        match (&node.kind, &node.link_target) {
            (EntryKind::Symlink, Some(target)) => Ok(target.clone()),
            (EntryKind::Symlink, None) => Err(Error::Unsupported("Das Linkziel ist nicht lesbar")),
            _ => Err(Error::InvalidInput("Der Eintrag ist keine Verknüpfung")),
        }
    }

    fn create_write(&self, _p: &VPath, _mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        self.denied()
    }
    fn mkdir(&self, _p: &VPath) -> Result<()> {
        self.denied()
    }
    fn rename(&self, _from: &VPath, _to: &VPath, _mode: ReplaceMode) -> Result<()> {
        self.denied()
    }
    fn create_symlink(&self, _target: &str, _at: &VPath) -> Result<()> {
        self.denied()
    }
    fn trash(&self, _p: &VPath) -> Result<()> {
        self.denied()
    }
    fn remove_permanent(&self, _p: &VPath) -> Result<()> {
        self.denied()
    }
    fn set_metadata(
        &self,
        _p: &VPath,
        _mode: Option<u32>,
        _modified: Option<std::time::SystemTime>,
    ) -> Result<()> {
        self.denied()
    }
    fn capabilities(&self) -> Caps {
        Caps::default()
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
