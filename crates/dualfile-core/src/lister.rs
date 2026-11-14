//! Lister support (phase 2): block-wise reading, searching and type detection for files of any
//! size. Nothing here loads a whole file; every limit is enforced here, never by the caller.

use crate::error::{Error, Result};
use crate::vfs::{VPath, Vfs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// Largest block `read_block` returns (longer requests are shortened).
pub const MAX_BLOCK: usize = 1 << 20;
/// Bytes one `find_in_file` call scans at most; the caller continues from the returned offset.
pub const MAX_SEARCH_BYTES: u64 = 256 << 20;
/// Longest search needle in bytes.
pub const MAX_NEEDLE: usize = 4096;
/// Bytes read for type detection.
const SNIFF_BYTES: usize = 4096;
/// Step of the search loop (also the cancellation granularity).
const SEARCH_CHUNK: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Text,
    Binary,
    Image,
    Audio,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Detection {
    pub kind: FileKind,
    pub mime: &'static str,
}

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub size: u64,
    pub kind: FileKind,
    pub modified: Option<SystemTime>,
    pub mime_guess: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchOutcome {
    Found(u64),
    NotFound,
    /// The scan limit was reached without a hit; call again with this value as `start`.
    Continue(u64),
}

/// Size, modification time and detected type of a regular file (links are followed).
pub fn file_info(vfs: &dyn Vfs, path: &VPath) -> Result<FileInfo> {
    let stat = vfs.stat_file(path)?;
    let head = vfs.read_range(path, 0, SNIFF_BYTES)?;
    let name = path
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let detected = detect_kind(&head, &name);
    Ok(FileInfo {
        size: stat.size,
        kind: detected.kind,
        modified: stat.modified,
        mime_guess: detected.mime,
    })
}

/// Up to `len` bytes at `offset` (`len` is cut to `MAX_BLOCK`; shorter at the end of the
/// file, empty behind it).
pub fn read_block(vfs: &dyn Vfs, path: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
    let len = len.min(MAX_BLOCK);
    if len == 0 {
        // Still validates the path (type and existence), like any other read.
        vfs.stat_file(path)?;
        return Ok(Vec::new());
    }
    vfs.read_range(path, offset, len)
}

/// Searches `needle` in the file, scanning at most `MAX_SEARCH_BYTES` per call.
///
/// `Forward` finds the first match starting at `start` or later; `Backward` the last match
/// starting before `start` (pass the file size to begin at the end). A match found in the
/// scanned part may run over the scan limit, so matches over block borders are found.
/// Case-insensitive matching folds ASCII letters only (the file is not decoded).
pub fn find_in_file(
    vfs: &dyn Vfs,
    path: &VPath,
    needle: &[u8],
    start: u64,
    direction: Direction,
    ignore_case: bool,
    cancel: &AtomicBool,
) -> Result<SearchOutcome> {
    find_limited(
        vfs,
        path,
        needle,
        start,
        direction,
        ignore_case,
        cancel,
        MAX_SEARCH_BYTES,
    )
}

#[allow(clippy::too_many_arguments)]
fn find_limited(
    vfs: &dyn Vfs,
    path: &VPath,
    needle: &[u8],
    start: u64,
    direction: Direction,
    ignore_case: bool,
    cancel: &AtomicBool,
    budget: u64,
) -> Result<SearchOutcome> {
    if needle.is_empty() || needle.len() > MAX_NEEDLE {
        return Err(Error::InvalidInput("Der Suchtext ist leer oder zu lang"));
    }
    let size = vfs.stat_file(path)?.size;
    let folded: Vec<u8> = if ignore_case {
        needle.to_ascii_lowercase()
    } else {
        needle.to_vec()
    };
    let overlap = needle.len() - 1;
    let mut scanned = 0u64;
    // Forward: `edge` is the next offset to look at. Backward: matches must start below `edge`.
    let mut edge = match direction {
        Direction::Forward => start,
        Direction::Backward => start.min(size),
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let step = SEARCH_CHUNK.min(budget - scanned);
        let (from, span) = match direction {
            Direction::Forward => (edge, step),
            Direction::Backward => {
                let from = edge.saturating_sub(step);
                (from, edge - from)
            }
        };
        let data = vfs.read_range(path, from, (span as usize) + overlap)?;
        let hit = match direction {
            Direction::Forward => find_first(&data, &folded, ignore_case, span as usize),
            Direction::Backward => find_last(&data, &folded, ignore_case, span as usize),
        };
        if let Some(index) = hit {
            return Ok(SearchOutcome::Found(from + index as u64));
        }
        scanned += span;
        match direction {
            Direction::Forward => {
                edge = from + span;
                if edge >= size || data.len() < (span as usize) + overlap {
                    return Ok(SearchOutcome::NotFound);
                }
            }
            Direction::Backward => {
                edge = from;
                if edge == 0 {
                    return Ok(SearchOutcome::NotFound);
                }
            }
        }
        if scanned >= budget {
            return Ok(SearchOutcome::Continue(edge));
        }
    }
}

fn matches_at(data: &[u8], at: usize, needle: &[u8], ignore_case: bool) -> bool {
    let window = &data[at..at + needle.len()];
    if ignore_case {
        window.eq_ignore_ascii_case(needle)
    } else {
        window == needle
    }
}

/// First match whose start index is below `limit`.
fn find_first(data: &[u8], needle: &[u8], ignore_case: bool, limit: usize) -> Option<usize> {
    let last = data.len().checked_sub(needle.len())?;
    let newest = last.min(limit.checked_sub(1)?);
    (0..=newest).find(|&at| matches_at(data, at, needle, ignore_case))
}

/// Last match whose start index is below `limit`.
fn find_last(data: &[u8], needle: &[u8], ignore_case: bool, limit: usize) -> Option<usize> {
    let last = data.len().checked_sub(needle.len())?;
    let newest = last.min(limit.checked_sub(1)?);
    (0..=newest)
        .rev()
        .find(|&at| matches_at(data, at, needle, ignore_case))
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn det(kind: FileKind, mime: &'static str) -> Detection {
    Detection { kind, mime }
}

/// Type by signature (first bytes), the file name only breaks ties for formats without a
/// reliable signature. Text means: no NUL byte and few control characters.
pub fn detect_kind(first_bytes: &[u8], name: &str) -> Detection {
    let b = first_bytes;
    let ext = extension(name);
    let ext = ext.as_str();
    let starts = |sig: &[u8]| b.starts_with(sig);
    let at = |pos: usize, sig: &[u8]| b.len() >= pos + sig.len() && &b[pos..pos + sig.len()] == sig;

    if starts(b"\x89PNG\r\n\x1a\n") {
        return det(FileKind::Image, "image/png");
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return det(FileKind::Image, "image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return det(FileKind::Image, "image/gif");
    }
    if starts(b"RIFF") && at(8, b"WEBP") {
        return det(FileKind::Image, "image/webp");
    }
    if starts(b"RIFF") && at(8, b"WAVE") {
        return det(FileKind::Audio, "audio/wav");
    }
    if starts(b"BM") && ext == "bmp" {
        return det(FileKind::Image, "image/bmp");
    }
    if starts(&[0, 0, 1, 0]) && ext == "ico" {
        return det(FileKind::Image, "image/x-icon");
    }
    if starts(b"ID3") || starts(b"fLaC") || starts(b"OggS") {
        let mime = if starts(b"fLaC") {
            "audio/flac"
        } else if starts(b"OggS") {
            "audio/ogg"
        } else {
            "audio/mpeg"
        };
        return det(FileKind::Audio, mime);
    }
    // Bare MPEG audio frame sync (FF Ex) is weak evidence: only together with the extension.
    if b.len() >= 2 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0 && matches!(ext, "mp3" | "mp2") {
        return det(FileKind::Audio, "audio/mpeg");
    }
    if at(4, b"ftyp") {
        return if at(8, b"M4A ") {
            det(FileKind::Audio, "audio/mp4")
        } else {
            det(FileKind::Video, "video/mp4")
        };
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return det(FileKind::Video, "video/webm");
    }
    if ext == "svg" && is_text(b) {
        return det(FileKind::Image, "image/svg+xml");
    }
    if starts(&[0xEF, 0xBB, 0xBF]) || starts(&[0xFF, 0xFE]) || starts(&[0xFE, 0xFF]) {
        return det(FileKind::Text, "text/plain");
    }
    if is_text(b) {
        return det(FileKind::Text, "text/plain");
    }
    det(FileKind::Binary, "application/octet-stream")
}

fn is_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let control = bytes
        .iter()
        .filter(|&&c| (c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)) || c == 0x7F)
        .count();
    control * 10 <= bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::LocalVfs;
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    fn vp(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    fn file_with(dir: &Path, name: &str, data: &[u8]) -> VPath {
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        vp(&path)
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn numbers(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    // ------------------------------------------------------------ read_block

    #[test]
    fn read_block_at_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"0123456789");
        assert_eq!(read_block(&LocalVfs, &path, 0, 4).unwrap(), b"0123");
    }

    #[test]
    fn read_block_in_the_middle() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"0123456789");
        assert_eq!(read_block(&LocalVfs, &path, 3, 4).unwrap(), b"3456");
    }

    #[test]
    fn read_block_at_the_end_is_shorter() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"0123456789");
        assert_eq!(read_block(&LocalVfs, &path, 8, 4).unwrap(), b"89");
    }

    #[test]
    fn read_block_beyond_the_end_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"0123456789");
        assert!(read_block(&LocalVfs, &path, 10, 4).unwrap().is_empty());
        assert!(read_block(&LocalVfs, &path, 1 << 40, 4).unwrap().is_empty());
    }

    #[test]
    fn read_block_of_length_zero_is_empty_but_checks_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"abc");
        assert!(read_block(&LocalVfs, &path, 0, 0).unwrap().is_empty());
        let missing = vp(&dir.path().join("nope"));
        assert!(matches!(
            read_block(&LocalVfs, &missing, 0, 0),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn read_block_is_limited_to_one_mebibyte() {
        let dir = tempfile::tempdir().unwrap();
        let data = numbers(MAX_BLOCK + 5000);
        let path = file_with(dir.path(), "big", &data);
        let block = read_block(&LocalVfs, &path, 100, usize::MAX).unwrap();
        assert_eq!(block.len(), MAX_BLOCK);
        assert_eq!(block, data[100..100 + MAX_BLOCK]);
    }

    #[test]
    fn read_range_default_implementation_skips_by_reading() {
        // A Vfs that only knows `open_read`/`stat`: exercises the trait's default `read_range`.
        struct Plain(LocalVfs);
        impl Vfs for Plain {
            fn list(&self, p: &VPath) -> Result<Vec<crate::vfs::Entry>> {
                self.0.list(p)
            }
            fn stat(&self, p: &VPath) -> Result<crate::vfs::Entry> {
                self.0.stat(p)
            }
            fn open_read(&self, p: &VPath) -> Result<Box<dyn std::io::Read + Send>> {
                self.0.open_read(p)
            }
            fn create_write(
                &self,
                p: &VPath,
                m: crate::vfs::WriteMode,
            ) -> Result<Box<dyn crate::vfs::WriteFinish>> {
                self.0.create_write(p, m)
            }
            fn mkdir(&self, p: &VPath) -> Result<()> {
                self.0.mkdir(p)
            }
            fn rename(&self, a: &VPath, b: &VPath, m: crate::vfs::ReplaceMode) -> Result<()> {
                self.0.rename(a, b, m)
            }
            fn read_link(&self, p: &VPath) -> Result<String> {
                self.0.read_link(p)
            }
            fn create_symlink(&self, t: &str, a: &VPath) -> Result<()> {
                self.0.create_symlink(t, a)
            }
            fn trash(&self, p: &VPath) -> Result<()> {
                self.0.trash(p)
            }
            fn remove_permanent(&self, p: &VPath) -> Result<()> {
                self.0.remove_permanent(p)
            }
            fn capabilities(&self) -> crate::vfs::Caps {
                self.0.capabilities()
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"0123456789");
        let vfs = Plain(LocalVfs);
        assert_eq!(vfs.read_range(&path, 3, 4).unwrap(), b"3456");
        assert_eq!(vfs.read_range(&path, 8, 9).unwrap(), b"89");
        assert!(vfs.read_range(&path, 50, 9).unwrap().is_empty());
        let dir_path = vp(dir.path());
        assert!(matches!(
            vfs.read_range(&dir_path, 0, 1),
            Err(Error::IsADirectory(_))
        ));
    }

    // ------------------------------------------------------------ special files

    #[test]
    fn a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = vp(dir.path());
        assert!(matches!(
            read_block(&LocalVfs, &path, 0, 10),
            Err(Error::IsADirectory(_))
        ));
        assert!(matches!(
            file_info(&LocalVfs, &path),
            Err(Error::IsADirectory(_))
        ));
    }

    #[test]
    fn a_symlink_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = file_with(dir.path(), "target.txt", b"hello world");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        let link = vp(&link);
        assert_eq!(read_block(&LocalVfs, &link, 6, 5).unwrap(), b"world");
        assert_eq!(file_info(&LocalVfs, &link).unwrap().size, 11);
    }

    #[test]
    fn a_broken_symlink_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("broken");
        std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
        assert!(matches!(
            read_block(&LocalVfs, &vp(&link), 0, 1),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let path = vp(&fifo);
        // A blocking open or read would hang forever; the thread plus timeout makes that a failure.
        let (sender, receiver) = mpsc::channel();
        let for_thread = path.clone();
        std::thread::spawn(move || {
            let block = read_block(&LocalVfs, &for_thread, 0, 16).map(|_| ());
            let info = file_info(&LocalVfs, &for_thread).map(|_| ());
            let search = find_in_file(
                &LocalVfs,
                &for_thread,
                b"x",
                0,
                Direction::Forward,
                false,
                &never(),
            )
            .map(|_| ());
            let _ = sender.send((block, info, search));
        });
        let (block, info, search) = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("opening a FIFO must not block");
        for result in [block, info, search] {
            assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
        }
    }

    #[test]
    fn a_fifo_behind_a_link_is_refused_too() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&fifo, &link).unwrap();
        let (sender, receiver) = mpsc::channel();
        let path = vp(&link);
        std::thread::spawn(move || {
            let _ = sender.send(read_block(&LocalVfs, &path, 0, 16).map(|_| ()));
        });
        let result = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_unix_socket_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let result = read_block(&LocalVfs, &vp(&socket), 0, 16);
        assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
    }

    #[test]
    fn a_regular_file_is_left_in_blocking_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"abc");
        let (file, _) = crate::vfs::open_regular(path.path()).unwrap();
        let flags = rustix::fs::fcntl_getfl(&file).unwrap();
        assert!(!flags.contains(rustix::fs::OFlags::NONBLOCK));
    }

    // ------------------------------------------------------------ find_in_file

    fn find(path: &VPath, needle: &[u8], start: u64, dir: Direction, ic: bool) -> SearchOutcome {
        find_in_file(&LocalVfs, path, needle, start, dir, ic, &never()).unwrap()
    }

    #[test]
    fn find_a_match_across_a_block_border() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![b'a'; 2 * SEARCH_CHUNK as usize + 100];
        let at = SEARCH_CHUNK as usize - 3; // "NEEDLE" straddles the first border
        data[at..at + 6].copy_from_slice(b"NEEDLE");
        let path = file_with(dir.path(), "f", &data);
        assert_eq!(
            find(&path, b"NEEDLE", 0, Direction::Forward, false),
            SearchOutcome::Found(at as u64)
        );
        assert_eq!(
            find(
                &path,
                b"NEEDLE",
                data.len() as u64,
                Direction::Backward,
                false
            ),
            SearchOutcome::Found(at as u64)
        );
    }

    #[test]
    fn find_forward_continues_after_the_start_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"ab ab ab");
        assert_eq!(
            find(&path, b"ab", 0, Direction::Forward, false),
            SearchOutcome::Found(0)
        );
        assert_eq!(
            find(&path, b"ab", 1, Direction::Forward, false),
            SearchOutcome::Found(3)
        );
        assert_eq!(
            find(&path, b"ab", 7, Direction::Forward, false),
            SearchOutcome::NotFound
        );
    }

    #[test]
    fn find_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", &numbers(3_000_000));
        assert_eq!(
            find(&path, b"no such text", 0, Direction::Forward, false),
            SearchOutcome::NotFound
        );
        assert_eq!(
            find(
                &path,
                b"no such text",
                3_000_000,
                Direction::Backward,
                false
            ),
            SearchOutcome::NotFound
        );
    }

    #[test]
    fn find_respects_case_unless_told_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"xx Hello World");
        assert_eq!(
            find(&path, b"hello", 0, Direction::Forward, false),
            SearchOutcome::NotFound
        );
        assert_eq!(
            find(&path, b"hello", 0, Direction::Forward, true),
            SearchOutcome::Found(3)
        );
        assert_eq!(
            find(&path, b"WORLD", 14, Direction::Backward, true),
            SearchOutcome::Found(9)
        );
    }

    #[test]
    fn find_backward_takes_the_last_match_before_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"ab ab ab");
        assert_eq!(
            find(&path, b"ab", 8, Direction::Backward, false),
            SearchOutcome::Found(6)
        );
        assert_eq!(
            find(&path, b"ab", 6, Direction::Backward, false),
            SearchOutcome::Found(3)
        );
        assert_eq!(
            find(&path, b"ab", 1, Direction::Backward, false),
            SearchOutcome::Found(0)
        );
        assert_eq!(
            find(&path, b"ab", 0, Direction::Backward, false),
            SearchOutcome::NotFound
        );
        // A start behind the end of the file is fine, too.
        assert_eq!(
            find(&path, b"ab", 1 << 30, Direction::Backward, false),
            SearchOutcome::Found(6)
        );
    }

    #[test]
    fn find_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", &vec![0u8; 3 << 20]);
        let cancel = AtomicBool::new(true);
        let result = find_in_file(
            &LocalVfs,
            &path,
            b"x",
            0,
            Direction::Forward,
            false,
            &cancel,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn find_rejects_empty_and_oversized_needles() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "f", b"abc");
        for needle in [&b""[..], &vec![b'a'; MAX_NEEDLE + 1][..]] {
            let result = find_in_file(
                &LocalVfs,
                &path,
                needle,
                0,
                Direction::Forward,
                false,
                &never(),
            );
            assert!(matches!(result, Err(Error::InvalidInput(_))));
        }
    }

    #[test]
    fn find_stops_at_the_scan_limit_and_continues_from_the_returned_offset() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![b'.'; 5 * SEARCH_CHUNK as usize];
        let hit = 4 * SEARCH_CHUNK as usize + 10;
        data[hit..hit + 3].copy_from_slice(b"XYZ");
        let path = file_with(dir.path(), "f", &data);
        let budget = 2 * SEARCH_CHUNK;
        let run = |start, direction| {
            find_limited(
                &LocalVfs,
                &path,
                b"XYZ",
                start,
                direction,
                false,
                &never(),
                budget,
            )
            .unwrap()
        };
        let first = run(0, Direction::Forward);
        assert_eq!(first, SearchOutcome::Continue(budget));
        let second = run(budget, Direction::Forward);
        assert_eq!(second, SearchOutcome::Continue(2 * budget));
        assert_eq!(
            run(2 * budget, Direction::Forward),
            SearchOutcome::Found(hit as u64)
        );

        let end = data.len() as u64;
        let first = run(end, Direction::Backward);
        assert_eq!(first, SearchOutcome::Found(hit as u64));
        // Searching backwards from the hit itself finds nothing before it, but needs three rounds to be sure.
        let from = hit as u64;
        assert_eq!(
            run(from, Direction::Backward),
            SearchOutcome::Continue(from - budget)
        );
        assert_eq!(
            run(from - budget, Direction::Backward),
            SearchOutcome::Continue(10)
        );
        assert_eq!(run(10, Direction::Backward), SearchOutcome::NotFound);
    }

    #[test]
    fn the_public_scan_limit_is_256_mebibytes() {
        assert_eq!(MAX_SEARCH_BYTES, 256 * 1024 * 1024);
        assert_eq!(MAX_BLOCK, 1024 * 1024);
    }

    // ------------------------------------------------------------ detect_kind

    fn kind(bytes: &[u8], name: &str) -> (FileKind, &'static str) {
        let d = detect_kind(bytes, name);
        (d.kind, d.mime)
    }

    #[test]
    fn detect_png_jpeg_gif_webp() {
        assert_eq!(
            kind(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", "x"),
            (FileKind::Image, "image/png")
        );
        assert_eq!(
            kind(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10], "noext"),
            (FileKind::Image, "image/jpeg")
        );
        assert_eq!(
            kind(b"GIF89a\x01\0", "a.dat"),
            (FileKind::Image, "image/gif")
        );
        assert_eq!(kind(b"GIF87a\x01\0", "a"), (FileKind::Image, "image/gif"));
        assert_eq!(
            kind(b"RIFF\x24\0\0\0WEBPVP8 ", "a"),
            (FileKind::Image, "image/webp")
        );
    }

    #[test]
    fn detect_mp3_mp4_and_other_media() {
        assert_eq!(
            kind(b"ID3\x04\0\0\0\0", "song"),
            (FileKind::Audio, "audio/mpeg")
        );
        assert_eq!(
            kind(&[0xFF, 0xFB, 0x90, 0x00], "song.mp3"),
            (FileKind::Audio, "audio/mpeg")
        );
        // Frame sync bytes without the matching extension are not trusted.
        assert_eq!(
            kind(&[0xFF, 0xFB, 0x90, 0x00, 0], "x.bin").0,
            FileKind::Binary
        );
        assert_eq!(
            kind(b"\0\0\0\x18ftypisom\0\0\0\0", "clip"),
            (FileKind::Video, "video/mp4")
        );
        assert_eq!(
            kind(b"\0\0\0\x18ftypM4A \0\0\0\0", "a"),
            (FileKind::Audio, "audio/mp4")
        );
        assert_eq!(
            kind(&[0x1A, 0x45, 0xDF, 0xA3, 1], "v"),
            (FileKind::Video, "video/webm")
        );
        assert_eq!(kind(b"OggS\0\x02", "a"), (FileKind::Audio, "audio/ogg"));
        assert_eq!(kind(b"fLaC\0\0", "a"), (FileKind::Audio, "audio/flac"));
        assert_eq!(
            kind(b"RIFF\0\0\0\0WAVEfmt ", "a"),
            (FileKind::Audio, "audio/wav")
        );
    }

    #[test]
    fn detect_svg_only_with_extension_and_text() {
        let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(kind(svg, "a.svg"), (FileKind::Image, "image/svg+xml"));
        assert_eq!(kind(svg, "a.txt").0, FileKind::Text);
    }

    #[test]
    fn detect_utf8_text_and_umlauts() {
        assert_eq!(
            kind(b"Hello\nWorld\n", "a.txt"),
            (FileKind::Text, "text/plain")
        );
        assert_eq!(kind("Grüße, Größe\n".as_bytes(), "a").0, FileKind::Text);
        assert_eq!(kind(b"", "empty").0, FileKind::Text);
        assert_eq!(kind(b"\xEF\xBB\xBFabc", "a").0, FileKind::Text);
    }

    #[test]
    fn detect_utf16_with_bom_as_text() {
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Hallo".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("Hallo".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        // UTF-16 text contains NUL bytes; only the BOM makes it text.
        assert_eq!(kind(&le, "a.txt").0, FileKind::Text);
        assert_eq!(kind(&be, "a.txt").0, FileKind::Text);
        let no_bom: Vec<u8> = "Hallo".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(kind(&no_bom, "a.txt").0, FileKind::Binary);
    }

    #[test]
    fn detect_binary() {
        assert_eq!(
            kind(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0], "prog"),
            (FileKind::Binary, "application/octet-stream")
        );
        assert_eq!(kind(&numbers(500), "x.bin").0, FileKind::Binary);
        assert_eq!(kind(&[1, 2, 3, 4, 5, 6, 7, 8], "x.txt").0, FileKind::Binary);
    }

    #[test]
    fn file_info_reports_size_kind_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), "pic.png", b"\x89PNG\r\n\x1a\nrest");
        let info = file_info(&LocalVfs, &path).unwrap();
        assert_eq!(info.size, 12);
        assert_eq!(info.kind, FileKind::Image);
        assert_eq!(info.mime_guess, "image/png");
        assert!(info.modified.is_some());
    }
}
