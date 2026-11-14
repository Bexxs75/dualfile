//! Read-only exact duplicate discovery: size -> sampled SHA-256 -> full SHA-256.
//! Hashing is serial (one worker, within min(4, available cores)); buffers are 64 KiB.
//! No deletion API. Results describe an observed version, not an authorization to delete.
use crate::{
    error::{Error, Result},
    ops::JobHandle,
    scan_fs::{self, Meta},
    usage::{issue, ScanIssue},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct DupeOptions {
    pub min_size: u64,
    /// All visited paths, including aliases/directories. Exhaustion marks incomplete.
    pub max_entries: usize,
    pub max_depth: usize,
}
impl Default for DupeOptions {
    fn default() -> Self {
        Self {
            min_size: 1,
            max_entries: 100_000,
            max_depth: 128,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DupeEntry {
    pub path: PathBuf,
    /// Other discovered names of this inode; primary path is lexicographically smallest.
    pub linked_paths: Vec<PathBuf>,
    pub modified: Option<SystemTime>,
    pub hard_links: u64,
}
#[derive(Clone, Debug)]
pub struct DupeGroup {
    pub hash: String,
    pub size: u64,
    pub entries: Vec<DupeEntry>,
    pub reclaimable_bytes: u64,
}
#[derive(Clone, Debug, Default)]
pub struct DupeResult {
    pub groups: Vec<DupeGroup>,
    pub skipped: Vec<(String, ScanIssue)>,
    pub skipped_count: u64,
    pub incomplete: bool,
    pub cancelled: bool,
    pub bytes_read: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DupeStage {
    Size,
    PartialHash,
    FullHash,
}
#[derive(Clone, Copy, Debug)]
pub struct DupeProgress {
    pub stage: DupeStage,
    pub done: u64,
    /// Size enumeration has no known total; hashing stages have exact candidate totals.
    pub total: Option<u64>,
    /// Cumulative across both hash stages, including failed/discarded reads.
    pub bytes_read: u64,
}
struct Candidate {
    entry: DupeEntry,
    meta: Meta,
}
struct Scan<'a> {
    options: &'a DupeOptions,
    job: &'a JobHandle,
    progress: &'a mut dyn FnMut(DupeProgress),
    result: DupeResult,
    candidates: Vec<Candidate>,
    paths: HashSet<PathBuf>,
    identities: HashMap<(u64, u64), usize>,
    stopped: bool,
}
impl Scan<'_> {
    fn note(&mut self, path: &Path, kind: ScanIssue) {
        issue(
            &mut self.result.skipped,
            &mut self.result.skipped_count,
            path,
            kind,
        );
        self.result.incomplete = true;
    }
    fn cancelled(&mut self) -> bool {
        if self.job.checkpoint() {
            self.result.cancelled = true;
            self.result.incomplete = true;
            self.stopped = true;
        }
        self.stopped
    }
    fn walk(&mut self, path: PathBuf, depth: usize) {
        if self.cancelled() || self.paths.contains(&path) {
            return;
        }
        if self.paths.len() >= self.options.max_entries {
            self.note(&path, ScanIssue::EntryLimit);
            self.stopped = true;
            return;
        }
        self.paths.insert(path.clone());
        (self.progress)(DupeProgress {
            stage: DupeStage::Size,
            done: self.paths.len() as u64,
            total: None,
            bytes_read: 0,
        });
        if self.cancelled() {
            return;
        }
        let meta = match scan_fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                self.note(&path, ScanIssue::Io(e.kind()));
                return;
            }
        };
        if meta.dir {
            if depth >= self.options.max_depth.min(128) {
                self.note(&path, ScanIssue::DepthLimit);
                return;
            }
            match scan_fs::children(&path) {
                Err(e) => self.note(&path, ScanIssue::Io(e.kind())),
                Ok(children) => {
                    for child in children {
                        if self.cancelled() {
                            break;
                        }
                        match child {
                            Ok(p) => self.walk(p, depth + 1),
                            Err(e) => self.note(&path, ScanIssue::Io(e.kind())),
                        }
                    }
                }
            }
        } else if meta.file && meta.len >= self.options.min_size.max(1) {
            if let Some(index) = meta
                .identity
                .and_then(|id| self.identities.get(&id).copied())
            {
                self.candidates[index].entry.linked_paths.push(path);
            } else {
                if let Some(id) = meta.identity {
                    self.identities.insert(id, self.candidates.len());
                }
                self.candidates.push(Candidate {
                    entry: DupeEntry {
                        path,
                        linked_paths: Vec::new(),
                        modified: meta.modified,
                        hard_links: meta.links,
                    },
                    meta,
                });
            }
        }
    }
}

fn unchanged(a: &Meta, b: &Meta) -> bool {
    a.file && b.file && a.len == b.len && a.modified == b.modified && a.identity == b.identity
}

fn hash_file(
    candidate: &Candidate,
    full: bool,
    job: &JobHandle,
    progress: &mut dyn FnMut(u64),
) -> std::result::Result<String, ScanIssue> {
    let io = |e: std::io::Error| ScanIssue::Io(e.kind());
    let mut file = scan_fs::open_file(&candidate.entry.path).map_err(io)?;
    let before = Meta::from_std(file.metadata().map_err(io)?);
    if !unchanged(&candidate.meta, &before) {
        return Err(ScanIssue::Changed);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 << 10];
    let ranges = if full {
        vec![(0, before.len)]
    } else if before.len > 1 << 20 {
        vec![(0, 64 << 10), (before.len - (64 << 10), 64 << 10)]
    } else {
        vec![(0, before.len.min(64 << 10))]
    };
    for (offset, mut left) in ranges {
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        while left > 0 {
            if job.checkpoint() {
                return Err(ScanIssue::Io(std::io::ErrorKind::Interrupted));
            }
            let wanted = left.min(buffer.len() as u64) as usize;
            let n = file.read(&mut buffer[..wanted]).map_err(io)?;
            if n == 0 {
                return Err(ScanIssue::Changed);
            }
            digest.update(&buffer[..n]);
            left -= n as u64;
            progress(n as u64);
        }
    }
    let after = Meta::from_std(file.metadata().map_err(io)?);
    let at_path = scan_fs::metadata(&candidate.entry.path).map_err(io)?;
    if !unchanged(&before, &after) || !unchanged(&before, &at_path) {
        return Err(ScanIssue::Changed);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Local roots are resolved once. Overlapping roots and Unix inode aliases are merged.
/// Non-Unix has no inode deduplication. Memory is O(max_entries + min(max_depth,128)).
/// Errors are capped at 200 displayed paths; skipped_count preserves the full count.
pub fn find_duplicates(
    roots: &[PathBuf],
    options: &DupeOptions,
    job: &JobHandle,
    progress: &mut dyn FnMut(DupeProgress),
) -> Result<DupeResult> {
    if options.max_entries == 0 {
        return Err(Error::InvalidInput("max_entries muss positiv sein"));
    }
    let mut s = Scan {
        options,
        job,
        progress,
        result: DupeResult::default(),
        candidates: Vec::new(),
        paths: HashSet::new(),
        identities: HashMap::new(),
        stopped: false,
    };
    for root in roots {
        if s.cancelled() {
            break;
        }
        match scan_fs::root(root) {
            Ok(path) => s.walk(path, 0),
            Err(e) => s.note(root, ScanIssue::Io(e.kind())),
        }
    }
    for c in &mut s.candidates {
        c.entry.linked_paths.push(c.entry.path.clone());
        c.entry.linked_paths.sort();
        c.entry.path = c.entry.linked_paths.remove(0);
    }
    let mut by_size = BTreeMap::<u64, Vec<usize>>::new();
    for (i, c) in s.candidates.iter().enumerate() {
        by_size.entry(c.meta.len).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = by_size.into_values().filter(|g| g.len() > 1).collect();
    // Reaching the path budget still permits hashing the bounded candidates already found.
    s.stopped = s.result.cancelled;
    for stage in [DupeStage::PartialHash, DupeStage::FullHash] {
        let total = groups.iter().map(|g| g.len() as u64).sum();
        let mut done = 0;
        let mut next = Vec::new();
        (s.progress)(DupeProgress {
            stage,
            done,
            total: Some(total),
            bytes_read: s.result.bytes_read,
        });
        for group in groups {
            let mut hashes = BTreeMap::<String, Vec<usize>>::new();
            for index in group {
                if s.cancelled() {
                    break;
                }
                let candidate = &s.candidates[index];
                let hash = hash_file(candidate, stage == DupeStage::FullHash, job, &mut |bytes| {
                    s.result.bytes_read = s.result.bytes_read.saturating_add(bytes);
                    (s.progress)(DupeProgress {
                        stage,
                        done,
                        total: Some(total),
                        bytes_read: s.result.bytes_read,
                    });
                });
                done += 1;
                if s.cancelled() {
                    break;
                }
                match hash {
                    Ok(hash) => hashes.entry(hash).or_default().push(index),
                    Err(kind) => {
                        let path = s.candidates[index].entry.path.clone();
                        s.note(&path, kind);
                    }
                }
                (s.progress)(DupeProgress {
                    stage,
                    done,
                    total: Some(total),
                    bytes_read: s.result.bytes_read,
                });
            }
            for (hash, matches) in hashes.into_iter().filter(|(_, g)| g.len() > 1) {
                if stage == DupeStage::FullHash {
                    let size = s.candidates[matches[0]].meta.len;
                    let mut entries: Vec<_> = matches
                        .iter()
                        .map(|&i| s.candidates[i].entry.clone())
                        .collect();
                    entries.sort_by(|a, b| a.path.cmp(&b.path));
                    s.result.groups.push(DupeGroup {
                        hash,
                        size,
                        reclaimable_bytes: size.saturating_mul((entries.len() - 1) as u64),
                        entries,
                    });
                } else {
                    next.push(matches);
                }
            }
            if s.result.cancelled {
                break;
            }
        }
        groups = next;
        if s.result.cancelled {
            break;
        }
    }
    s.result.groups.sort_by(|a, b| {
        b.reclaimable_bytes
            .cmp(&a.reclaimable_bytes)
            .then_with(|| a.hash.cmp(&b.hash))
            .then_with(|| a.entries[0].path.cmp(&b.entries[0].path))
    });
    Ok(s.result)
}

#[derive(Clone, Debug)]
pub enum KeepRule {
    Oldest,
    Newest,
    ShortestPath,
    InFolder(PathBuf),
}
#[derive(Debug)]
pub struct KeepSuggestion<'a> {
    pub keep: &'a DupeEntry,
    pub redundant: Vec<&'a DupeEntry>,
}
/// Missing timestamps sort after known timestamps for both time rules. Folder membership
/// considers all hardlink names. Empty groups return None; ties use native Path ordering.
pub fn suggest_keep(group: &DupeGroup, rule: KeepRule) -> Option<KeepSuggestion<'_>> {
    let keep = group.entries.iter().min_by(|a, b| {
        let ordering = match &rule {
            KeepRule::Oldest => a
                .modified
                .is_none()
                .cmp(&b.modified.is_none())
                .then_with(|| a.modified.cmp(&b.modified)),
            KeepRule::Newest => a
                .modified
                .is_none()
                .cmp(&b.modified.is_none())
                .then_with(|| b.modified.cmp(&a.modified)),
            KeepRule::ShortestPath => a.path.as_os_str().len().cmp(&b.path.as_os_str().len()),
            KeepRule::InFolder(root) => {
                let inside = |e: &DupeEntry| {
                    e.path.starts_with(root) || e.linked_paths.iter().any(|p| p.starts_with(root))
                };
                inside(b).cmp(&inside(a))
            }
        };
        ordering.then_with(|| a.path.cmp(&b.path))
    })?;
    Some(KeepSuggestion {
        keep,
        redundant: group
            .entries
            .iter()
            .filter(|e| !std::ptr::eq(*e, keep))
            .collect(),
    })
}

#[cfg(test)]
mod tests;
