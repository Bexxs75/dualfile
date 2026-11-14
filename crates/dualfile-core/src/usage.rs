//! Local, read-only disk usage. Symlinks contribute their own metadata size. Unix
//! hardlinks are counted once; other platforms use len and do not deduplicate inodes.
//! Directory metadata bytes are excluded; directories themselves count (including root).
use crate::{
    error::{from_io, Error, Result},
    ops::JobHandle,
    scan_fs,
};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct UsageOptions {
    /// Stored named nodes (root included). Collapsed children live in `other`.
    pub max_entries: usize,
    /// Separate bound for inode deduplication. Exhaustion stops the scan explicitly.
    pub max_identities: usize,
    pub max_depth: usize,
    pub one_file_system: bool,
}
impl Default for UsageOptions {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_identities: 1_000_000,
            max_depth: 128,
            one_file_system: true,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub size: u64,
    pub allocated: u64,
    pub files: u64,
    pub directories: u64,
}
impl UsageTotals {
    fn add(&mut self, b: Self) {
        self.size = self.size.saturating_add(b.size);
        self.allocated = self.allocated.saturating_add(b.allocated);
        self.files = self.files.saturating_add(b.files);
        self.directories = self.directories.saturating_add(b.directories);
    }
}
#[derive(Clone, Debug)]
pub struct UsageNode {
    pub path: PathBuf,
    pub totals: UsageTotals,
    pub children: Vec<UsageNode>,
    /// “Sonstiges”: exact aggregate, no drill-down into discarded names.
    pub other: Option<UsageTotals>,
}
impl UsageNode {
    pub fn stored_entries(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(Self::stored_entries)
            .sum::<usize>()
    }
    fn compact(&mut self, budget: usize) {
        self.children.sort_by(|a, b| {
            b.totals
                .size
                .cmp(&a.totals.size)
                .then_with(|| a.path.cmp(&b.path))
        });
        while self.stored_entries() > budget {
            let child = self.children.pop().expect("over budget has children");
            self.other.get_or_insert_default().add(child.totals);
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanIssue {
    Io(io::ErrorKind),
    DepthLimit,
    EntryLimit,
    FileSystemBoundary,
    Changed,
}
#[derive(Clone, Debug)]
pub struct UsageResult {
    pub root: UsageNode,
    pub skipped: Vec<(String, ScanIssue)>,
    pub skipped_count: u64,
    pub incomplete: bool,
    pub cancelled: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageProgress {
    pub entries_seen: u64,
    pub bytes: u64,
}

pub(crate) fn issue(
    list: &mut Vec<(String, ScanIssue)>,
    count: &mut u64,
    path: &Path,
    kind: ScanIssue,
) {
    *count += 1;
    if list.len() < 200 {
        list.push((path.display().to_string(), kind));
    }
}
struct Walker<'a> {
    opts: &'a UsageOptions,
    job: &'a JobHandle,
    progress: &'a mut dyn FnMut(UsageProgress),
    seen: HashSet<(u64, u64)>,
    device: Option<u64>,
    skipped: Vec<(String, ScanIssue)>,
    skipped_count: u64,
    stopped: bool,
    cancelled: bool,
    state: UsageProgress,
    last: Option<Instant>,
}
impl Walker<'_> {
    fn note(&mut self, path: &Path, kind: ScanIssue) {
        issue(&mut self.skipped, &mut self.skipped_count, path, kind);
    }
    fn visit(&mut self, path: PathBuf, depth: usize, budget: usize) -> UsageNode {
        let mut node = UsageNode {
            path,
            totals: UsageTotals::default(),
            children: Vec::new(),
            other: None,
        };
        if self.job.checkpoint() {
            self.cancelled = true;
            self.stopped = true;
        }
        if self.stopped {
            return node;
        }
        let meta = match scan_fs::metadata(&node.path) {
            Ok(m) => m,
            Err(e) => {
                self.note(&node.path, ScanIssue::Io(e.kind()));
                return node;
            }
        };
        self.state.entries_seen += 1;
        if self.opts.one_file_system && !same_device(self.device, meta.identity.map(|id| id.0)) {
            self.note(&node.path, ScanIssue::FileSystemBoundary);
            return node;
        }
        if let Some(id) = meta.identity {
            if self.seen.contains(&id) {
                return node;
            }
            if self.seen.len() >= self.opts.max_identities {
                self.note(&node.path, ScanIssue::EntryLimit);
                self.stopped = true;
                return node;
            }
            self.seen.insert(id);
        }
        if meta.dir {
            node.totals.directories = 1;
        } else {
            node.totals = UsageTotals {
                size: meta.len,
                allocated: meta.allocated,
                files: 1,
                directories: 0,
            };
            self.state.bytes = self.state.bytes.saturating_add(meta.len);
        }
        if self
            .last
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(100))
        {
            (self.progress)(self.state);
            self.last = Some(Instant::now());
        }
        if self.job.checkpoint() {
            self.cancelled = true;
            self.stopped = true;
            return node;
        }
        if meta.dir {
            if depth >= self.opts.max_depth.min(128) {
                self.note(&node.path, ScanIssue::DepthLimit);
                return node;
            }
            match scan_fs::children(&node.path) {
                Err(e) => self.note(&node.path, ScanIssue::Io(e.kind())),
                Ok(children) => {
                    for child in children {
                        if self.stopped {
                            break;
                        }
                        match child {
                            Err(e) => self.note(&node.path, ScanIssue::Io(e.kind())),
                            Ok(p) => {
                                // Retained siblings share this budget with the active subtree.
                                let available = budget.saturating_sub(node.stored_entries()).max(1);
                                let child = self.visit(p, depth + 1, available);
                                node.totals.add(child.totals);
                                node.children.push(child);
                                node.compact(budget);
                            }
                        }
                    }
                }
            }
        }
        node
    }
}
fn same_device(root: Option<u64>, entry: Option<u64>) -> bool {
    root.is_none() || root == entry
}

/// Measures once. The result has <= max_entries named nodes; temporary trees are bounded
/// by O(max_entries + min(max_depth,128)), inode storage separately by max_identities.
/// Progress is rate-limited to 100 ms (including completion); the result is authoritative.
pub fn measure_usage(
    root: &Path,
    options: &UsageOptions,
    job: &JobHandle,
    progress: &mut dyn FnMut(UsageProgress),
) -> Result<UsageResult> {
    if options.max_entries == 0 {
        return Err(Error::InvalidInput("max_entries muss positiv sein"));
    }
    let root = scan_fs::root(root).map_err(|e| from_io("Wurzel auflösen", root, e))?;
    let meta = scan_fs::metadata(&root).map_err(|e| from_io("Wurzel lesen", &root, e))?;
    if !meta.dir {
        return Err(Error::NotADirectory(root));
    }
    let mut w = Walker {
        opts: options,
        job,
        progress,
        seen: HashSet::new(),
        device: meta.identity.map(|id| id.0),
        skipped: Vec::new(),
        skipped_count: 0,
        stopped: false,
        cancelled: false,
        state: UsageProgress::default(),
        last: None,
    };
    let root = w.visit(root, 0, options.max_entries);
    Ok(UsageResult {
        root,
        incomplete: w.skipped_count > 0 || w.cancelled,
        skipped: w.skipped,
        skipped_count: w.skipped_count,
        cancelled: w.cancelled,
    })
}

#[cfg(test)]
mod tests;
