//! Local, read-only image similarity. Similarity is only a suggestion, never permission
//! to delete. Connected components may contain pairs farther apart than max_distance.
//! Mirroring is not normalized. Animated formats use the decoder's first image.
use crate::{
    error::{Error, Result},
    ops::JobHandle,
    scan_fs::{self, Meta},
    usage::{issue, ScanIssue},
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime},
};

/// Strict per-axis decode limit.
pub const MAX_DIMENSION: u32 = 16_384;
/// Decoder allocation budget (image's internal allocations are best-effort bounded).
pub const MAX_DECODE_BYTES: u64 = 256 << 20;
/// Overfull bands are omitted, explicitly marking the result incomplete.
pub const MAX_BUCKET_ENTRIES: usize = 5_000;
/// Global bound also covers repeated pairs across bands; no quadratic pair set is kept.
pub const MAX_PAIR_CHECKS: u64 = 20_000_000;
#[derive(Clone, Debug)]
pub struct SimilarOptions {
    pub min_size: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_distance: u32,
}
impl Default for SimilarOptions {
    fn default() -> Self {
        Self {
            min_size: 1024,
            max_file_bytes: 64 << 20,
            max_entries: 100_000,
            max_depth: 128,
            max_distance: 6,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimilarEntry {
    pub path: PathBuf,
    pub linked_paths: Vec<PathBuf>,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub width: u32,
    pub height: u32,
    pub hash: u64,
    pub hash_hex: String,
    /// Hamming distance to the lexicographically smallest path, not necessarily entries[0].
    pub distance_to_reference: u32,
    pub hard_links: u64,
}
#[derive(Clone, Debug)]
pub struct SimilarGroup {
    pub entries: Vec<SimilarEntry>,
    pub reclaimable_bytes: u64,
}
#[derive(Clone, Debug, Default)]
pub struct SimilarResult {
    pub groups: Vec<SimilarGroup>,
    pub skipped: Vec<(String, ScanIssue)>,
    pub skipped_count: u64,
    pub incomplete: bool,
    pub cancelled: bool,
    pub bytes_read: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimilarStage {
    Scan,
    Hash,
    Group,
}
#[derive(Clone, Copy, Debug)]
pub struct SimilarProgress {
    pub stage: SimilarStage,
    pub done: u64,
    pub total: Option<u64>,
    /// Actual read bytes, including decoder rereads and discarded candidates.
    pub bytes_read: u64,
}
struct Reporter<'a> {
    callback: &'a mut dyn FnMut(SimilarProgress),
    last: Option<(SimilarStage, Instant)>,
}
impl Reporter<'_> {
    fn emit(&mut self, p: SimilarProgress) {
        if self.last.is_none_or(|(stage, time)| {
            stage != p.stage || time.elapsed() >= Duration::from_millis(100)
        }) {
            (self.callback)(p);
            self.last = Some((p.stage, Instant::now()));
        }
    }
}
fn format(path: &Path) -> Option<ImageFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::WebP),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}
struct Candidate {
    entry: SimilarEntry,
    meta: Meta,
}
struct Scan<'a> {
    options: &'a SimilarOptions,
    job: &'a JobHandle,
    progress: Reporter<'a>,
    result: SimilarResult,
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
        self.progress.emit(SimilarProgress {
            stage: SimilarStage::Scan,
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
        } else if meta.file && format(&path).is_some() && meta.len >= self.options.min_size {
            if meta.len > self.options.max_file_bytes {
                self.note(&path, ScanIssue::Io(std::io::ErrorKind::FileTooLarge));
                return;
            }
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
                    entry: SimilarEntry {
                        path,
                        linked_paths: Vec::new(),
                        modified: meta.modified,
                        hard_links: meta.links,
                        size: meta.len,
                        width: 0,
                        height: 0,
                        hash: 0,
                        hash_hex: String::new(),
                        distance_to_reference: 0,
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

/// Limits reads to the observed file length even if a writer grows it concurrently.
/// Checkpoints on every physical read/seek make decoding cooperatively cancellable.
/// Cancellation is not Interrupted: read_exact would retry that error forever.
struct CheckedReader<'a> {
    file: &'a std::fs::File,
    job: &'a JobHandle,
    bytes: &'a AtomicU64,
    position: u64,
    length: u64,
}
impl Read for CheckedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.job.checkpoint() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        let count = (buffer.len() as u64).min(self.length.saturating_sub(self.position)) as usize;
        let n = self.file.read(&mut buffer[..count])?;
        self.position += n as u64;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}
impl Seek for CheckedReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.job.checkpoint() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        self.position = self.file.seek(pos)?;
        Ok(self.position)
    }
}
fn guarded<T>(
    f: impl FnOnce() -> std::result::Result<T, ScanIssue>,
) -> std::result::Result<T, ScanIssue> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or(Err(ScanIssue::Io(io::ErrorKind::InvalidData)))
}
fn decode_file(
    candidate: &Candidate,
    job: &JobHandle,
    bytes: &AtomicU64,
) -> std::result::Result<DynamicImage, ScanIssue> {
    guarded(|| {
        let io = |e: io::Error| ScanIssue::Io(e.kind());
        let invalid = |_| ScanIssue::Io(io::ErrorKind::InvalidData);
        let file = scan_fs::open_file(&candidate.entry.path).map_err(io)?;
        let before = Meta::from_std(file.metadata().map_err(io)?);
        if !unchanged(&candidate.meta, &before) {
            return Err(ScanIssue::Changed);
        }
        let input = CheckedReader {
            file: &file,
            job,
            bytes,
            position: 0,
            length: before.len,
        };
        let mut reader = ImageReader::with_format(
            BufReader::new(input),
            format(&candidate.entry.path).ok_or(ScanIssue::Io(io::ErrorKind::InvalidData))?,
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_DIMENSION);
        limits.max_image_height = Some(MAX_DIMENSION);
        limits.max_alloc = Some(MAX_DECODE_BYTES);
        reader.limits(limits);
        let mut decoder = reader.into_decoder().map_err(invalid)?;
        // into_decoder sets limits but does not reserve the DynamicImage output buffer.
        if decoder.total_bytes() > MAX_DECODE_BYTES {
            return Err(ScanIssue::Io(io::ErrorKind::InvalidData));
        }
        let orientation = decoder.orientation().map_err(invalid)?;
        let mut img = DynamicImage::from_decoder(decoder).map_err(invalid)?;
        if job.checkpoint() {
            return Err(ScanIssue::Io(io::ErrorKind::Interrupted));
        }
        img.apply_orientation(orientation);
        let after = Meta::from_std(file.metadata().map_err(io)?);
        let at_path = scan_fs::metadata(&candidate.entry.path).map_err(io)?;
        if !unchanged(&before, &after) || !unchanged(&before, &at_path) {
            return Err(ScanIssue::Changed);
        }
        Ok(img)
    })
}

