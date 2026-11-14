use super::*;
use crate::ops::ProgressSink;
use crate::remote_tests::MemServer;
use crate::vfs::{parse_ipc_path, Scheme};
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::mpsc;
use std::sync::OnceLock;

fn vp(path: &Path) -> VPath {
    VPath::local(path).unwrap()
}

fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn run(root: &Path, options: &SearchOptions) -> SearchResult {
    search(&VfsRouter::new(), &vp(root), options, &JobHandle::silent()).unwrap()
}

fn names(root: &Path, result: &SearchResult) -> Vec<String> {
    let mut found: Vec<String> = result
        .hits
        .iter()
        .map(|hit| {
            hit.path
                .path()
                .strip_prefix(root)
                .unwrap_or(hit.path.path())
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn by_name(pattern: &str) -> SearchOptions {
    SearchOptions {
        name: Some(pattern.into()),
        ..SearchOptions::default()
    }
}

fn by_text(text: &str) -> SearchOptions {
    SearchOptions {
        text: Some(text.into()),
        ..SearchOptions::default()
    }
}

fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "main.rs", b"fn main() {}\n");
    put(r, "Cargo.toml", b"[package]\n");
    put(r, "a.txt", b"alpha\n");
    put(r, "b.md", b"beta\n");
    put(r, "notes/todo.TXT", b"first\nsecond needle here\nthird\n");
    put(r, "notes/deep/x.rs", b"let x = 1;\n");
    put(r, "app.log", b"log\n");
    dir
}

fn zip_of(items: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in items {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn targz_of(items: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, data) in items {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn mode(path: &Path, bits: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(bits)).unwrap();
}

// ------------------------------------------------------------------ names

#[test]
fn glob_matches_whole_names_with_alternatives_and_classes() {
    let dir = sample();
    let r = dir.path();
    assert_eq!(
        names(r, &run(r, &by_name("*.rs"))),
        ["main.rs", "notes/deep/x.rs"]
    );
    assert_eq!(
        names(r, &run(r, &by_name("*.rs;*.toml"))),
        ["Cargo.toml", "main.rs", "notes/deep/x.rs"]
    );
    assert_eq!(names(r, &run(r, &by_name("?.txt"))), ["a.txt"]);
    assert_eq!(names(r, &run(r, &by_name("[a-b].*"))), ["a.txt", "b.md"]);
    assert!(names(r, &run(r, &by_name("[!a-b]*.md"))).is_empty());
    // the whole name has to match, not a part
    assert!(names(r, &run(r, &by_name("main"))).is_empty());
    // an unclosed class is a literal
    assert!(names(r, &run(r, &by_name("[x"))).is_empty());
}

#[test]
fn glob_special_characters_are_literal() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "a+b(1).txt", b"x");
    put(r, "aab1.txt", b"x");
    assert_eq!(names(r, &run(r, &by_name("a+b(1).txt"))), ["a+b(1).txt"]);
    assert_eq!(names(r, &run(r, &by_name("a\\+b*"))), ["a+b(1).txt"]);
}

#[test]
fn name_case_is_ignored_unless_asked() {
    let dir = sample();
    let r = dir.path();
    assert_eq!(
        names(r, &run(r, &by_name("*.txt"))),
        ["a.txt", "notes/todo.TXT"]
    );
    let strict = SearchOptions {
        name_case_sensitive: true,
        ..by_name("*.txt")
    };
    assert_eq!(names(r, &run(r, &strict)), ["a.txt"]);
}

#[test]
fn regex_name_search_and_bad_patterns() {
    let dir = sample();
    let r = dir.path();
    let regex = SearchOptions {
        name_mode: NameMode::Regex,
        ..by_name(r"^[a-z]+\.rs$")
    };
    assert_eq!(names(r, &run(r, &regex)), ["main.rs", "notes/deep/x.rs"]);
    // folders are hits for a pure name search
    assert_eq!(names(r, &run(r, &by_name("deep"))), ["notes/deep"]);
    let broken = SearchOptions {
        name_mode: NameMode::Regex,
        ..by_name("(")
    };
    assert!(matches!(
        search(&VfsRouter::new(), &vp(r), &broken, &JobHandle::silent()),
        Err(Error::InvalidInput(_))
    ));
    // neither a name nor a text
    assert!(matches!(
        search(
            &VfsRouter::new(),
            &vp(r),
            &SearchOptions::default(),
            &JobHandle::silent()
        ),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn non_recursive_stays_in_the_folder() {
    let dir = sample();
    let r = dir.path();
    let flat = SearchOptions {
        recursive: false,
        ..by_name("*.rs")
    };
    assert_eq!(names(r, &run(r, &flat)), ["main.rs"]);
}

// ------------------------------------------------------------------ text

#[test]
fn text_hit_has_line_number_and_excerpt() {
    let dir = sample();
    let r = dir.path();
    let result = run(r, &by_text("needle"));
    assert_eq!(result.hits.len(), 1);
    let hit = &result.hits[0];
    assert_eq!(hit.line, Some(2));
    assert_eq!(hit.excerpt.as_deref(), Some("second needle here"));
    assert_eq!((hit.match_start, hit.match_len), (7, 6));
    assert!(!result.incomplete);
    assert_eq!(result.total(), 1);
    assert!(result.files_searched >= 5);
}

#[test]
fn text_literal_is_not_a_pattern_and_case_is_ignored_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "f.txt", b"price (a.b) 5\nprice axb\nPRICE (A.B)\n");
    let result = run(r, &by_text("(a.b)"));
    let lines: Vec<_> = result.hits.iter().map(|h| h.line.unwrap()).collect();
    assert_eq!(lines, [1, 3]);
    let strict = SearchOptions {
        text_case_sensitive: true,
        ..by_text("(a.b)")
    };
    let lines: Vec<_> = run(r, &strict)
        .hits
        .iter()
        .map(|h| h.line.unwrap())
        .collect();
    assert_eq!(lines, [1]);
}

#[test]
fn text_regex() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "f.txt", b"foo1\nfooo22\nbar\nfo3\n");
    let options = SearchOptions {
        text_regex: true,
        ..by_text(r"fo+\d")
    };
    let lines: Vec<_> = run(r, &options)
        .hits
        .iter()
        .map(|h| h.line.unwrap())
        .collect();
    assert_eq!(lines, [1, 2, 4]);
}

