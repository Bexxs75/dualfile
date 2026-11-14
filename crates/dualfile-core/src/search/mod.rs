//! Search over many files and archives: file name (glob or regular expression), text in
//! files (literal or regular expression), size and date filters.
//!
//! Safety rules: the search only reads. Links are never followed (so a link cycle cannot
//! make it loop), pipes/devices/sockets are refused by `Vfs::stat_file` before a file is
//! opened (no blocking), archives are only entered within the existing archive budgets
//! (entries, depth, ratio, decompressed bytes, time), server and plugin locations only when
//! the caller asks for them explicitly and then with a small budget. Regular expressions
//! run on the `regex` crate (no backtracking), pattern length and compiled size are
//! limited. Nothing is cut off silently: every limit, read error or cancellation makes the
//! result `incomplete` and says why.

use crate::archive::{ArchiveLimits, ArchiveVfs, Budget};
use crate::error::{Error, Result};
use crate::ops::{JobHandle, ProgressEvent};
use crate::router::VfsRouter;
use crate::vfs::{format_ipc_path, Entry, EntryKind, VPath, Vfs, MAX_ARCHIVE_NESTING};
use regex::{Regex, RegexBuilder};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
mod tests;

/// Longest name pattern or text pattern in bytes.
pub const MAX_PATTERN_BYTES: usize = 1024;
/// Compiled size limit of a regular expression (memory of the program and of the lazy DFA).
const REGEX_SIZE_LIMIT: usize = 4 << 20;
const REGEX_NEST_LIMIT: u32 = 64;
/// A file is treated as binary when its first bytes contain a NUL.
const BINARY_PROBE: usize = 8 << 10;
/// Longest line that is matched; the rest of a longer line is not searched (reported).
const MAX_LINE_BYTES: usize = 64 << 10;
/// Longest excerpt of a matching line in characters.
pub const MAX_EXCERPT_CHARS: usize = 200;
/// Characters of context kept before a match when the line has to be cut.
const EXCERPT_LEAD: usize = 40;
const READ_CHUNK: usize = 64 << 10;
/// Most issue texts kept (the result is incomplete anyway).
const MAX_ISSUES: usize = 100;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NameMode {
    /// `*`, `?`, `[abc]`, `[!a-z]`; several patterns separated by `;`. Matches the whole name.
    #[default]
    Glob,
    /// Regular expression; matches anywhere in the name (anchor with `^`/`$`).
    Regex,
}

#[derive(Clone, Debug)]
pub struct SearchLimits {
    /// Most hits kept; more makes the result incomplete.
    pub max_hits: usize,
    /// Most hits of one file.
    pub max_hits_per_file: usize,
    /// Most files looked at (every listed file counts).
    pub max_files: u64,
    /// Most bytes read for text search (local and decompressed bytes).
    pub max_bytes: u64,
    /// Most bytes of one file that are searched.
    pub max_file_bytes: u64,
    pub max_time: Option<Duration>,
    /// Deepest folder level (children of the start folder are level 1).
    pub max_depth: usize,
    /// Decompressed bytes all archives of one search may produce together.
    pub archive_bytes: u64,
    /// Time all archive reads of one search may take together.
    pub archive_time: Duration,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_hits: 10_000,
            max_hits_per_file: 1_000,
            max_files: 1_000_000,
            max_bytes: 8 << 30,
            max_file_bytes: 64 << 20,
            max_time: Some(Duration::from_secs(600)),
            max_depth: 128,
            archive_bytes: 2 << 30,
            archive_time: Duration::from_secs(120),
        }
    }
}

impl SearchLimits {
    /// The (low) budget for servers and plugins: whatever the caller asked for is cut down
    /// to this.
    fn for_remote(&self) -> Self {
        Self {
            max_hits: self.max_hits.min(1_000),
            max_hits_per_file: self.max_hits_per_file.min(100),
            max_files: self.max_files.min(2_000),
            max_bytes: self.max_bytes.min(REMOTE_MAX_BYTES),
            max_file_bytes: self.max_file_bytes.min(REMOTE_MAX_FILE_BYTES),
            max_time: Some(
                self.max_time
                    .map_or(REMOTE_MAX_TIME, |t| t.min(REMOTE_MAX_TIME)),
            ),
            max_depth: self.max_depth.min(16),
            archive_bytes: 0,
            archive_time: Duration::ZERO,
        }
    }
}