fn hash_file(
    candidate: &Candidate,
    job: &JobHandle,
    bytes: &AtomicU64,
) -> std::result::Result<SimilarEntry, ScanIssue> {
    guarded(|| {
        let img = decode_file(candidate, job, bytes)?;
        let mut entry = candidate.entry.clone();
        entry.width = img.width();
        entry.height = img.height();
        entry.hash = phash(&img);
        entry.hash_hex = format!("{:016x}", entry.hash);
        if job.checkpoint() {
            return Err(ScanIssue::Io(io::ErrorKind::Interrupted));
        }
        let at_path = scan_fs::metadata(&entry.path).map_err(|e| ScanIssue::Io(e.kind()))?;
        if !unchanged(&candidate.meta, &at_path) {
            return Err(ScanIssue::Changed);
        }
        Ok(entry)
    })
}

/// In-memory PNG. No cache or temporary files are created.
#[derive(Debug)]
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn render_thumbnail(path: &Path, max_edge: u32) -> Result<Thumbnail> {
    render_thumbnail_checked(path, max_edge, &JobHandle::silent())
}

/// Cooperative variant used by session owners to cancel reads on close.
pub fn render_thumbnail_checked(path: &Path, max_edge: u32, job: &JobHandle) -> Result<Thumbnail> {
    let result = guarded(|| {
        let meta = scan_fs::metadata(path).map_err(|e| ScanIssue::Io(e.kind()))?;
        if !meta.file || meta.len > SimilarOptions::default().max_file_bytes {
            return Err(ScanIssue::Io(io::ErrorKind::InvalidData));
        }
        let candidate = Candidate {
            entry: SimilarEntry {
                path: path.into(),
                linked_paths: vec![],
                size: meta.len,
                modified: meta.modified,
                hard_links: meta.links,
                width: 0,
                height: 0,
                hash: 0,
                hash_hex: String::new(),
                distance_to_reference: 0,
            },
            meta,
        };
        let img = decode_file(&candidate, job, &AtomicU64::new(0))?;
        let edge = max_edge.clamp(64, 512).min(img.width().max(img.height()));
        let img = img.thumbnail(edge, edge);
        if job.checkpoint() {
            return Err(ScanIssue::Io(io::ErrorKind::Interrupted));
        }
        let mut output = io::Cursor::new(Vec::new());
        img.write_to(&mut output, ImageFormat::Png)
            .map_err(|_| ScanIssue::Io(io::ErrorKind::InvalidData))?;
        Ok(Thumbnail {
            bytes: output.into_inner(),
            width: img.width(),
            height: img.height(),
        })
    });
    result.map_err(|issue| {
        if job.is_cancelled() {
            Error::Cancelled
        } else if issue == ScanIssue::Changed {
            Error::ChangedSincePlan(path.into())
        } else {
            Error::InvalidInput("Vorschaubild konnte nicht gelesen werden")
        }
    })
}