#[test]
fn name_and_text_combine_and_crlf_and_last_line_without_newline() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "a.txt", b"x\r\nneedle\r\nend");
    put(r, "b.txt", b"nothing\nneedle at the end");
    put(r, "c.md", b"needle\n");
    let options = SearchOptions {
        name: Some("*.txt".into()),
        text: Some("needle".into()),
        ..SearchOptions::default()
    };
    let result = run(r, &options);
    assert_eq!(names(r, &result), ["a.txt", "b.txt"]);
    let a = result
        .hits
        .iter()
        .find(|h| h.path.path().ends_with("a.txt"))
        .unwrap();
    assert_eq!((a.line, a.excerpt.as_deref()), (Some(2), Some("needle")));
    let b = result
        .hits
        .iter()
        .find(|h| h.path.path().ends_with("b.txt"))
        .unwrap();
    assert_eq!(b.line, Some(2));
}

#[test]
fn binary_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let mut data = b"needle\n".to_vec();
    data.push(0);
    data.extend_from_slice(b"needle\n");
    put(r, "bin.dat", &data);
    put(r, "text.txt", b"needle\n");
    let result = run(r, &by_text("needle"));
    assert_eq!(names(r, &result), ["text.txt"]);
    assert_eq!(result.skipped_binary, 1);
    assert!(!result.incomplete);
}

#[test]
fn long_lines_are_shortened_around_the_match() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let line = format!("{}NEEDLE{}\n", "a".repeat(500), "b".repeat(500));
    put(r, "long.txt", line.as_bytes());
    let hit = run(r, &by_text("needle")).hits.remove(0);
    let excerpt = hit.excerpt.unwrap();
    assert!(excerpt.chars().count() <= MAX_EXCERPT_CHARS + 2);
    let shown: Vec<char> = excerpt.chars().collect();
    let found: String = shown[hit.match_start as usize..(hit.match_start + hit.match_len) as usize]
        .iter()
        .collect();
    assert_eq!(found, "NEEDLE");
    assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
}