/// Server and plugin budget: files, bytes, time.
pub const REMOTE_MAX_BYTES: u64 = 16 << 20;
pub const REMOTE_MAX_FILE_BYTES: u64 = 1 << 20;
pub const REMOTE_MAX_TIME: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct SearchOptions {
    /// Name filter; `None` or empty: every name.
    pub name: Option<String>,
    pub name_mode: NameMode,
    pub name_case_sensitive: bool,
    /// Text to find in files; `None` or empty: only names are searched.
    pub text: Option<String>,
    /// The text is a regular expression (otherwise a literal).
    pub text_regex: bool,
    pub text_case_sensitive: bool,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Files without a modification time never match a date filter.
    pub modified_after: Option<SystemTime>,
    pub modified_before: Option<SystemTime>,
    /// Descend into sub folders.
    pub recursive: bool,
    /// Look into archive files (as folders).
    pub in_archives: bool,
    /// Archive levels looked into (an archive inside an archive is level 2); 1 to 3.
    pub archive_depth: usize,
    /// Servers and plugin file systems are only searched when this is set (and then with
    /// `SearchLimits::for_remote`).
    pub allow_remote: bool,
    pub limits: SearchLimits,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            name: None,
            name_mode: NameMode::Glob,
            name_case_sensitive: false,
            text: None,
            text_regex: false,
            text_case_sensitive: false,
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
            recursive: true,
            in_archives: false,
            archive_depth: 1,
            allow_remote: false,
            limits: SearchLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub path: VPath,
    pub kind: EntryKind,
    pub size: u64,
    /// Text hits: 1-based line number and a shortened extract of the line (plain text,
    /// control characters replaced). `None` for name hits.
    pub line: Option<u64>,
    pub excerpt: Option<String>,
    /// Position and length (in characters) of the match inside `excerpt`.
    pub match_start: u32,
    pub match_len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    Cancelled,
    Hits,
    Files,
    Bytes,
    Time,
}

impl StopReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Die Suche wurde abgebrochen",
            Self::Hits => "Die Obergrenze für Treffer ist erreicht",
            Self::Files => "Die Obergrenze für Dateien ist erreicht",
            Self::Bytes => "Die Obergrenze für gelesene Bytes ist erreicht",
            Self::Time => "Die Zeitgrenze ist erreicht",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub root: VPath,
    pub hits: Vec<SearchHit>,
    pub dirs_visited: u64,
    /// Files and other non-folder entries looked at.
    pub files_visited: u64,
    /// Files whose content was read (not binary, not refused).
    pub files_searched: u64,
    pub archives_searched: u64,
    pub skipped_binary: u64,
    /// Pipes, devices and sockets that were not read.
    pub skipped_special: u64,
    pub bytes_read: u64,
    /// Something was not examined (limit, read error, cancelled). Never silent.
    pub incomplete: bool,
    pub cancelled: bool,
    /// The limit or event that ended the search early.
    pub stopped: Option<StopReason>,
    pub issues: Vec<String>,
}

impl SearchResult {
    /// Number of hits (the result holds all of them: one more is never dropped silently).
    pub fn total(&self) -> u64 {
        self.hits.len() as u64
    }
}

// ---------------------------------------------------------------- Patterns

fn compile(pattern: &str, case_sensitive: bool) -> Result<Regex> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(Error::InvalidInput("Das Suchmuster ist zu lang"));
    }
    RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .nest_limit(REGEX_NEST_LIMIT)
        .build()
        .map_err(|_| Error::InvalidInput("Ungültiger regulärer Ausdruck"))
}

