use crate::error::{Error, Result};
use crate::ops::{JobHandle, ProgressEvent};
use crate::vfs::{Entry, EntryKind, VPath, Vfs};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, SystemTime};

/// Two modification times closer than this count as equal (FAT and ZIP store 2 s).
pub const TIME_TOLERANCE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareState {
    OnlyLeft,
    OnlyRight,
    Same,
    LeftNewer,
    RightNewer,
    /// Same time (within the tolerance) but a different size or content.
    Different,
    /// File, folder and link on the two sides do not match.
    TypeMismatch,
    /// Could not be examined (unreadable, budget used up, cancelled). Never treated as
    /// "empty" or "missing"; the result is marked `incomplete`.
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareMode {
    /// Size and modification time (2 s tolerance).
    SizeTime,
    /// Byte by byte; stops at the first difference.
    Content,
}

#[derive(Clone, Debug)]
pub struct CompareOptions {
    pub mode: CompareMode,
    pub recursive: bool,
    /// Most entries in the result; more makes the result incomplete.
    pub max_entries: usize,
    /// Deepest folder level that is listed (children of the roots are level 1).
    pub max_depth: usize,
    /// Most bytes read for content comparison (both sides counted).
    pub max_content_bytes: u64,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            mode: CompareMode::SizeTime,
            recursive: true,
            max_entries: 1_000_000,
            max_depth: 128,
            max_content_bytes: 16 * 1024 * 1024 * 1024,
        }
    }
}

/// What one side of a pair looked like in its listing.
#[derive(Clone, Debug, PartialEq)]
pub struct SideInfo {
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Link text for links (never followed).
    pub link_target: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompareEntry {
    /// Path relative to the compared roots, components joined with `/`. Empty for the
    /// root itself (only used when a root cannot be listed).
    pub rel: String,
    pub state: CompareState,
    pub left: Option<SideInfo>,
    pub right: Option<SideInfo>,
    /// Reason for `Unknown` (and other remarks).
    pub note: Option<String>,
    /// For folders: the content was listed and its entries are part of the result.
    pub listed: bool,
}

#[derive(Clone, Debug)]
pub struct CompareResult {
    pub left: VPath,
    pub right: VPath,
    pub mode: CompareMode,
    pub recursive: bool,
    /// Sorted: a folder comes before its content, siblings by name.
    pub entries: Vec<CompareEntry>,
    /// Something was not examined (unreadable, limit, cancelled). Never silent.
    pub incomplete: bool,
    pub cancelled: bool,
    pub issues: Vec<String>,
}

impl CompareResult {
    pub fn count(&self, state: CompareState) -> usize {
        self.entries.iter().filter(|e| e.state == state).count()
    }
}

struct Pending {
    rel: String,
    left: Option<VPath>,
    right: Option<VPath>,
    depth: usize,
    /// Index of the entry describing this folder (`None` for the roots).
    entry: Option<usize>,
}

struct Walker<'a> {
    lv: &'a dyn Vfs,
    rv: &'a dyn Vfs,
    opts: &'a CompareOptions,
    job: &'a JobHandle,
    entries: Vec<CompareEntry>,
    incomplete: bool,
    cancelled: bool,
    issues: Vec<String>,
    content_bytes: u64,
    budget_hit: bool,
}

fn side(entry: &Entry) -> SideInfo {
    SideInfo {
        kind: entry.kind,
        size: entry.size,
        modified: entry.modified,
        link_target: entry.link_target.clone(),
    }
}