#[test]
fn control_characters_in_the_excerpt_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "c.txt", b"a\x1b[31mneedle\x07\n");
    let hit = run(r, &by_text("needle")).hits.remove(0);
    assert!(!hit.excerpt.unwrap().chars().any(|c| c.is_control()));
}

#[test]
fn lines_beyond_the_line_limit_are_reported_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let mut data = vec![b'x'; MAX_LINE_BYTES + 100];
    data.extend_from_slice(b"needle\nneedle\n");
    put(r, "wide.txt", &data);
    let result = run(r, &by_text("needle"));
    // the first (cut) line does not show the needle, the second one does
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].line, Some(2));
    assert!(result.incomplete);
}

#[test]
fn size_and_date_filters() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "small.txt", b"hi");
    put(r, "big.txt", &[b'x'; 5000]);
    let size = SearchOptions {
        min_size: Some(1000),
        ..by_name("*.txt")
    };
    assert_eq!(names(r, &run(r, &size)), ["big.txt"]);
    let size = SearchOptions {
        max_size: Some(1000),
        ..by_name("*.txt")
    };
    assert_eq!(names(r, &run(r, &size)), ["small.txt"]);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let file = fs::File::options()
        .write(true)
        .open(r.join("small.txt"))
        .unwrap();
    file.set_modified(old).unwrap();
    drop(file);
    let before = SearchOptions {
        modified_before: Some(old + Duration::from_secs(10)),
        ..by_name("*.txt")
    };
    assert_eq!(names(r, &run(r, &before)), ["small.txt"]);
    let after = SearchOptions {
        modified_after: Some(old + Duration::from_secs(10)),
        ..by_name("*.txt")
    };
    assert_eq!(names(r, &run(r, &after)), ["big.txt"]);
}

// ------------------------------------------------------------------ safety

#[test]
fn links_are_not_followed_even_in_a_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "real/data.txt", b"needle\n");
    std::os::unix::fs::symlink("..", r.join("real/up")).unwrap();
    std::os::unix::fs::symlink(".", r.join("self")).unwrap();
    std::os::unix::fs::symlink("real/data.txt", r.join("alias.txt")).unwrap();
    let result = run(r, &by_text("needle"));
    // only the real file; neither the cycle nor the link to the file is entered or read
    assert_eq!(names(r, &result), ["real/data.txt"]);
    assert_eq!(result.hits.len(), 1);
    assert!(!result.incomplete);
    assert_eq!(result.dirs_visited, 2);
    // links can still be found by name, as links
    let result = run(r, &by_name("alias.txt"));
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].kind, EntryKind::Symlink);
}

#[test]
fn a_fifo_is_skipped_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().to_path_buf();
    put(&r, "ok.txt", b"needle\n");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        r.join("pipe.txt"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    let root = r.clone();
    std::thread::spawn(move || {
        let _ = sender.send(run(&root, &by_text("needle")));
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(20))
        .expect("the search must not block on a pipe");
    assert_eq!(names(&r, &result), ["ok.txt"]);
    assert_eq!(result.skipped_special, 1);
    assert!(!result.incomplete);
}

#[test]
fn an_unreadable_folder_makes_the_result_incomplete_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "open/a.txt", b"needle\n");
    put(r, "locked/b.txt", b"needle\n");
    mode(&r.join("locked"), 0);
    let result = run(r, &by_text("needle"));
    mode(&r.join("locked"), 0o755);
    if rustix::process::geteuid().is_root() {
        return; // root can read everything
    }
    assert_eq!(names(r, &result), ["open/a.txt"]);
    assert!(result.incomplete);
    assert!(
        result.issues.iter().any(|i| i.contains("locked")),
        "{:?}",
        result.issues
    );
}