/// Translates one glob into regex syntax (without anchors).
fn glob_body(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '\\' if i + 1 < chars.len() => {
                i += 1;
                out.push_str(&regex::escape(&chars[i].to_string()));
            }
            '[' => {
                // A class needs a closing `]` (a first `]` is a member); otherwise literal.
                let mut j = i + 1;
                let negated = matches!(chars.get(j), Some('!' | '^'));
                if negated {
                    j += 1;
                }
                let body_start = j;
                if chars.get(j) == Some(&']') {
                    j += 1;
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    out.push_str("\\[");
                } else {
                    out.push('[');
                    if negated {
                        out.push('^');
                    }
                    for &c in &chars[body_start..j] {
                        if matches!(c, '\\' | '[' | ']' | '&' | '~' | '^') {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push(']');
                    i = j;
                }
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out
}

fn compile_name(options: &SearchOptions) -> Result<Option<Regex>> {
    let Some(pattern) = options.name.as_deref().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    if pattern.contains('\0') {
        return Err(Error::InvalidInput("Das Suchmuster ist ungültig"));
    }
    let source = match options.name_mode {
        NameMode::Regex => pattern.to_owned(),
        NameMode::Glob => {
            if pattern.len() > MAX_PATTERN_BYTES {
                return Err(Error::InvalidInput("Das Suchmuster ist zu lang"));
            }
            let alternatives: Vec<String> = pattern
                .split(';')
                .filter(|part| !part.is_empty())
                .map(glob_body)
                .collect();
            if alternatives.is_empty() {
                return Ok(None);
            }
            format!("(?s)^(?:{})$", alternatives.join("|"))
        }
    };
    compile(&source, options.name_case_sensitive).map(Some)
}

fn compile_text(options: &SearchOptions) -> Result<Option<Regex>> {
    let Some(text) = options.text.as_deref().filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.contains('\0') {
        return Err(Error::InvalidInput("Der Suchtext ist ungültig"));
    }
    if text.len() > MAX_PATTERN_BYTES {
        return Err(Error::InvalidInput("Das Suchmuster ist zu lang"));
    }
    let source = if options.text_regex {
        text.to_owned()
    } else {
        regex::escape(text)
    };
    compile(&source, options.text_case_sensitive).map(Some)
}

/// Archive file name extensions that are tried as archives.
fn looks_like_archive(name: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".zip", ".jar", ".tar", ".tgz", ".gz", ".tbz", ".tbz2", ".bz2", ".txz", ".xz", ".7z",
        ".rar", ".cbz", ".cbr",
    ];
    let lower = name.to_lowercase();
    EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// The text of an error for the issue list; like the UI errors it leaves out the OS error
/// text of an I/O failure.
fn describe(error: &Error) -> String {
    match error {
        Error::Io { op, path, .. } => format!("{op} fehlgeschlagen für {}", path.display()),
        other => other.to_string(),
    }
}

fn display(path: &VPath) -> String {
    format_ipc_path(path)
}

/// The extract shown for a matching line: control characters replaced, cut to
/// `MAX_EXCERPT_CHARS` around the match. Returns the text and the position and length of
/// the match in it (characters).
fn excerpt(line: &str, match_start_byte: usize, match_end_byte: usize) -> (String, u32, u32) {
    let chars: Vec<char> = line.chars().collect();
    let start_char = line[..match_start_byte].chars().count();
    let len_chars = line[match_start_byte..match_end_byte].chars().count();
    let from = if chars.len() <= MAX_EXCERPT_CHARS || start_char <= EXCERPT_LEAD {
        0
    } else {
        start_char - EXCERPT_LEAD
    };
    let to = (from + MAX_EXCERPT_CHARS).min(chars.len());
    let mut text = String::new();
    if from > 0 {
        text.push('…');
    }
    let prefix = text.chars().count();
    text.extend(
        chars[from..to]
            .iter()
            .map(|c| if c.is_control() { ' ' } else { *c }),
    );
    if to < chars.len() {
        text.push('…');
    }
    let rel = start_char - from;
    let shown_len = len_chars.min(to.saturating_sub(from + rel));
    (text, (prefix + rel) as u32, shown_len as u32)
}

// ---------------------------------------------------------------- The search

struct Searcher<'a> {
    router: &'a VfsRouter,
    opts: &'a SearchOptions,
    limits: SearchLimits,
    name_re: Option<Regex>,
    text_re: Option<Regex>,
    job: &'a JobHandle,
    started: Instant,
    archive_budget: Budget,
    archives: Vec<Arc<ArchiveVfs>>,
    archive_levels: usize,
    archive_exhausted: bool,
    result: SearchResult,
}

enum FileEnd {
    Done,
    /// Stop everything (limit hit or cancelled; the reason is set already).
    Stop,
}

impl Searcher<'_> {
    fn issue(&mut self, text: String) {
        self.result.incomplete = true;
        if self.result.issues.len() < MAX_ISSUES {
            self.result.issues.push(text);
        }
    }

    fn stop(&mut self, reason: StopReason) {
        if self.result.stopped.is_none() {
            self.result.stopped = Some(reason);
            self.result.incomplete = true;
            if reason == StopReason::Cancelled {
                self.result.cancelled = true;
            }
            self.issue(reason.message().into());
        }
    }

    /// True once the search must end (limit, time, cancel).
    fn should_stop(&mut self) -> bool {
        if self.result.stopped.is_some() {
            return true;
        }
        if self.job.is_cancelled() {
            self.stop(StopReason::Cancelled);
        } else if self
            .limits
            .max_time
            .is_some_and(|max| self.started.elapsed() >= max)
        {
            self.stop(StopReason::Time);
        }
        self.result.stopped.is_some()
    }

    fn name_matches(&self, name: &str) -> bool {
        self.name_re.as_ref().is_none_or(|re| re.is_match(name))
    }

    fn has_attribute_filter(&self) -> bool {
        let o = self.opts;
        o.min_size.is_some()
            || o.max_size.is_some()
            || o.modified_after.is_some()
            || o.modified_before.is_some()
    }

    fn file_passes_filters(&self, entry: &Entry) -> bool {
        let o = self.opts;
        if o.min_size.is_some_and(|min| entry.size < min)
            || o.max_size.is_some_and(|max| entry.size > max)
        {
            return false;
        }
        if o.modified_after.is_some() || o.modified_before.is_some() {
            let Some(time) = entry.modified else {
                return false;
            };
            if o.modified_after.is_some_and(|after| time < after)
                || o.modified_before.is_some_and(|before| time > before)
            {
                return false;
            }
        }
        true
    }

    /// Keeps the hit; at the limit the search ends (a hit beyond it is what makes the
    /// result incomplete, so exactly `max_hits` hits stay complete).
    fn push_hit(&mut self, hit: SearchHit) -> bool {
        if self.result.hits.len() >= self.limits.max_hits {
            self.stop(StopReason::Hits);
            return false;
        }
        self.result.hits.push(hit);
        true
    }

    fn name_hit(&mut self, path: VPath, entry: &Entry) {
        let _ = self.push_hit(SearchHit {
            path,
            kind: entry.kind,
            size: entry.size,
            line: None,
            excerpt: None,
            match_start: 0,
            match_len: 0,
        });
    }

    /// Walks the folder tree below `start` through `vfs`. `level` counts the archive levels
    /// between the start of the search and `vfs`.
    fn walk(&mut self, vfs: &dyn Vfs, start: VPath, level: usize) {
        let mut stack = vec![(start, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if self.should_stop() {
                return;
            }
            let mut entries = match vfs.list(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    self.issue(format!("{}: {}", display(&dir), describe(&error)));
                    continue;
                }
            };
            self.result.dirs_visited += 1;
            self.job.emit(ProgressEvent::ItemDone(dir.clone()));
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            let mut subdirs = Vec::new();
            for entry in entries {
                if self.should_stop() {
                    return;
                }
                if entry.name.is_empty() {
                    self.issue(format!(
                        "{}: Ein Eintrag war nicht lesbar: {}",
                        display(&dir),
                        entry.error.as_deref().unwrap_or("unbekannt")
                    ));
                    continue;
                }
                let Ok(path) = dir.join_entry(&entry.name) else {
                    self.issue(format!("{}: Ungültiger Name", display(&dir)));
                    continue;
                };
                if let Some(error) = &entry.error {
                    self.issue(format!("{}: {error}", display(&path)));
                    continue;
                }
                match entry.kind {
                    EntryKind::Dir => {
                        // Folder names are hits for a pure name search only.
                        if self.text_re.is_none()
                            && !self.has_attribute_filter()
                            && self.name_re.is_some()
                            && self.name_matches(&entry.name)
                        {
                            self.name_hit(path.clone(), &entry);
                        }
                        if self.opts.recursive {
                            if depth + 1 > self.limits.max_depth {
                                self.issue(format!("{}: Tiefengrenze erreicht", display(&path)));
                            } else {
                                subdirs.push((path, depth + 1));
                            }
                        }
                    }
                    EntryKind::Symlink => {
                        // Never followed or read; its name can still match.
                        if self.text_re.is_none()
                            && !self.has_attribute_filter()
                            && self.name_re.is_some()
                            && self.name_matches(&entry.name)
                        {
                            self.name_hit(path, &entry);
                        }
                    }
                    EntryKind::File => {
                        if let FileEnd::Stop = self.file(vfs, &entry, path, level) {
                            return;
                        }
                    }
                }
            }
            stack.extend(subdirs.into_iter().rev());
        }
    }

    fn file(&mut self, vfs: &dyn Vfs, entry: &Entry, path: VPath, level: usize) -> FileEnd {
        self.result.files_visited += 1;
        if self.result.files_visited > self.limits.max_files {
            self.result.files_visited -= 1;
            self.stop(StopReason::Files);
            return FileEnd::Stop;
        }
        let mut opened_as_archive = false;
        if self.opts.in_archives
            && level < self.archive_levels
            && !path.scheme().is_remote()
            && looks_like_archive(&entry.name)
        {
            opened_as_archive = self.archive(&path, level + 1);
            if self.result.stopped.is_some() {
                return FileEnd::Stop;
            }
        }
        if !self.name_matches(&entry.name) || !self.file_passes_filters(entry) {
            return FileEnd::Done;
        }
        if self.text_re.is_none() {
            self.name_hit(path, entry);
        } else if !opened_as_archive {
            return self.text(vfs, entry, &path);
        }
        FileEnd::Done
    }

    /// Searches the content of the archive file `file`. `false` if it is no archive (it is
    /// then handled as an ordinary file).
    fn archive(&mut self, file: &VPath, level: usize) -> bool {
        if self.archive_exhausted {
            return false;
        }
        let index_budget = ArchiveVfs::new_index_budget(&ArchiveLimits::default());
        let archive = match self.router.archive_for(file, &index_budget) {
            Ok(archive) => archive,
            Err(Error::Archive(message))
                if message == crate::archive::MSG_NOT_AN_ARCHIVE
                    || message == crate::archive::MSG_CORRUPT =>
            {
                if message == crate::archive::MSG_CORRUPT {
                    self.issue(format!("{}: {message}", display(file)));
                }
                return false;
            }
            Err(Error::Cancelled) => {
                self.stop(StopReason::Cancelled);
                return false;
            }
            Err(error) => {
                self.issue(format!("{}: {}", display(file), describe(&error)));
                return false;
            }
        };
        archive.set_job_budget(self.archive_budget.clone());
        self.archives.push(Arc::clone(&archive));
        self.result.archives_searched += 1;
        self.walk(&*archive, archive.root(), level);
        true
    }

    fn archive_budget_spent(&self) -> bool {
        self.archive_budget.check_time().is_err()
            || self.archive_budget.used_bytes() >= self.limits.archive_bytes
    }

    /// Reads one file line by line and records the lines that match.
    fn text(&mut self, vfs: &dyn Vfs, entry: &Entry, path: &VPath) -> FileEnd {
        let in_archive = path.outer().is_some();
        if in_archive && self.archive_exhausted {
            return FileEnd::Done;
        }
        // Refuses pipes, devices and sockets without blocking, before anything is opened.
        if let Err(error) = vfs.stat_file(path) {
            match error {
                Error::Unsupported(_) => self.result.skipped_special += 1,
                other => self.issue(format!("{}: {}", display(path), describe(&other))),
            }
            return FileEnd::Done;
        }
        if entry.size == 0 {
            return FileEnd::Done;
        }
        let remaining = self.limits.max_bytes.saturating_sub(self.result.bytes_read);
        if remaining == 0 {
            self.stop(StopReason::Bytes);
            return FileEnd::Stop;
        }
        let limit = remaining.min(self.limits.max_file_bytes);
        let mut reader = match vfs.open_read(path) {
            Ok(reader) => reader,
            Err(error) => {
                self.issue(format!("{}: {}", display(path), describe(&error)));
                return FileEnd::Done;
            }
        };
        match self.scan(&mut *reader, entry, path, limit, remaining) {
            Ok(()) => {}
            Err(error) => {
                let error = match error {
                    Error::Io { source, .. } if in_archive => {
                        crate::archive::map_io(source, path.path())
                    }
                    other => other,
                };
                if in_archive && self.archive_budget_spent() {
                    self.archive_exhausted = true;
                    self.issue(format!(
                        "{}: Das Archiv-Budget ist aufgebraucht; weitere Archive werden übersprungen",
                        display(path)
                    ));
                } else if !matches!(error, Error::Cancelled) {
                    self.issue(format!("{}: {}", display(path), describe(&error)));
                }
            }
        }
        if self.result.stopped.is_some() {
            FileEnd::Stop
        } else {
            FileEnd::Done
        }
    }

    fn scan(
        &mut self,
        reader: &mut dyn Read,
        entry: &Entry,
        path: &VPath,
        limit: u64,
        remaining: u64,
    ) -> Result<()> {
        let io = |source: std::io::Error| Error::Io {
            op: "Datei lesen",
            path: path.path().to_path_buf(),
            source,
        };
        let mut buffer = vec![0u8; READ_CHUNK];
        let mut file_bytes = 0u64;
        let mut lines = LineState::default();
        let mut hits_in_file = 0usize;
        let mut first = true;
        loop {
            if self.job.is_cancelled() {
                self.stop(StopReason::Cancelled);
                return Ok(());
            }
            if self
                .limits
                .max_time
                .is_some_and(|max| self.started.elapsed() >= max)
            {
                self.stop(StopReason::Time);
                return Ok(());
            }
            let want = if first { BINARY_PROBE } else { READ_CHUNK };
            let want = (want as u64).min(limit - file_bytes) as usize;
            if want == 0 {
                // Limit reached: is there more?
                let mut probe = [0u8; 1];
                let more = loop {
                    match reader.read(&mut probe) {
                        Ok(n) => break n > 0,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(io(e)),
                    }
                };
                if more {
                    if limit == remaining {
                        self.stop(StopReason::Bytes);
                    } else {
                        self.issue(format!(
                            "{}: Nur die ersten {} Bytes wurden durchsucht",
                            display(path),
                            self.limits.max_file_bytes
                        ));
                    }
                }
                break;
            }
            let n = read_full(reader, &mut buffer[..want]).map_err(io)?;
            if n == 0 {
                break;
            }
            file_bytes += n as u64;
            self.result.bytes_read += n as u64;
            let chunk = &buffer[..n];
            if first {
                first = false;
                if chunk.contains(&0) {
                    self.result.skipped_binary += 1;
                    return Ok(());
                }
                self.result.files_searched += 1;
            }
            if !self.scan_chunk(chunk, &mut lines, path, entry, &mut hits_in_file) {
                return Ok(());
            }
            if n < want {
                break;
            }
        }
        if !lines.carry.is_empty() {
            let line = std::mem::take(&mut lines.carry);
            lines.number += 1;
            if !self.scan_line(&line, lines.number, path, entry, &mut hits_in_file) {
                return Ok(());
            }
        }
        if lines.cut {
            self.issue(format!(
                "{}: Zeilen über {} Bytes wurden nur zum Teil durchsucht",
                display(path),
                MAX_LINE_BYTES
            ));
        }
        Ok(())
    }

    /// Splits the chunk into lines; `false` if the search of this file must end.
    fn scan_chunk(
        &mut self,
        chunk: &[u8],
        lines: &mut LineState,
        path: &VPath,
        entry: &Entry,
        hits_in_file: &mut usize,
    ) -> bool {
        let mut rest = chunk;
        while !rest.is_empty() {
            match rest.iter().position(|b| *b == b'\n') {
                Some(end) => {
                    lines.append(&rest[..end]);
                    let line = std::mem::take(&mut lines.carry);
                    lines.number += 1;
                    if !self.scan_line(&line, lines.number, path, entry, hits_in_file) {
                        return false;
                    }
                    rest = &rest[end + 1..];
                }
                None => {
                    lines.append(rest);
                    break;
                }
            }
        }
        true
    }

    /// Matches one line; `false` if the search of this file must end.
    fn scan_line(
        &mut self,
        line: &[u8],
        number: u64,
        path: &VPath,
        entry: &Entry,
        hits_in_file: &mut usize,
    ) -> bool {
        let Some(re) = &self.text_re else {
            return false;
        };
        let text = String::from_utf8_lossy(line);
        let text = text.strip_suffix('\r').unwrap_or(&*text);
        let Some(found) = re.find(text) else {
            return true;
        };
        if *hits_in_file >= self.limits.max_hits_per_file {
            self.issue(format!(
                "{}: Die Obergrenze für Treffer je Datei ist erreicht",
                display(path)
            ));
            return false;
        }
        let (shown, start, len) = excerpt(text, found.start(), found.end());
        if !self.push_hit(SearchHit {
            path: path.clone(),
            kind: EntryKind::File,
            size: entry.size,
            line: Some(number),
            excerpt: Some(shown),
            match_start: start,
            match_len: len,
        }) {
            return false;
        }
        *hits_in_file += 1;
        !self.should_stop()
    }
}

#[derive(Default)]
struct LineState {
    carry: Vec<u8>,
    number: u64,
    /// A line was longer than `MAX_LINE_BYTES`.
    cut: bool,
}

impl LineState {
    fn append(&mut self, part: &[u8]) {
        let room = MAX_LINE_BYTES.saturating_sub(self.carry.len());
        if part.len() > room {
            self.cut = true;
        }
        self.carry.extend_from_slice(&part[..part.len().min(room)]);
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

/// Checks what can be checked before a search is queued: the patterns compile within the
/// limits, something is searched for, and a server or plugin start was asked for.
pub fn check_options(options: &SearchOptions) -> Result<()> {
    let name = compile_name(options)?;
    let text = compile_text(options)?;
    if name.is_none() && text.is_none() {
        return Err(Error::InvalidInput(
            "Weder Namensmuster noch Text angegeben",
        ));
    }
    Ok(())
}

/// The refusal for a server/plugin start that was not asked for explicitly.
pub fn check_start(start: &VPath, options: &SearchOptions) -> Result<()> {
    if start.scheme().is_remote() && !options.allow_remote {
        return Err(Error::Unsupported(MSG_REMOTE_NOT_ALLOWED));
    }
    Ok(())
}

const MSG_REMOTE_NOT_ALLOWED: &str =
    "Suche auf Servern und in Plugin-Dateisystemen (nicht freigegeben)";

/// Searches below `start` (a local folder, a folder inside an archive, or, only with
/// `allow_remote`, a folder on a server or in a plugin). Returns an error only for input
/// that cannot be searched at all (bad pattern, no folder, remote not allowed); everything
/// that happens during the search ends up in the result (`incomplete`, `issues`).
pub fn search(
    router: &VfsRouter,
    start: &VPath,
    options: &SearchOptions,
    job: &JobHandle,
) -> Result<SearchResult> {
    let name_re = compile_name(options)?;
    let text_re = compile_text(options)?;
    if name_re.is_none() && text_re.is_none() {
        return Err(Error::InvalidInput(
            "Weder Namensmuster noch Text angegeben",
        ));
    }
    let remote = start.scheme().is_remote();
    if remote && !options.allow_remote {
        return Err(Error::Unsupported(
            "Suche auf Servern und in Plugin-Dateisystemen (nicht freigegeben)",
        ));
    }
    let limits = if remote {
        options.limits.for_remote()
    } else {
        options.limits.clone()
    };
    let archive_levels = if remote || !options.in_archives {
        0
    } else {
        options
            .archive_depth
            .clamp(1, MAX_ARCHIVE_NESTING)
            .min(MAX_ARCHIVE_NESTING.saturating_sub(start.archive_depth()))
    };
    let vfs = router.vfs_for(start)?;
    if vfs.stat(start)?.kind != EntryKind::Dir {
        return Err(Error::NotADirectory(start.path().to_path_buf()));
    }
    let mut searcher = Searcher {
        router,
        opts: options,
        name_re,
        text_re,
        job,
        started: Instant::now(),
        archive_budget: Budget::new(limits.archive_bytes, Some(limits.archive_time)),
        archives: Vec::new(),
        archive_levels,
        archive_exhausted: false,
        limits,
        result: SearchResult {
            root: start.clone(),
            hits: Vec::new(),
            dirs_visited: 0,
            files_visited: 0,
            files_searched: 0,
            archives_searched: 0,
            skipped_binary: 0,
            skipped_special: 0,
            bytes_read: 0,
            incomplete: false,
            cancelled: false,
            stopped: None,
            issues: Vec::new(),
        },
    };
    // A search that starts inside an archive reads it within the search budget, too.
    if let Some(file) = start.outer() {
        let index_budget = ArchiveVfs::new_index_budget(&ArchiveLimits::default());
        let archive = router.archive_for(file, &index_budget)?;
        archive.set_job_budget(searcher.archive_budget.clone());
        searcher.archives.push(archive);
    }
    searcher.walk(&*vfs, start.clone(), 0);
    // The cached archive instances are shared: give them their usual budget back.
    for archive in &searcher.archives {
        archive.reset_job_budget();
    }
    Ok(searcher.result)
}
