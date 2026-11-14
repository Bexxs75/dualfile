//! The in-memory index of an archive: limits, nodes, and the builder that classifies every
//! entry path (safe or not) and enforces the entry/name/depth/memory limits while parsing.

use super::budget::{Budget, RarConfig, MSG_INDEX_BYTES};
use super::{sevenz, tarfmt, zipfmt};
use crate::error::{Error, Result};
use crate::vfs::{EntryKind, MAX_ARCHIVE_NAME_LEN, MAX_ARCHIVE_PATH_DEPTH};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// All limits of the archive layer. `Default` holds the production values; tests lower them.
#[derive(Clone, Debug)]
pub struct ArchiveLimits {
    pub max_entries: usize,
    pub max_name_len: usize,
    pub max_depth: usize,
    /// Estimated memory of the whole index.
    pub index_memory: u64,
    /// Name plus link target of a single entry.
    pub max_entry_meta: usize,
    /// Decompressed bytes that indexing a TAR.* may read (shared over nesting levels).
    pub index_bytes_tar: u64,
    /// Bytes of directory structures that indexing a ZIP/7Z may read.
    pub index_dir_bytes: u64,
    pub index_time: Duration,
    /// Largest archive that is read into memory from inside another archive.
    pub nested_max_bytes: u64,
    /// Compression ratio guard: more than `ratio_max`:1 once `ratio_min_bytes` are produced.
    pub ratio_min_bytes: u64,
    pub ratio_max: u64,
    pub max_file_bytes: u64,
    /// Total decompressed bytes of one job (copy, search).
    pub job_bytes: u64,
    /// Per read operation (`read_range`).
    pub read_bytes: u64,
    pub read_time: Duration,
    pub rar: RarConfig,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 500_000,
            max_name_len: MAX_ARCHIVE_NAME_LEN,
            max_depth: MAX_ARCHIVE_PATH_DEPTH,
            index_memory: 256 * MIB,
            max_entry_meta: 64 * 1024,
            index_bytes_tar: 4 * GIB,
            index_dir_bytes: 256 * MIB,
            index_time: Duration::from_secs(30),
            nested_max_bytes: 256 * MIB,
            ratio_min_bytes: 100 * MIB,
            ratio_max: 1000,
            max_file_bytes: 10 * GIB,
            job_bytes: 20 * GIB,
            read_bytes: 8 * GIB,
            read_time: Duration::from_secs(120),
            rar: RarConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flag {
    Ok,
    /// Path could leave the archive; never readable.
    Unsafe(&'static str),
    /// Shown only (special file, encrypted, unsupported method).
    Unreadable(&'static str),
}

#[derive(Clone, Debug)]
pub(crate) enum Loc {
    None,
    Zip(zipfmt::ZipLoc),
    Tar(tarfmt::TarLoc),
    SevenZ(sevenz::SevenLoc),
    Rar(String),
    /// The member's path as the plugin named it.
    External(String),
}

pub(crate) struct Node {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
    pub flag: Flag,
    pub loc: Loc,
    pub children: BTreeMap<String, usize>,
}

impl Node {
    fn dir(name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: EntryKind::Dir,
            size: 0,
            modified: None,
            mode: None,
            link_target: None,
            flag: Flag::Ok,
            loc: Loc::None,
            children: BTreeMap::new(),
        }
    }
    fn footprint(&self) -> u64 {
        (std::mem::size_of::<Node>() + self.name.len() * 2 + 48) as u64
            + self.link_target.as_ref().map_or(0, |t| t.len() as u64)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RawKind {
    File,
    Dir,
    Symlink,
    /// Device, FIFO, socket, hard link ...: shown only, with a short label.
    Special(&'static str),
}

/// One entry as a format parser reports it, before path classification.
pub(crate) struct RawEntry {
    pub path: String,
    pub kind: RawKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
    pub loc: Loc,
    /// Set for entries that cannot be read (encrypted, unsupported method).
    pub unreadable: Option<&'static str>,
}

pub(crate) struct IndexBuilder {
    nodes: Vec<Node>,
    limits: ArchiveLimits,
    budget: Budget,
    seen: usize,
    memory: u64,
    unsafe_count: usize,
}

const MSG_NAME_TOO_LONG: &str = "Archiv enthält zu lange Namen";
const MSG_TOO_DEEP: &str = "Archiv enthält zu tief verschachtelte Pfade";

/// Why a raw entry path is not safe, or the components of a safe one.
fn classify(path: &str) -> std::result::Result<Vec<&str>, &'static str> {
    if path.contains('\0') {
        return Err("Unsicherer Eintrag: NUL im Pfad");
    }
    if path.contains('\\') {
        return Err("Unsicherer Eintrag: Backslash im Pfad");
    }
    if path.starts_with('/') {
        return Err("Unsicherer Eintrag: absoluter Pfad");
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("Unsicherer Eintrag: Pfad verlässt das Archiv"),
            part => parts.push(part),
        }
    }
    if let Some(first) = parts.first() {
        let bytes = first.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return Err("Unsicherer Eintrag: Laufwerksangabe im Pfad");
        }
    }
    Ok(parts)
}

fn sanitize_for_display(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(120)
        .collect()
}

impl IndexBuilder {
    pub(crate) fn new(limits: &ArchiveLimits, budget: Budget) -> Self {
        Self {
            nodes: vec![Node::dir("")],
            limits: limits.clone(),
            budget,
            seen: 0,
            memory: 0,
            unsafe_count: 0,
        }
    }

    pub(crate) fn finish(self) -> Vec<Node> {
        self.nodes
    }

    fn account(&mut self, node: &Node) -> Result<()> {
        self.memory = self.memory.saturating_add(node.footprint());
        if self.memory > self.limits.index_memory {
            return Err(Error::Archive(MSG_INDEX_BYTES));
        }
        Ok(())
    }

    fn push_child(&mut self, parent: usize, node: Node) -> Result<usize> {
        self.account(&node)?;
        let name = node.name.clone();
        self.nodes.push(node);
        let index = self.nodes.len() - 1;
        self.nodes[parent].children.insert(name, index);
        Ok(index)
    }

    fn add_unsafe(&mut self, raw: &RawEntry, reason: &'static str) -> Result<()> {
        self.unsafe_count += 1;
        let mut name = format!(
            "[unsicher {}] {}",
            self.unsafe_count,
            sanitize_for_display(&raw.path)
        );
        while self.nodes[0].children.contains_key(&name) {
            name.push('_');
        }
        let node = Node {
            name,
            kind: EntryKind::File,
            size: 0,
            modified: None,
            mode: None,
            link_target: None,
            flag: Flag::Unsafe(reason),
            loc: Loc::None,
            children: BTreeMap::new(),
        };
        self.push_child(0, node).map(|_| ())
    }

    /// Adds one parsed entry. Checks time, entry count and size limits first; aborts the
    /// whole parse with an error when one is exceeded.
    pub(crate) fn add(&mut self, raw: RawEntry) -> Result<()> {
        self.budget
            .check_time()
            .map_err(|_| Error::Archive(super::budget::MSG_INDEX_TIME))?;
        self.seen += 1;
        if self.seen > self.limits.max_entries {
            return Err(Error::Archive(MSG_INDEX_BYTES));
        }
        let meta = raw.path.len() + raw.link_target.as_ref().map_or(0, String::len);
        if meta > self.limits.max_entry_meta {
            return Err(Error::Archive(MSG_INDEX_BYTES));
        }
        let parts = match classify(&raw.path) {
            Ok(parts) => parts,
            Err(reason) => return self.add_unsafe(&raw, reason),
        };
        if parts.is_empty() {
            return Ok(()); // the archive root itself ("./")
        }
        if parts.len() > self.limits.max_depth {
            return Err(Error::Archive(MSG_TOO_DEEP));
        }
        if parts
            .iter()
            .any(|part| part.len() > self.limits.max_name_len)
        {
            return Err(Error::Archive(MSG_NAME_TOO_LONG));
        }
        let (last, parents) = parts.split_last().expect("not empty");
        let mut current = 0;
        for part in parents {
            match self.nodes[current].children.get(*part).copied() {
                Some(child) if self.nodes[child].kind == EntryKind::Dir => current = child,
                Some(_) => {
                    return self.add_unsafe(&raw, "Unsicherer Eintrag: Elternpfad ist kein Ordner")
                }
                None => current = self.push_child(current, Node::dir(part))?,
            }
        }
        let (kind, flag) = match raw.kind {
            RawKind::File => (EntryKind::File, Flag::Ok),
            RawKind::Dir => (EntryKind::Dir, Flag::Ok),
            RawKind::Symlink => (EntryKind::Symlink, Flag::Ok),
            RawKind::Special(label) => (EntryKind::File, Flag::Unreadable(label)),
        };
        let flag = match raw.unreadable {
            Some(reason) => Flag::Unreadable(reason),
            None => flag,
        };
        let size = if kind == EntryKind::File && matches!(flag, Flag::Ok) {
            raw.size
        } else {
            0
        };
        let node = Node {
            name: (*last).to_string(),
            kind,
            size,
            modified: raw.modified,
            // setuid/setgid/sticky are never taken over.
            mode: raw.mode.map(|mode| mode & 0o777),
            link_target: raw.link_target,
            flag,
            loc: raw.loc,
            children: BTreeMap::new(),
        };
        match self.nodes[current].children.get(*last).copied() {
            Some(existing) if self.nodes[existing].kind == EntryKind::Dir => {
                if kind == EntryKind::Dir {
                    let target = &mut self.nodes[existing];
                    target.mode = node.mode.or(target.mode);
                    target.modified = node.modified.or(target.modified);
                    Ok(())
                } else {
                    self.add_unsafe(
                        &raw_path_only(&raw.path),
                        "Unsicherer Eintrag: Name kollidiert mit einem Ordner",
                    )
                }
            }
            Some(existing) => {
                self.account(&node)?;
                self.nodes[existing] = node;
                Ok(())
            }
            None => self.push_child(current, node).map(|_| ()),
        }
    }
}

fn raw_path_only(path: &str) -> RawEntry {
    RawEntry {
        path: path.to_string(),
        kind: RawKind::File,
        size: 0,
        modified: None,
        mode: None,
        link_target: None,
        loc: Loc::None,
        unreadable: None,
    }
}