#[test]
fn an_unreadable_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "secret.txt", b"needle\n");
    put(r, "ok.txt", b"needle\n");
    mode(&r.join("secret.txt"), 0);
    let result = run(r, &by_text("needle"));
    mode(&r.join("secret.txt"), 0o644);
    if rustix::process::geteuid().is_root() {
        return;
    }
    assert_eq!(names(r, &result), ["ok.txt"]);
    assert!(result.incomplete);
    assert!(result.issues.iter().any(|i| i.contains("secret.txt")));
}

#[test]
fn the_start_must_be_a_folder() {
    let dir = sample();
    let file = vp(&dir.path().join("a.txt"));
    assert!(matches!(
        search(
            &VfsRouter::new(),
            &file,
            &by_name("*"),
            &JobHandle::silent()
        ),
        Err(Error::NotADirectory(_))
    ));
    assert!(search(
        &VfsRouter::new(),
        &vp(&dir.path().join("missing")),
        &by_name("*"),
        &JobHandle::silent()
    )
    .is_err());
}

// ------------------------------------------------------------------ cancel and limits

#[test]
fn a_cancelled_search_is_marked() {
    let dir = sample();
    let job = JobHandle::silent();
    job.cancel();
    let result = search(&VfsRouter::new(), &vp(dir.path()), &by_name("*"), &job).unwrap();
    assert!(result.cancelled && result.incomplete);
    assert_eq!(result.stopped, Some(StopReason::Cancelled));
}

struct CancelAfter {
    seen: std::sync::atomic::AtomicUsize,
    after: usize,
    job: OnceLock<JobHandle>,
}

impl ProgressSink for CancelAfter {
    fn event(&self, _: ProgressEvent) {
        let seen = self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if seen >= self.after {
            if let Some(job) = self.job.get() {
                job.cancel();
            }
        }
    }
}

#[test]
fn cancelling_during_the_search_stops_it_with_partial_hits() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for i in 0..20 {
        put(r, &format!("d{i:02}/f.txt"), b"needle\n");
    }
    let sink = Arc::new(CancelAfter {
        seen: Default::default(),
        after: 4,
        job: OnceLock::new(),
    });
    let job = JobHandle::new(sink.clone());
    sink.job.set(job.clone()).ok();
    let result = search(&VfsRouter::new(), &vp(r), &by_text("needle"), &job).unwrap();
    assert!(result.cancelled && result.incomplete);
    assert!(!result.hits.is_empty() && result.hits.len() < 20);
}

fn many_matching_lines(r: &Path, count: usize) {
    let text: String = (0..count).map(|i| format!("needle {i}\n")).collect();
    put(r, "many.txt", text.as_bytes());
}

#[test]
fn the_hit_limit_is_reported_and_exact_fit_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    many_matching_lines(r, 5);
    let mut options = by_text("needle");
    options.limits.max_hits = 3;
    let result = run(r, &options);
    assert_eq!(result.hits.len(), 3);
    assert!(result.incomplete);
    assert_eq!(result.stopped, Some(StopReason::Hits));
    options.limits.max_hits = 5;
    let result = run(r, &options);
    assert_eq!(result.hits.len(), 5);
    assert!(!result.incomplete);
}

#[test]
fn the_per_file_hit_limit_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    many_matching_lines(r, 10);
    put(r, "other.txt", b"needle\n");
    let mut options = by_text("needle");
    options.limits.max_hits_per_file = 4;
    let result = run(r, &options);
    assert_eq!(result.hits.len(), 5);
    assert!(result.incomplete);
    assert!(result.issues.iter().any(|i| i.contains("many.txt")));
}

#[test]
fn the_file_limit_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for i in 0..10 {
        put(r, &format!("f{i}.txt"), b"x\n");
    }
    let mut options = by_name("*.txt");
    options.limits.max_files = 4;
    let result = run(r, &options);
    assert_eq!(result.hits.len(), 4);
    assert_eq!(result.stopped, Some(StopReason::Files));
    assert!(result.incomplete);
}

#[test]
fn the_byte_limit_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for i in 0..10 {
        put(r, &format!("f{i}.txt"), &[b'x'; 100]);
    }
    let mut options = by_text("needle");
    options.limits.max_bytes = 250;
    let result = run(r, &options);
    assert_eq!(result.stopped, Some(StopReason::Bytes));
    assert!(result.incomplete);
    assert!(result.bytes_read <= 250);
}