/// `Some(true)`: left newer, `Some(false)`: right newer, `None`: within the tolerance
/// or a time is missing.
fn newer_side(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<bool> {
    let (l, r) = (left?, right?);
    match l.duration_since(r) {
        Ok(d) if d > TIME_TOLERANCE => Some(true),
        Ok(_) => None,
        Err(e) if e.duration() > TIME_TOLERANCE => Some(false),
        Err(_) => None,
    }
}

fn state_by_time(left: &Entry, right: &Entry) -> CompareState {
    match newer_side(left.modified, right.modified) {
        Some(true) => CompareState::LeftNewer,
        Some(false) => CompareState::RightNewer,
        None => CompareState::Different,
    }
}

fn read_full(reader: &mut dyn Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

impl Walker<'_> {
    fn issue(&mut self, text: String) {
        self.incomplete = true;
        if self.issues.len() < 100 {
            self.issues.push(text);
        }
    }

    fn push(&mut self, entry: CompareEntry) -> Option<usize> {
        if self.entries.len() >= self.opts.max_entries {
            if !self.budget_hit {
                self.budget_hit = true;
                self.issue("Die Obergrenze für Einträge ist erreicht".into());
            }
            return None;
        }
        self.entries.push(entry);
        Some(self.entries.len() - 1)
    }

    fn mark_unknown(&mut self, idx: Option<usize>, rel: &str, note: String) {
        self.issue(format!(
            "{}: {note}",
            if rel.is_empty() { "." } else { rel }
        ));
        match idx {
            Some(i) => {
                self.entries[i].state = CompareState::Unknown;
                self.entries[i].note = Some(note);
            }
            None => {
                self.push(CompareEntry {
                    rel: rel.to_string(),
                    state: CompareState::Unknown,
                    left: None,
                    right: None,
                    note: Some(note),
                    listed: false,
                });
            }
        }
    }

    /// Streams both files and compares them; `Ok(true)` if all bytes are equal.
    fn content_equal(&mut self, lp: &VPath, rp: &VPath, size: u64) -> Result<bool> {
        let cost = size.saturating_mul(2);
        if self.content_bytes.saturating_add(cost) > self.opts.max_content_bytes {
            return Err(Error::Unsupported(
                "Byte-Obergrenze für den Inhaltsvergleich",
            ));
        }
        // stat_file refuses pipes, devices and sockets without blocking.
        self.lv.stat_file(lp)?;
        self.rv.stat_file(rp)?;
        let mut a = self.lv.open_read(lp)?;
        let mut b = self.rv.open_read(rp)?;
        let io = |e: std::io::Error| Error::Io {
            op: "Datei lesen",
            path: lp.path().to_path_buf(),
            source: e,
        };
        let mut ba = vec![0u8; 128 * 1024];
        let mut bb = vec![0u8; 128 * 1024];
        loop {
            if self.job.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let na = read_full(&mut a, &mut ba).map_err(io)?;
            let nb = read_full(&mut b, &mut bb).map_err(io)?;
            self.content_bytes += (na + nb) as u64;
            if na != nb || ba[..na] != bb[..nb] {
                return Ok(false);
            }
            if na == 0 {
                return Ok(true);
            }
        }
    }

    fn compare_pair(
        &mut self,
        l: &Entry,
        r: &Entry,
        lp: &VPath,
        rp: &VPath,
    ) -> (CompareState, Option<String>) {
        if l.kind != r.kind {
            return (CompareState::TypeMismatch, None);
        }
        match l.kind {
            EntryKind::Dir => (CompareState::Same, None),
            EntryKind::Symlink => {
                let target = |entry: &Entry, vfs: &dyn Vfs, path: &VPath| {
                    entry
                        .link_target
                        .clone()
                        .map(Ok)
                        .unwrap_or_else(|| vfs.read_link(path))
                };
                match (target(l, self.lv, lp), target(r, self.rv, rp)) {
                    (Ok(a), Ok(b)) if a == b => (CompareState::Same, None),
                    (Ok(_), Ok(_)) => (CompareState::Different, None),
                    (Err(e), _) | (_, Err(e)) => (CompareState::Unknown, Some(e.to_string())),
                }
            }
            EntryKind::File => match self.opts.mode {
                CompareMode::SizeTime => {
                    if let Some(newer_left) = newer_side(l.modified, r.modified) {
                        let state = if newer_left {
                            CompareState::LeftNewer
                        } else {
                            CompareState::RightNewer
                        };
                        (state, None)
                    } else if l.size != r.size || l.modified.is_none() || r.modified.is_none() {
                        (CompareState::Different, None)
                    } else {
                        (CompareState::Same, None)
                    }
                }
                CompareMode::Content => {
                    if l.size != r.size {
                        return (state_by_time(l, r), None);
                    }
                    match self.content_equal(lp, rp, l.size) {
                        Ok(true) => (CompareState::Same, None),
                        Ok(false) => (state_by_time(l, r), None),
                        Err(Error::Cancelled) => {
                            self.cancelled = true;
                            (CompareState::Unknown, Some("abgebrochen".into()))
                        }
                        Err(e) => (CompareState::Unknown, Some(e.to_string())),
                    }
                }
            },
        }
    }

    fn list_side(vfs: &dyn Vfs, path: &Option<VPath>) -> std::result::Result<Vec<Entry>, String> {
        match path {
            None => Ok(Vec::new()),
            Some(p) => vfs.list(p).map_err(|e| e.to_string()),
        }
    }

    fn run(&mut self, left: &VPath, right: &VPath) {
        let mut stack = vec![Pending {
            rel: String::new(),
            left: Some(left.clone()),
            right: Some(right.clone()),
            depth: 0,
            entry: None,
        }];
        while let Some(dir) = stack.pop() {
            if self.budget_hit {
                break;
            }
            if self.job.is_cancelled() || self.cancelled {
                self.cancelled = true;
                self.issue("Der Vergleich wurde abgebrochen".into());
                break;
            }
            let left_list = Self::list_side(self.lv, &dir.left);
            let right_list = Self::list_side(self.rv, &dir.right);
            let (left_entries, right_entries) = match (left_list, right_list) {
                (Ok(l), Ok(r)) => (l, r),
                (Err(e), _) | (_, Err(e)) => {
                    // Never treated as an empty folder: the whole subtree is unknown.
                    self.mark_unknown(dir.entry, &dir.rel, format!("Ordner nicht lesbar: {e}"));
                    continue;
                }
            };
            if let Some(i) = dir.entry {
                self.entries[i].listed = true;
            }
            if let Some(path) = dir.left.as_ref().or(dir.right.as_ref()) {
                self.job.emit(ProgressEvent::ItemDone(path.clone()));
            }
            let mut pairs: BTreeMap<String, (Option<Entry>, Option<Entry>)> = BTreeMap::new();
            let mut nameless_error = None;
            for (is_left, list) in [(true, left_entries), (false, right_entries)] {
                for entry in list {
                    if entry.name.is_empty() {
                        nameless_error = Some(entry.error.unwrap_or_default());
                        continue;
                    }
                    let slot = pairs.entry(entry.name.clone()).or_default();
                    if is_left {
                        slot.0 = Some(entry);
                    } else {
                        slot.1 = Some(entry);
                    }
                }
            }
            if let Some(error) = nameless_error {
                // An entry without a name could not be read at all: unknown content.
                self.mark_unknown(
                    dir.entry,
                    &dir.rel,
                    format!("Ein Eintrag war nicht lesbar: {error}"),
                );
                continue;
            }
            let mut children = Vec::new();
            for (name, (l, r)) in pairs {
                if self.budget_hit {
                    break;
                }
                let rel = if dir.rel.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{name}", dir.rel)
                };
                let lp = match &dir.left {
                    Some(base) => l.as_ref().and_then(|_| base.join_entry(&name).ok()),
                    None => None,
                };
                let rp = match &dir.right {
                    Some(base) => r.as_ref().and_then(|_| base.join_entry(&name).ok()),
                    None => None,
                };
                if (l.is_some() && lp.is_none()) || (r.is_some() && rp.is_none()) {
                    self.mark_unknown(None, &rel, "Ungültiger Name".into());
                    continue;
                }
                let broken = l
                    .as_ref()
                    .and_then(|e| e.error.clone())
                    .or_else(|| r.as_ref().and_then(|e| e.error.clone()));
                let (state, note) = if let Some(error) = broken {
                    (CompareState::Unknown, Some(error))
                } else {
                    match (&l, &r, &lp, &rp) {
                        (Some(_), None, ..) => (CompareState::OnlyLeft, None),
                        (None, Some(_), ..) => (CompareState::OnlyRight, None),
                        (Some(le), Some(re), Some(lp), Some(rp)) => {
                            self.compare_pair(le, re, lp, rp)
                        }
                        _ => continue,
                    }
                };
                if state == CompareState::Unknown {
                    self.issue(format!("{rel}: {}", note.as_deref().unwrap_or("unbekannt")));
                }
                let descend = matches!(
                    (&l, &r),
                    (Some(a), Some(b)) if a.kind == EntryKind::Dir && b.kind == EntryKind::Dir
                ) || matches!((&l, &r), (Some(a), None) | (None, Some(a)) if a.kind == EntryKind::Dir);
                let descend = descend && state != CompareState::Unknown;
                let Some(index) = self.push(CompareEntry {
                    rel: rel.clone(),
                    state,
                    left: l.as_ref().map(side),
                    right: r.as_ref().map(side),
                    note,
                    listed: false,
                }) else {
                    break;
                };
                if descend && self.opts.recursive {
                    if dir.depth + 1 > self.opts.max_depth {
                        self.issue(format!("{rel}: Tiefengrenze erreicht"));
                    } else {
                        children.push(Pending {
                            rel,
                            left: lp,
                            right: rp,
                            depth: dir.depth + 1,
                            entry: Some(index),
                        });
                    }
                }
            }
            stack.extend(children.into_iter().rev());
        }
        if self.budget_hit || self.cancelled {
            self.incomplete = true;
        }
    }
}

/// Compares two folders (each through its own `Vfs`), recursively through `Vfs::list`
/// only. Links are compared as links, never followed. Errors, limits and cancellation
/// make the result `incomplete`; unreadable parts are `Unknown`, never "empty".
pub fn compare_dirs(
    left_vfs: &dyn Vfs,
    left: &VPath,
    right_vfs: &dyn Vfs,
    right: &VPath,
    opts: &CompareOptions,
    job: &JobHandle,
) -> CompareResult {
    let mut walker = Walker {
        lv: left_vfs,
        rv: right_vfs,
        opts,
        job,
        entries: Vec::new(),
        incomplete: false,
        cancelled: false,
        issues: Vec::new(),
        content_bytes: 0,
        budget_hit: false,
    };
    walker.run(left, right);
    let mut entries = walker.entries;
    entries.sort_by(|a, b| a.rel.split('/').cmp(b.rel.split('/')));
    CompareResult {
        left: left.clone(),
        right: right.clone(),
        mode: opts.mode,
        recursive: opts.recursive,
        entries,
        incomplete: walker.incomplete,
        cancelled: walker.cancelled,
        issues: walker.issues,
    }
}