/// Resolves each input root once, then uses the same symlink-free reader as dupes.
/// Unix inode aliases count once. Non-Unix inherits scan_fs's documented race limits.
/// A skipped candidate or grouping budget makes the result explicitly incomplete.
pub fn find_similar_images(
    roots: &[PathBuf],
    options: &SimilarOptions,
    job: &JobHandle,
    progress: &mut dyn FnMut(SimilarProgress),
) -> Result<SimilarResult> {
    if options.max_entries == 0 {
        return Err(Error::InvalidInput("max_entries muss positiv sein"));
    }
    if options.max_distance > 16 {
        return Err(Error::InvalidInput(
            "max_distance muss zwischen 0 und 16 liegen",
        ));
    }
    let mut s = Scan {
        options,
        job,
        progress: Reporter {
            callback: progress,
            last: None,
        },
        result: SimilarResult::default(),
        candidates: Vec::new(),
        paths: HashSet::new(),
        identities: HashMap::new(),
        stopped: false,
    };
    s.progress.emit(SimilarProgress {
        stage: SimilarStage::Scan,
        done: 0,
        total: None,
        bytes_read: 0,
    });
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
    s.candidates.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
    if s.result.cancelled {
        return Ok(s.result);
    }
    let total = s.candidates.len() as u64;
    s.progress.emit(SimilarProgress {
        stage: SimilarStage::Hash,
        done: 0,
        total: Some(total),
        bytes_read: 0,
    });
    let next = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    let mut entries = Vec::new();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel(workers);
        let candidates = &s.candidates;
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            let bytes = &bytes;
            scope.spawn(move || {
                while !job.checkpoint() {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(candidate) = candidates.get(index) else {
                        break;
                    };
                    let result = hash_file(candidate, job, bytes);
                    if tx.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut done = 0;
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok((index, result)) => {
                    done += 1;
                    match result {
                        Ok(entry) => entries.push(entry),
                        Err(kind) if !job.is_cancelled() => {
                            issue(
                                &mut s.result.skipped,
                                &mut s.result.skipped_count,
                                &candidates[index].entry.path,
                                kind,
                            );
                            s.result.incomplete = true;
                        }
                        Err(_) => {}
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            s.progress.emit(SimilarProgress {
                stage: SimilarStage::Hash,
                done,
                total: Some(total),
                bytes_read: bytes.load(Ordering::Relaxed),
            });
        }
    });
    s.result.bytes_read = bytes.load(Ordering::Relaxed);
    if job.checkpoint() {
        s.result.cancelled = true;
        s.result.incomplete = true;
        return Ok(s.result);
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let hashes: Vec<_> = entries.iter().map(|e| e.hash).collect();
    let grouped = group_hashes(&hashes, options.max_distance, job, &mut |done| {
        s.progress.emit(SimilarProgress {
            stage: SimilarStage::Group,
            done,
            total: Some(hashes.len() as u64),
            bytes_read: s.result.bytes_read,
        });
    });
    for index in grouped.limited {
        s.note(&entries[index].path, ScanIssue::EntryLimit);
    }
    s.result.cancelled = job.is_cancelled();
    s.result.incomplete |= s.result.cancelled;
    s.result.groups = make_groups(&entries, grouped.components);
    Ok(s.result)
}
fn make_groups(entries: &[SimilarEntry], components: Vec<Vec<usize>>) -> Vec<SimilarGroup> {
    let mut groups = Vec::new();
    for component in components {
        let reference = entries[component[0]].hash; // entries and component indices are path-sorted
        let mut group_entries: Vec<_> = component
            .into_iter()
            .map(|i| {
                let mut entry = entries[i].clone();
                entry.distance_to_reference = (entry.hash ^ reference).count_ones();
                entry
            })
            .collect();
        let largest = group_entries.iter().map(|e| e.size).max().unwrap_or(0);
        let reclaimable_bytes = group_entries
            .iter()
            .fold(0u64, |sum, e| sum.saturating_add(e.size))
            .saturating_sub(largest);
        group_entries.sort_by(|a, b| {
            pixels(b)
                .cmp(&pixels(a))
                .then_with(|| b.size.cmp(&a.size))
                .then_with(|| a.path.cmp(&b.path))
        });
        groups.push(SimilarGroup {
            entries: group_entries,
            reclaimable_bytes,
        });
    }
    groups.sort_by(|a, b| {
        b.reclaimable_bytes.cmp(&a.reclaimable_bytes).then_with(|| {
            a.entries
                .iter()
                .map(|e| &e.path)
                .min()
                .cmp(&b.entries.iter().map(|e| &e.path).min())
        })
    });
    groups
}
fn pixels(e: &SimilarEntry) -> u64 {
    u64::from(e.width) * u64::from(e.height)
}

struct Grouped {
    components: Vec<Vec<usize>>,
    limited: Vec<usize>,
}
fn representative(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
fn group_hashes(
    hashes: &[u64],
    distance: u32,
    job: &JobHandle,
    progress: &mut dyn FnMut(u64),
) -> Grouped {
    let mut parent: Vec<_> = (0..hashes.len()).collect();
    let mut ranks = vec![0u8; hashes.len()];
    let mut limited = Vec::new();
    let mut checks = 0;
    progress(0);
    'bands: for band in 0..=distance {
        let start = 64 * band / (distance + 1);
        let end = 64 * (band + 1) / (distance + 1);
        let mask = u64::MAX >> (64 - (end - start));
        // Sorted maps make budget truncation deterministic, independent of hash seeds.
        let mut buckets = BTreeMap::<u64, Vec<usize>>::new();
        for (i, hash) in hashes.iter().enumerate() {
            if i % 256 == 0 && job.checkpoint() {
                break 'bands;
            }
            buckets.entry((hash >> start) & mask).or_default().push(i);
        }
        for bucket in buckets.values() {
            if job.checkpoint() {
                break 'bands;
            }
            if bucket.len() > MAX_BUCKET_ENTRIES {
                limited.push(bucket[0]);
                continue;
            }
            for (pos, &i) in bucket.iter().enumerate() {
                for &j in &bucket[..pos] {
                    if checks >= MAX_PAIR_CHECKS {
                        limited.push(i);
                        break 'bands;
                    }
                    if checks % 4096 == 0 {
                        progress(band as u64 * hashes.len() as u64 / u64::from(distance + 1));
                        if job.checkpoint() {
                            break 'bands;
                        }
                    }
                    checks += 1;
                    if (hashes[i] ^ hashes[j]).count_ones() <= distance {
                        let mut a = representative(&mut parent, i);
                        let mut b = representative(&mut parent, j);
                        if a != b {
                            if ranks[a] < ranks[b] {
                                std::mem::swap(&mut a, &mut b);
                            }
                            parent[b] = a;
                            if ranks[a] == ranks[b] {
                                ranks[a] += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let mut components = BTreeMap::<usize, Vec<usize>>::new();
    for i in 0..hashes.len() {
        components
            .entry(representative(&mut parent, i))
            .or_default()
            .push(i);
    }
    progress(hashes.len() as u64);
    Grouped {
        components: components.into_values().filter(|c| c.len() > 1).collect(),
        limited,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SimilarKeepRule {
    LargestResolution,
    LargestFile,
    Oldest,
    Newest,
    ShortestPath,
}
#[derive(Debug)]
pub struct SimilarKeepSuggestion<'a> {
    pub keep: &'a SimilarEntry,
    /// Alternatives for explicit user review, not a deletion list.
    pub alternatives: Vec<&'a SimilarEntry>,
}
/// Pure selection suggestion. Missing timestamps sort last; ties use native path order.
pub fn suggest_keep_similar(
    group: &SimilarGroup,
    rule: SimilarKeepRule,
) -> Option<SimilarKeepSuggestion<'_>> {
    let keep = group.entries.iter().min_by(|a, b| {
        let order = match rule {
            SimilarKeepRule::LargestResolution => pixels(b).cmp(&pixels(a)),
            SimilarKeepRule::LargestFile => b.size.cmp(&a.size),
            SimilarKeepRule::Oldest => a
                .modified
                .is_none()
                .cmp(&b.modified.is_none())
                .then_with(|| a.modified.cmp(&b.modified)),
            SimilarKeepRule::Newest => a
                .modified
                .is_none()
                .cmp(&b.modified.is_none())
                .then_with(|| b.modified.cmp(&a.modified)),
            SimilarKeepRule::ShortestPath => {
                a.path.as_os_str().len().cmp(&b.path.as_os_str().len())
            }
        };
        order.then_with(|| a.path.cmp(&b.path))
    })?;
    Some(SimilarKeepSuggestion {
        keep,
        alternatives: group
            .entries
            .iter()
            .filter(|e| !std::ptr::eq(*e, keep))
            .collect(),
    })
}

// Fixed IEEE-754 cosine samples cos(k*pi/64), k=0..32. Reflection supplies
// the other quadrants, including exact zeros. No platform libm calls or FMA.
const COS: [f64; 33] = [
    1.0,
    0.9987954562051724,
    0.9951847266721969,
    0.989176509964781,
    0.9807852804032304,
    0.970031253194544,
    0.9569403357322088,
    0.9415440651830208,
    0.9238795325112867,
    0.9039892931234433,
    0.881921264348355,
    0.8577286100002721,
    0.8314696123025452,
    0.8032075314806449,
    0.773010453362737,
    0.7409511253549591,
    std::f64::consts::FRAC_1_SQRT_2,
    0.6715589548470183,
    0.6343932841636455,
    0.5956993044924335,
    0.5555702330196023,
    0.5141027441932217,
    0.4713967368259978,
    0.4275550934302822,
    0.38268343236508984,
    0.33688985339222005,
    0.29028467725446233,
    0.24298017990326398,
    0.19509032201612833,
    0.14673047445536175,
    0.09801714032956077,
    0.049067674327418126,
    0.0,
];
fn cosine(index: usize) -> f64 {
    let i = index % 128;
    match i {
        0..=32 => COS[i],
        33..=64 => -COS[64 - i],
        65..=96 => -COS[i - 64],
        _ => COS[128 - i],
    }
}
/// 64-bit DCT-II pHash. Row-major 8x8 low frequencies; bit 0 is DC.
/// Strictly greater than the median of the 63 AC values sets a bit.
/// Fixed cosine constants and ordered f64 operations avoid platform libm variance.
/// The caller supplies an already oriented image; file discovery applies EXIF first.
pub fn phash(img: &DynamicImage) -> u64 {
    let small = image::imageops::resize(
        &img.to_luma8(),
        32,
        32,
        image::imageops::FilterType::Triangle,
    );
    let mut horizontal = [[0f64; 8]; 32];
    for (y, row) in horizontal.iter_mut().enumerate() {
        for (u, value) in row.iter_mut().enumerate() {
            for x in 0..32 {
                *value +=
                    f64::from(small.get_pixel(x as u32, y as u32)[0]) * cosine((2 * x + 1) * u);
            }
        }
    }
    let mut coefficients = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            for (y, row) in horizontal.iter().enumerate() {
                coefficients[v * 8 + u] += row[u] * cosine((2 * y + 1) * v);
            }
        }
    }
    let mut ac = coefficients[1..].to_vec();
    ac.sort_by(f64::total_cmp);
    let median = ac[31];
    coefficients
        .iter()
        .enumerate()
        .fold(0, |hash, (i, c)| hash | (u64::from(*c > median) << i))
}
#[cfg(test)]
mod tests;