#[test]
fn a_file_over_the_per_file_limit_is_searched_in_part_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let mut data = vec![b'x'; 10_000];
    data.extend_from_slice(b"\nneedle\n");
    put(r, "big.txt", &data);
    let mut options = by_text("needle");
    options.limits.max_file_bytes = 4096;
    let result = run(r, &options);
    assert!(result.hits.is_empty());
    assert!(result.incomplete);
    assert!(result.issues.iter().any(|i| i.contains("big.txt")));
    options.limits.max_file_bytes = 1 << 20;
    assert_eq!(run(r, &options).hits.len(), 1);
}

#[test]
fn the_time_limit_is_reported() {
    let dir = sample();
    let mut options = by_name("*");
    options.limits.max_time = Some(Duration::ZERO);
    let result = run(dir.path(), &options);
    assert_eq!(result.stopped, Some(StopReason::Time));
    assert!(result.incomplete);
}

#[test]
fn the_depth_limit_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "a/b/c/deep.txt", b"needle\n");
    let mut options = by_text("needle");
    options.limits.max_depth = 2;
    let result = run(r, &options);
    assert!(result.hits.is_empty());
    assert!(result.incomplete);
}

// ------------------------------------------------------------------ evil patterns

#[test]
fn evil_patterns_are_fast_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "a.txt", format!("{}c\n", "a".repeat(60_000)).as_bytes());
    let started = Instant::now();
    // classic catastrophic backtracking: linear on the regex crate
    for pattern in [r"(a*)*b", r"(a|aa)+$", r"(x+x+)+y", r"(.*)*(.*)*z"] {
        let options = SearchOptions {
            text_regex: true,
            ..by_text(pattern)
        };
        let result = run(r, &options);
        assert!(result.hits.is_empty(), "{pattern}");
    }
    // huge repetition and deep nesting: refused at compile time
    for pattern in [
        "(a{1000}){1000}".to_string(),
        "((((a{100}){100}){100}){100})".to_string(),
        "(".repeat(200) + "a" + &")".repeat(200),
        "a".repeat(MAX_PATTERN_BYTES + 1),
    ] {
        let options = SearchOptions {
            text_regex: true,
            ..by_text(&pattern)
        };
        let outcome = search(&VfsRouter::new(), &vp(r), &options, &JobHandle::silent());
        assert!(
            matches!(outcome, Err(Error::InvalidInput(_))),
            "{pattern:.20}"
        );
    }
    let long_name = SearchOptions {
        name_mode: NameMode::Regex,
        ..by_name(&"a".repeat(MAX_PATTERN_BYTES + 1))
    };
    assert!(search(&VfsRouter::new(), &vp(r), &long_name, &JobHandle::silent()).is_err());
    assert!(started.elapsed() < Duration::from_secs(20));
}

// ------------------------------------------------------------------ archives

fn expect_listable(router: &VfsRouter, path: &VPath) {
    // the hit path round-trips through the IPC format and opens with `list`
    let text = format_ipc_path(path);
    let parsed = parse_ipc_path(&text).unwrap();
    assert_eq!(&parsed, path);
    let parent = parsed.parent().unwrap();
    let listing = router.vfs_for(&parent).unwrap().list(&parent).unwrap();
    let name = path.path().file_name().unwrap().to_string_lossy();
    assert!(listing.iter().any(|e| e.name == name), "{text}");
}

