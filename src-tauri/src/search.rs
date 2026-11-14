//! Search over many files and archives as the UI sees it: option and result DTOs plus the
//! store that keeps finished results on the Rust side. The UI only ever holds the number
//! (job number = result id); the hits stay here and are fetched in pages.

use crate::convert::{kind_name, vpath_text, UiError};
use dualfile_core::search::{
    NameMode, SearchHit, SearchLimits, SearchOptions, SearchResult, StopReason, MAX_PATTERN_BYTES,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Finished or running search results kept at the same time (like comparisons).
pub const MAX_SEARCHES: usize = 4;
/// Largest page of `search_page`.
pub const MAX_SEARCH_PAGE: usize = 500;
/// Most hits one search keeps (more makes it incomplete, never silent).
pub const MAX_SEARCH_HITS: usize = 10_000;
/// Decompression budget of one search inside archives.
pub const SEARCH_ARCHIVE_BYTES: u64 = 8 << 30;
pub const SEARCH_ARCHIVE_TIME: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------- Options

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NameModeDto {
    #[default]
    Glob,
    Regex,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchOptionsDto {
    pub name: Option<String>,
    pub name_mode: NameModeDto,
    pub name_case_sensitive: bool,
    pub text: Option<String>,
    pub text_regex: bool,
    pub text_case_sensitive: bool,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Milliseconds since 1970.
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
    pub recursive: bool,
    pub in_archives: bool,
    /// Archive levels looked into (1 to 3).
    pub archive_depth: u8,
    /// Servers and plugin file systems are only searched when this is set.
    pub include_remote: bool,
}

impl Default for SearchOptionsDto {
    fn default() -> Self {
        Self {
            name: None,
            name_mode: NameModeDto::Glob,
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
            include_remote: false,
        }
    }
}

fn time_of(millis: i64) -> SystemTime {
    if millis >= 0 {
        UNIX_EPOCH + Duration::from_millis(millis.unsigned_abs())
    } else {
        UNIX_EPOCH - Duration::from_millis(millis.unsigned_abs())
    }
}

fn invalid(message: &str) -> UiError {
    UiError::new("invalidInput", message)
}

impl SearchOptionsDto {
    /// Checks what the UI sent and builds the core options with the fixed budgets (the UI
    /// never sets limits).
    pub fn to_core(&self) -> Result<SearchOptions, UiError> {
        for text in [&self.name, &self.text].into_iter().flatten() {
            if text.len() > MAX_PATTERN_BYTES || text.contains('\0') {
                return Err(invalid("Das Suchmuster ist ungültig oder zu lang"));
            }
        }
        let has = |value: &Option<String>| value.as_deref().is_some_and(|t| !t.is_empty());
        if !has(&self.name) && !has(&self.text) {
            return Err(invalid(
                "Bitte ein Namensmuster oder einen Suchtext angeben",
            ));
        }
        if let (Some(min), Some(max)) = (self.min_size, self.max_size) {
            if min > max {
                return Err(invalid("Die Größenangaben widersprechen sich"));
            }
        }
        if let (Some(after), Some(before)) = (self.modified_after, self.modified_before) {
            if after > before {
                return Err(invalid("Die Datumsangaben widersprechen sich"));
            }
        }
        if !(1..=3).contains(&self.archive_depth) {
            return Err(invalid("Die Archivtiefe muss zwischen 1 und 3 liegen"));
        }
        let options = SearchOptions {
            name: self.name.clone(),
            name_mode: match self.name_mode {
                NameModeDto::Glob => NameMode::Glob,
                NameModeDto::Regex => NameMode::Regex,
            },
            name_case_sensitive: self.name_case_sensitive,
            text: self.text.clone(),
            text_regex: self.text_regex,
            text_case_sensitive: self.text_case_sensitive,
            min_size: self.min_size,
            max_size: self.max_size,
            modified_after: self.modified_after.map(time_of),
            modified_before: self.modified_before.map(time_of),
            recursive: self.recursive,
            in_archives: self.in_archives,
            archive_depth: usize::from(self.archive_depth),
            allow_remote: self.include_remote,
            limits: SearchLimits {
                max_hits: MAX_SEARCH_HITS,
                archive_bytes: SEARCH_ARCHIVE_BYTES,
                archive_time: SEARCH_ARCHIVE_TIME,
                ..SearchLimits::default()
            },
        };
        dualfile_core::search::check_options(&options)?;
        Ok(options)
    }
}

// ---------------------------------------------------------------- Results

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitDto {
    /// IPC path (an archive entry as `archive:...!/inner`), usable with `list`.
    pub path: String,
    pub kind: &'static str,
    pub size: u64,
    /// 1-based line number of a text hit; `None` for a name hit.
    pub line: Option<u64>,
    pub excerpt: Option<String>,
    /// Position and length of the match inside `excerpt`, in characters.
    pub match_start: u32,
    pub match_len: u32,
}

fn hit_dto(hit: &SearchHit) -> SearchHitDto {
    SearchHitDto {
        path: vpath_text(&hit.path),
        kind: kind_name(hit.kind),
        size: hit.size,
        line: hit.line,
        excerpt: hit.excerpt.clone(),
        match_start: hit.match_start,
        match_len: hit.match_len,
    }
}

fn stop_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Cancelled => "cancelled",
        StopReason::Hits => "hits",
        StopReason::Files => "files",
        StopReason::Bytes => "bytes",
        StopReason::Time => "time",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSummaryDto {
    pub root: String,
    /// All hits of the result.
    pub total: u64,
    pub dirs_visited: u64,
    pub files_visited: u64,
    pub files_searched: u64,
    pub archives_searched: u64,
    pub skipped_binary: u64,
    pub skipped_special: u64,
    pub bytes_read: u64,
    /// Something was not examined (limit, read error, cancelled).
    pub incomplete: bool,
    pub cancelled: bool,
    /// `hits`, `files`, `bytes`, `time` or `cancelled` when a limit ended the search.
    pub stopped: Option<&'static str>,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPageDto {
    pub id: u64,
    pub offset: u64,
    pub total: u64,
    pub hits: Vec<SearchHitDto>,
    pub summary: SearchSummaryDto,
}

pub fn summary_dto(result: &SearchResult) -> SearchSummaryDto {
    SearchSummaryDto {
        root: vpath_text(&result.root),
        total: result.total(),
        dirs_visited: result.dirs_visited,
        files_visited: result.files_visited,
        files_searched: result.files_searched,
        archives_searched: result.archives_searched,
        skipped_binary: result.skipped_binary,
        skipped_special: result.skipped_special,
        bytes_read: result.bytes_read,
        incomplete: result.incomplete,
        cancelled: result.cancelled,
        stopped: result.stopped.map(stop_name),
        issues: result.issues.clone(),
    }
}

pub fn page(result: &SearchResult, id: u64, offset: usize, limit: usize) -> SearchPageDto {
    let limit = limit.clamp(1, MAX_SEARCH_PAGE);
    let hits = result
        .hits
        .iter()
        .skip(offset)
        .take(limit)
        .map(hit_dto)
        .collect();
    SearchPageDto {
        id,
        offset: offset as u64,
        total: result.total(),
        hits,
        summary: summary_dto(result),
    }
}

// ---------------------------------------------------------------- Store

#[derive(Default)]
struct StoreInner {
    results: HashMap<u64, Arc<SearchResult>>,
    /// Searches that were reserved but have not delivered their result yet.
    running: usize,
}

/// Search results by job number. At most `MAX_SEARCHES` exist (running ones count); the UI
/// frees one with `search_close`. A cancelled search keeps what it found so far (marked
/// `cancelled` and `incomplete`).
#[derive(Default)]
pub struct SearchStore {
    inner: Mutex<StoreInner>,
}

impl SearchStore {
    fn guard(&self) -> std::sync::MutexGuard<'_, StoreInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes one of the slots or refuses. The slot is given back when the guard is dropped
    /// without a result (job cancelled while queued, search refused, panic, ...).
    pub fn reserve(self: &Arc<Self>) -> Result<SearchReservation, UiError> {
        let mut inner = self.guard();
        if inner.results.len() + inner.running >= MAX_SEARCHES {
            return Err(invalid(&format!(
                "Es sind schon {MAX_SEARCHES} Suchergebnisse offen; bitte erst eines schließen"
            )));
        }
        inner.running += 1;
        Ok(SearchReservation {
            store: Arc::clone(self),
            used: false,
        })
    }

    pub fn get(&self, id: u64) -> Option<Arc<SearchResult>> {
        self.guard().results.get(&id).cloned()
    }

    /// Frees the result; `false` if there was none.
    pub fn close(&self, id: u64) -> bool {
        self.guard().results.remove(&id).is_some()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.guard().results.len()
    }
}

pub struct SearchReservation {
    store: Arc<SearchStore>,
    used: bool,
}

impl SearchReservation {
    /// Hands the result over under its job number.
    pub fn finish(mut self, id: u64, result: SearchResult) {
        self.used = true;
        let mut inner = self.store.guard();
        inner.running = inner.running.saturating_sub(1);
        inner.results.insert(id, Arc::new(result));
    }
}

impl Drop for SearchReservation {
    fn drop(&mut self) {
        if !self.used {
            let mut inner = self.store.guard();
            inner.running = inner.running.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::ops::JobHandle;
    use dualfile_core::search::search;
    use dualfile_core::vfs::VPath;
    use dualfile_core::VfsRouter;

    #[test]
    fn a_cancelled_result_is_kept_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let options = SearchOptionsDto {
            text: Some("needle".into()),
            ..SearchOptionsDto::default()
        }
        .to_core()
        .unwrap();
        let job = JobHandle::silent();
        job.cancel();
        let result = search(
            &VfsRouter::new(),
            &VPath::local(dir.path()).unwrap(),
            &options,
            &job,
        )
        .unwrap();
        let store = Arc::new(SearchStore::default());
        store.reserve().unwrap().finish(7, result);
        let page = page(&store.get(7).unwrap(), 7, 0, 10);
        assert!(page.summary.cancelled && page.summary.incomplete);
        assert_eq!(page.summary.stopped, Some("cancelled"));
        assert!(store.close(7));
        assert!(!store.close(7));
    }

    #[test]
    fn options_map_to_core_with_fixed_budgets() {
        let dto = SearchOptionsDto {
            name: Some("*.rs".into()),
            modified_after: Some(-1000),
            modified_before: Some(2000),
            archive_depth: 2,
            include_remote: true,
            ..SearchOptionsDto::default()
        };
        let core = dto.to_core().unwrap();
        assert!(core.allow_remote && core.recursive);
        assert_eq!(core.archive_depth, 2);
        assert_eq!(core.limits.max_hits, MAX_SEARCH_HITS);
        assert!(core.modified_after.unwrap() < core.modified_before.unwrap());
        // camelCase from the UI, missing fields default
        let parsed: SearchOptionsDto =
            serde_json::from_str(r#"{"text":"x","textRegex":true,"inArchives":true}"#).unwrap();
        assert!(parsed.text_regex && parsed.in_archives && parsed.recursive);
        assert_eq!(parsed.archive_depth, 1);
    }
}