#[test]
fn hits_inside_a_zip_have_a_valid_archive_path() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::write(
        r.join("pack.zip"),
        zip_of(&[
            ("docs/readme.txt", b"one\ntwo\nneedle three\n"),
            ("docs/other.txt", b"nothing\n"),
        ]),
    )
    .unwrap();
    put(r, "plain.txt", b"needle\n");
    let router = VfsRouter::new();
    // without archives only the plain file is found
    let plain = search(&router, &vp(r), &by_text("needle"), &JobHandle::silent()).unwrap();
    assert_eq!(names(r, &plain), ["plain.txt"]);
    let options = SearchOptions {
        in_archives: true,
        ..by_text("needle")
    };
    let result = search(&router, &vp(r), &options, &JobHandle::silent()).unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(result.archives_searched, 1);
    let hit = result
        .hits
        .iter()
        .find(|h| h.path.outer().is_some())
        .unwrap();
    assert_eq!(hit.line, Some(3));
    assert_eq!(hit.path.path(), Path::new("/docs/readme.txt"));
    assert!(format_ipc_path(&hit.path).starts_with("archive:"));
    expect_listable(&router, &hit.path);
    assert!(!result.incomplete, "{:?}", result.issues);
}

#[test]
fn hits_inside_a_tar_gz_have_a_valid_archive_path() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::write(
        r.join("pack.tar.gz"),
        targz_of(&[
            ("src/lib.rs", b"fn a() {}\n// needle\n"),
            ("x.bin", b"\0\0needle"),
        ]),
    )
    .unwrap();
    let router = VfsRouter::new();
    let options = SearchOptions {
        in_archives: true,
        ..by_text("needle")
    };
    let result = search(&router, &vp(r), &options, &JobHandle::silent()).unwrap();
    assert_eq!(result.hits.len(), 1, "{:?}", result.issues);
    assert_eq!(result.hits[0].line, Some(2));
    assert_eq!(result.skipped_binary, 1);
    expect_listable(&router, &result.hits[0].path);
}

#[test]
fn name_hits_in_archives() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::write(
        r.join("pack.zip"),
        zip_of(&[
            ("a/one.rs", b"1"),
            ("a/two.txt", b"22"),
            ("three.rs", b"333"),
        ]),
    )
    .unwrap();
    let options = SearchOptions {
        in_archives: true,
        ..by_name("*.rs")
    };
    let result = run(r, &options);
    assert_eq!(result.hits.len(), 2);
    assert!(result.hits.iter().all(|h| h.path.outer().is_some()));
}

#[test]
fn nested_archives_need_enough_depth() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let inner = zip_of(&[("deep.txt", b"needle\n")]);
    fs::write(r.join("outer.zip"), zip_of(&[("inner.zip", &inner)])).unwrap();
    let mut options = SearchOptions {
        in_archives: true,
        archive_depth: 1,
        ..by_text("needle")
    };
    assert!(run(r, &options).hits.is_empty());
    options.archive_depth = 2;
    let router = VfsRouter::new();
    let result = search(&router, &vp(r), &options, &JobHandle::silent()).unwrap();
    assert_eq!(result.hits.len(), 1, "{:?}", result.issues);
    assert_eq!(result.hits[0].path.archive_depth(), 2);
    expect_listable(&router, &result.hits[0].path);
}

#[test]
fn a_search_can_start_inside_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::write(
        r.join("pack.zip"),
        zip_of(&[("docs/a.txt", b"needle\n"), ("other/b.txt", b"needle\n")]),
    )
    .unwrap();
    let start = VPath::archive(vp(&r.join("pack.zip")), "/docs").unwrap();
    let result = search(
        &VfsRouter::new(),
        &start,
        &by_text("needle"),
        &JobHandle::silent(),
    )
    .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].path.path(), Path::new("/docs/a.txt"));
}

#[test]
fn the_archive_budget_makes_the_result_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let big: String = (0..40_000)
        .map(|i| format!("line {i} of filler text\n"))
        .collect();
    fs::write(
        r.join("a.zip"),
        zip_of(&[("big.txt", big.as_bytes()), ("small.txt", b"needle\n")]),
    )
    .unwrap();
    fs::write(r.join("b.zip"), zip_of(&[("x.txt", b"needle\n")])).unwrap();
    put(r, "plain.txt", b"needle\n");
    let router = VfsRouter::new();
    let mut options = SearchOptions {
        in_archives: true,
        ..by_text("needle")
    };
    options.limits.archive_bytes = 10_000;
    let result = search(&router, &vp(r), &options, &JobHandle::silent()).unwrap();
    assert!(result.incomplete);
    assert!(
        result.issues.iter().any(|i| i.contains("Archiv")),
        "{:?}",
        result.issues
    );
    // local files are still searched
    assert!(result
        .hits
        .iter()
        .any(|h| h.path.path().ends_with("plain.txt")));
    // the cached archive instances have their normal budget again afterwards
    let again = search(
        &router,
        &vp(r),
        &SearchOptions {
            in_archives: true,
            ..by_text("needle")
        },
        &JobHandle::silent(),
    )
    .unwrap();
    assert!(!again.incomplete, "{:?}", again.issues);
    assert_eq!(again.hits.len(), 3);
}

#[test]
fn a_damaged_archive_is_reported_and_other_files_still_found() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let mut zip = zip_of(&[("a.txt", b"needle\n")]);
    zip.truncate(zip.len() / 2);
    fs::write(r.join("broken.zip"), zip).unwrap();
    put(r, "plain.txt", b"needle\n");
    let result = run(
        r,
        &SearchOptions {
            in_archives: true,
            ..by_text("needle")
        },
    );
    assert!(names(r, &result).contains(&"plain.txt".to_string()));
    assert!(result.incomplete);
}

#[test]
fn a_plain_gz_that_is_no_archive_is_just_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "notes.gz", b"needle in a file that is no archive\n");
    let result = run(
        r,
        &SearchOptions {
            in_archives: true,
            ..by_text("needle")
        },
    );
    assert_eq!(names(r, &result), ["notes.gz"]);
    assert!(!result.incomplete);
}

// ------------------------------------------------------------------ remote

fn remote_fixture() -> (VfsRouter, Arc<MemServer>) {
    let router = VfsRouter::new();
    let server = MemServer::new(Scheme::Sftp, "srv");
    server.dir("/data");
    server.put("/data/a.txt", b"needle\n", 1);
    server.put("/data/big.txt", &vec![b'x'; 3 << 20], 1);
    router.register_remote("srv", server.clone()).unwrap();
    (router, server)
}

#[test]
fn servers_are_only_searched_when_asked_for() {
    let (router, server) = remote_fixture();
    let start = server.vp("/data");
    let outcome = search(&router, &start, &by_text("needle"), &JobHandle::silent());
    assert!(matches!(outcome, Err(Error::Unsupported(_))));
    let options = SearchOptions {
        allow_remote: true,
        ..by_text("needle")
    };
    let result = search(&router, &start, &options, &JobHandle::silent()).unwrap();
    assert_eq!(result.hits.len(), 1);
}

#[test]
fn server_searches_run_with_a_low_budget() {
    let (router, server) = remote_fixture();
    let mut options = SearchOptions {
        allow_remote: true,
        in_archives: true,
        ..by_text("needle")
    };
    // whatever the caller allows, a server read is cut at the low limits
    options.limits.max_file_bytes = u64::MAX;
    options.limits.max_bytes = u64::MAX;
    options.limits.max_time = None;
    let result = search(&router, &server.vp("/data"), &options, &JobHandle::silent()).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert!(result.incomplete);
    assert!(
        result.issues.iter().any(|i| i.contains("big.txt")),
        "{:?}",
        result.issues
    );
    assert!(result.bytes_read <= REMOTE_MAX_BYTES);
    assert_eq!(result.archives_searched, 0);
    let limits = SearchLimits::default().for_remote();
    assert!(limits.max_files <= 2_000 && limits.max_time.is_some());
}

#[test]
fn plugin_locations_count_as_remote() {
    let router = VfsRouter::new();
    let plugin = MemServer::new(Scheme::Plugin, "plug");
    plugin.put("/a.txt", b"needle\n", 1);
    router.register_plugin("plug", plugin.clone()).unwrap();
    let start = plugin.vp("/");
    assert!(matches!(
        search(&router, &start, &by_text("needle"), &JobHandle::silent()),
        Err(Error::Unsupported(_))
    ));
    let options = SearchOptions {
        allow_remote: true,
        ..by_text("needle")
    };
    assert_eq!(
        search(&router, &start, &options, &JobHandle::silent())
            .unwrap()
            .hits
            .len(),
        1
    );
}
