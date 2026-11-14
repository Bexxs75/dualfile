use super::*;
use std::fs;
fn pattern(side: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::from_fn(side, side, |x, y| {
        let x = x * 128 / side;
        let y = y * 128 / side;
        let value = if (x as i32 - 41).pow(2) + (y as i32 - 53).pow(2) < 28 * 28 {
            220
        } else if x > 75 && y > 40 && y < 109 {
            160
        } else {
            20 + (x + y) / 4
        };
        image::Rgb([value as u8; 3])
    }))
}
fn other() -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::from_fn(128, 128, |x, y| {
        image::Rgb([((x * 19 + y * 31 + x * y) % 256) as u8; 3])
    }))
}
fn opts() -> SimilarOptions {
    SimilarOptions {
        min_size: 0,
        ..Default::default()
    }
}
fn run(path: &Path) -> SimilarResult {
    find_similar_images(
        &[path.to_path_buf()],
        &opts(),
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap()
}
fn group(hashes: &[u64], distance: u32) -> Grouped {
    group_hashes(hashes, distance, &JobHandle::silent(), &mut |_| {})
}
#[test]
fn golden_stable_and_distinct() {
    let a = phash(&pattern(128));
    assert_eq!(a, phash(&pattern(128)));
    assert!((a ^ phash(&other())).count_ones() > 16);
    assert!((a ^ phash(&pattern(128).fliph())).count_ones() > 6);
    assert_eq!(a, 0xe3231d13fcfc2261);
}
#[test]
fn resized_jpeg_groups_but_other_and_mirror_do_not() {
    let d = tempfile::tempdir().unwrap();
    pattern(128).save(d.path().join("a.PNG")).unwrap();
    pattern(128)
        .resize_exact(64, 64, image::imageops::FilterType::Triangle)
        .save(d.path().join("b.jpg"))
        .unwrap();
    other().save(d.path().join("c.png")).unwrap();
    pattern(128).fliph().save(d.path().join("d.png")).unwrap();
    let r = run(d.path());
    assert!(!r.incomplete, "{:?}", r.skipped);
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].entries.len(), 2);
    assert_eq!(r.groups[0].entries[0].width, 128);
    assert_eq!(r.groups[0].entries[1].width, 64);
}
#[test]
fn all_five_formats_are_decoded_and_unknown_extensions_ignored() {
    let d = tempfile::tempdir().unwrap();
    for ext in ["png", "jpg", "bmp", "gif", "webp"] {
        pattern(128)
            .save(d.path().join(format!("image.{ext}")))
            .unwrap();
    }
    fs::write(d.path().join("ignored.tiff"), b"bad").unwrap();
    let r = run(d.path());
    assert!(!r.incomplete, "{:?}", r.skipped);
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].entries.len(), 5);
    assert!(r.groups[0]
        .entries
        .iter()
        .all(|e| e.hash_hex == format!("{:016x}", e.hash)));
}
#[test]
fn threshold_transitivity_and_invalid_options() {
    assert!(group(&[0, 63], 0).components.is_empty());
    assert_eq!(group(&[0, 63], 6).components, vec![vec![0, 1]]);
    assert!(group(&[0, 127], 6).components.is_empty());
    assert_eq!(group(&[0, 63, 4095], 6).components, vec![vec![0, 1, 2]]);
    let d = tempfile::tempdir().unwrap();
    for options in [
        SimilarOptions {
            max_distance: 17,
            ..opts()
        },
        SimilarOptions {
            max_entries: 0,
            ..opts()
        },
    ] {
        assert!(matches!(
            find_similar_images(
                &[d.path().into()],
                &options,
                &JobHandle::silent(),
                &mut |_| {}
            ),
            Err(Error::InvalidInput(_))
        ));
    }
}
#[test]
fn twenty_thousand_hashes_under_five_seconds() {
    let mut state = 42u64;
    let hashes: Vec<_> = (0..20_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        })
        .collect();
    let start = Instant::now();
    let r = group(&hashes, 6);
    assert!(r.limited.is_empty());
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    eprintln!("20,000 hashes: {:?}", start.elapsed());
}
#[test]
fn oversized_bucket_and_pair_budget_are_bounded() {
    let r = group(&vec![0; MAX_BUCKET_ENTRIES + 1], 6);
    assert!(!r.limited.is_empty());
    assert!(r.components.is_empty());
    let r = group(&vec![0; MAX_BUCKET_ENTRIES], 6);
    assert!(!r.limited.is_empty());
    assert_eq!(r.components[0].len(), MAX_BUCKET_ENTRIES);
}
fn png_chunk(output: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(name);
    output.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(name);
    crc.update(data);
    output.extend_from_slice(&crc.finalize().to_be_bytes());
}
#[test]
fn corrupt_oversize_and_dimension_bomb_are_reported_and_search_continues() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("text.jpg"), b"not a jpeg").unwrap();
    fs::write(
        d.path().join("truncated.jpeg"),
        [0xff, 0xd8, 0xff, 0xe0, 0, 16],
    )
    .unwrap();
    let mut bomb = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&(MAX_DIMENSION + 1).to_be_bytes());
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut bomb, b"IHDR", &header);
    png_chunk(&mut bomb, b"IDAT", &[]);
    png_chunk(&mut bomb, b"IEND", &[]);
    let mut reader = ImageReader::with_format(std::io::Cursor::new(&bomb), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    assert!(matches!(
        reader.into_decoder(),
        Err(image::ImageError::Limits(_))
    ));
    fs::write(d.path().join("bomb.png"), bomb).unwrap();
    assert!(render_thumbnail(&d.path().join("bomb.png"), 128).is_err());
    for name in ["a.png", "b.png"] {
        pattern(128).save(d.path().join(name)).unwrap();
    }
    fs::File::create(d.path().join("big.bmp"))
        .unwrap()
        .set_len(64 << 20 | 1)
        .unwrap();
    let r = run(d.path());
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.skipped_count, 4);
    assert!(r.incomplete);
    assert!(r
        .skipped
        .iter()
        .any(|(_, e)| *e == ScanIssue::Io(io::ErrorKind::FileTooLarge)));
    assert_eq!(
        r.skipped
            .iter()
            .filter(|(_, e)| *e == ScanIssue::Io(io::ErrorKind::InvalidData))
            .count(),
        3
    );
}
#[cfg(unix)]
#[test]
fn hardlinks_symlinks_overlaps_and_special_files() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    pattern(128).save(d.path().join("a.png")).unwrap();
    fs::hard_link(d.path().join("a.png"), d.path().join("alias.png")).unwrap();
    symlink("a.png", d.path().join("sym.png")).unwrap();
    symlink(d.path(), d.path().join("loop")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        d.path().join("pipe.png"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(run(d.path()).groups.is_empty());
    pattern(128).save(d.path().join("b.png")).unwrap();
    let r = find_similar_images(
        &[
            d.path().into(),
            d.path().join("loop"),
            d.path().join("a.png"),
        ],
        &opts(),
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap();
    assert!(!r.incomplete);
    assert_eq!(r.groups[0].entries.len(), 2);
    let a = r.groups[0]
        .entries
        .iter()
        .find(|e| e.path.ends_with("a.png"))
        .unwrap();
    assert_eq!(a.linked_paths, vec![d.path().join("alias.png")]);
    assert_eq!(a.hard_links, 2);
}
#[test]
fn empty_missing_budgets_and_error_cap() {
    let d = tempfile::tempdir().unwrap();
    assert!(!run(d.path()).incomplete);
    assert!(run(&d.path().join("missing")).incomplete);
    pattern(128).save(d.path().join("a.png")).unwrap();
    for options in [
        SimilarOptions {
            max_entries: 1,
            ..opts()
        },
        SimilarOptions {
            max_depth: 0,
            ..opts()
        },
    ] {
        let r = find_similar_images(
            &[d.path().into()],
            &options,
            &JobHandle::silent(),
            &mut |_| {},
        )
        .unwrap();
        assert!(r.incomplete);
    }
    let roots: Vec<_> = (0..250).map(|i| d.path().join(i.to_string())).collect();
    let r = find_similar_images(&roots, &opts(), &JobHandle::silent(), &mut |_| {}).unwrap();
    assert_eq!(r.skipped.len(), 200);
    assert_eq!(r.skipped_count, 250);
}
#[test]
fn progress_stages_and_cancel_in_each_stage() {
    let d = tempfile::tempdir().unwrap();
    for name in ["a.png", "b.png"] {
        pattern(128).save(d.path().join(name)).unwrap();
    }
    let mut events = Vec::new();
    let r = find_similar_images(
        &[d.path().into()],
        &opts(),
        &JobHandle::silent(),
        &mut |p| events.push(p),
    )
    .unwrap();
    let mut stages: Vec<_> = events.iter().map(|p| p.stage).collect();
    stages.dedup();
    assert_eq!(
        stages,
        vec![SimilarStage::Scan, SimilarStage::Hash, SimilarStage::Group]
    );
    assert_eq!(events[0].total, None);
    assert_eq!(
        events
            .iter()
            .find(|p| p.stage == SimilarStage::Hash)
            .unwrap()
            .total,
        Some(2)
    );
    assert!(events
        .windows(2)
        .all(|p| p[0].bytes_read <= p[1].bytes_read));
    assert!(r.bytes_read > 0);
    for stage in [SimilarStage::Scan, SimilarStage::Hash, SimilarStage::Group] {
        let job = JobHandle::silent();
        let r = find_similar_images(&[d.path().into()], &opts(), &job, &mut |p| {
            if p.stage == stage {
                job.cancel();
            }
        })
        .unwrap();
        assert!(r.cancelled && r.incomplete);
    }
}
#[test]
fn changes_and_decoder_panics_are_rejected() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("a.png");
    pattern(128).save(&path).unwrap();
    let before = scan_fs::metadata(&path).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    assert!(!unchanged(&before, &scan_fs::metadata(&path).unwrap()));
    let mut changed = before.clone();
    changed.len += 1;
    assert!(!unchanged(&before, &changed));
    let result: std::result::Result<(), ScanIssue> = guarded(|| panic!("simulated decoder panic"));
    assert_eq!(result, Err(ScanIssue::Io(io::ErrorKind::InvalidData)));
    let r = find_similar_images(
        &[d.path().into()],
        &opts(),
        &JobHandle::silent(),
        &mut |p| {
            if p.stage == SimilarStage::Hash {
                fs::write(&path, b"changed").unwrap();
            }
        },
    )
    .unwrap();
    assert!(r.groups.is_empty());
    assert_eq!(r.skipped[0].1, ScanIssue::Changed);
}
fn entry(path: &str, hash: u64, size: u64, width: u32, age: u64) -> SimilarEntry {
    SimilarEntry {
        path: path.into(),
        linked_paths: vec![],
        size,
        width,
        height: width,
        modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(age)),
        hash,
        hash_hex: format!("{hash:016x}"),
        distance_to_reference: 0,
        hard_links: 1,
    }
}
#[test]
fn all_keep_rules_ties_and_missing_dates() {
    let mut g = SimilarGroup {
        entries: vec![
            entry("/z/long", 0, 100, 32, 1),
            entry("/a/long", 0, 50, 64, 2),
            entry("/b", 0, 10, 16, 3),
        ],
        reclaimable_bytes: 60,
    };
    for (rule, path) in [
        (SimilarKeepRule::LargestResolution, "/a/long"),
        (SimilarKeepRule::LargestFile, "/z/long"),
        (SimilarKeepRule::Oldest, "/z/long"),
        (SimilarKeepRule::Newest, "/b"),
        (SimilarKeepRule::ShortestPath, "/b"),
    ] {
        let s = suggest_keep_similar(&g, rule).unwrap();
        assert_eq!(s.keep.path, Path::new(path));
        assert_eq!(s.alternatives.len(), 2);
    }
    g.entries = vec![entry("/z", 0, 10, 10, 1), entry("/a", 0, 10, 10, 1)];
    for rule in [
        SimilarKeepRule::LargestResolution,
        SimilarKeepRule::LargestFile,
        SimilarKeepRule::Oldest,
        SimilarKeepRule::Newest,
        SimilarKeepRule::ShortestPath,
    ] {
        assert_eq!(
            suggest_keep_similar(&g, rule).unwrap().keep.path,
            Path::new("/a")
        );
    }
    g.entries[1].modified = None;
    for rule in [SimilarKeepRule::Oldest, SimilarKeepRule::Newest] {
        assert_eq!(
            suggest_keep_similar(&g, rule).unwrap().keep.path,
            Path::new("/z")
        );
    }
    g.entries.clear();
    assert!(suggest_keep_similar(&g, SimilarKeepRule::LargestFile).is_none());
}
#[test]
fn component_reference_distances_and_space_order() {
    let entries = vec![
        entry("/a", 0, 10, 10, 1),
        entry("/b", 63, 20, 30, 1),
        entry("/c", 4095, 30, 20, 1),
        entry("/d", u64::MAX, 10, 10, 1),
        entry("/e", u64::MAX, 40, 10, 1),
    ];
    let hashes: Vec<_> = entries.iter().map(|e| e.hash).collect();
    let groups = make_groups(&entries, group(&hashes, 6).components);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].reclaimable_bytes, 30);
    assert_eq!(groups[1].reclaimable_bytes, 10);
    assert_eq!(
        groups[0]
            .entries
            .iter()
            .map(|e| e.path.as_path())
            .collect::<Vec<_>>(),
        vec![Path::new("/b"), Path::new("/c"), Path::new("/a")]
    );
    assert_eq!(
        groups[0]
            .entries
            .iter()
            .map(|e| e.distance_to_reference)
            .collect::<Vec<_>>(),
        vec![6, 12, 0]
    );
    let entries = vec![
        entry("/z", 1, 10, 10, 1),
        entry("/zz", 1, 10, 10, 1),
        entry("/a", 2, 10, 10, 1),
        entry("/aa", 2, 10, 10, 1),
    ];
    let groups = make_groups(&entries, vec![vec![0, 1], vec![2, 3]]);
    assert_eq!(groups[0].entries[0].path, Path::new("/a"));
}
#[test]
fn exif_orientation_applied_before_dimensions_and_hashing() {
    let d = tempfile::tempdir().unwrap();
    let original = pattern(128).resize_exact(96, 64, image::imageops::FilterType::Triangle);
    let mut jpeg = std::io::Cursor::new(Vec::new());
    original.write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
    let jpeg = jpeg.into_inner();
    // Little-endian TIFF with a single SHORT Orientation=6 (90 degrees clockwise).
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut oriented = jpeg[..2].to_vec();
    oriented.extend_from_slice(&[0xff, 0xe1]);
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend_from_slice(exif);
    oriented.extend_from_slice(&jpeg[2..]);
    fs::write(d.path().join("a.jpg"), oriented).unwrap();
    image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg)
        .unwrap()
        .rotate90()
        .save(d.path().join("b.png"))
        .unwrap();
    let thumbnail = render_thumbnail(&d.path().join("a.jpg"), 512).unwrap();
    assert_eq!((thumbnail.width, thumbnail.height), (64, 96));
    let r = run(d.path());
    assert!(!r.incomplete, "{:?}", r.skipped);
    assert_eq!(r.groups.len(), 1);
    assert!(r.groups[0]
        .entries
        .iter()
        .all(|e| (e.width, e.height, e.distance_to_reference) == (64, 96, 0)));
}
#[test]
fn cancellation_during_physical_reads_does_not_retry_interrupted_forever() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("data");
    fs::write(&path, [42; 100]).unwrap();
    let file = fs::File::open(&path).unwrap();
    let job = JobHandle::silent();
    let bytes = AtomicU64::new(0);
    let mut reader = CheckedReader {
        file: &file,
        job: &job,
        bytes: &bytes,
        position: 0,
        length: 100,
    };
    let mut block = [0; 10];
    reader.read_exact(&mut block).unwrap();
    job.cancel();
    assert_eq!(
        reader.read_exact(&mut block).unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert_eq!(bytes.load(Ordering::Relaxed), 10);
}
#[cfg(unix)]
#[test]
fn replacing_parent_with_symlink_before_decode_does_not_escape() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sub = d.path().join("sub");
    fs::create_dir(&sub).unwrap();
    for name in ["a.png", "b.png"] {
        pattern(128).save(sub.join(name)).unwrap();
        pattern(128).save(outside.path().join(name)).unwrap();
    }
    let r = find_similar_images(
        std::slice::from_ref(&sub),
        &opts(),
        &JobHandle::silent(),
        &mut |p| {
            if p.stage == SimilarStage::Hash {
                fs::rename(&sub, d.path().join("old")).unwrap();
                symlink(outside.path(), &sub).unwrap();
            }
        },
    )
    .unwrap();
    assert!(r.incomplete);
    assert!(r.groups.is_empty());
    assert_eq!(r.bytes_read, 0);
}
#[test]
fn cancel_inside_hash_file_after_first_read() {
    use crate::ops::{JobControl, ProgressEvent, ProgressSink};
    use std::sync::{atomic::AtomicBool, Arc};
    struct Control {
        bytes: Arc<AtomicU64>,
        cancelled: AtomicBool,
    }
    impl JobControl for Control {
        fn cancel(&self) {
            self.cancelled.store(true, Ordering::Relaxed);
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Relaxed)
        }
        fn pause(&self) {}
        fn resume(&self) {}
        fn is_paused(&self) -> bool {
            false
        }
        fn checkpoint(&self) -> bool {
            if self.bytes.load(Ordering::Relaxed) > 0 {
                self.cancel();
            }
            self.is_cancelled()
        }
    }
    struct Sink;
    impl ProgressSink for Sink {
        fn event(&self, _: ProgressEvent) {}
    }
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("a.bmp");
    pattern(128).save(&path).unwrap();
    let meta = scan_fs::metadata(&path).unwrap();
    let mut entry = entry("", 0, meta.len, 128, 0);
    entry.path = path;
    let candidate = Candidate { entry, meta };
    let bytes = Arc::new(AtomicU64::new(0));
    let job = JobHandle::controlled(
        Arc::new(Sink),
        Arc::new(Control {
            bytes: bytes.clone(),
            cancelled: AtomicBool::new(false),
        }),
    );
    assert!(hash_file(&candidate, &job, &bytes).is_err());
    assert!(job.is_cancelled());
    assert!(bytes.load(Ordering::Relaxed) > 0);
}

#[test]
fn thumbnails_are_bounded_png_and_reject_invalid_inputs() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("a.png");
    pattern(800).save(&path).unwrap();
    for (edge, expected) in [(1, 64), (128, 128), (999, 512)] {
        let t = render_thumbnail(&path, edge).unwrap();
        assert_eq!((t.width, t.height), (expected, expected));
        assert!(t.bytes.starts_with(b"\x89PNG"));
    }
    fs::write(&path, b"broken").unwrap();
    assert!(render_thumbnail(&path, 128).is_err());
    fs::File::create(&path)
        .unwrap()
        .set_len((64 << 20) + 1)
        .unwrap();
    assert!(render_thumbnail(&path, 128).is_err());
}
#[cfg(unix)]
#[test]
fn thumbnails_refuse_symlink_leaf_and_parent_and_cancel() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("a.png");
    pattern(64).save(&path).unwrap();
    std::os::unix::fs::symlink(&path, d.path().join("link.png")).unwrap();
    std::os::unix::fs::symlink(d.path(), d.path().join("dir")).unwrap();
    assert!(render_thumbnail(&d.path().join("link.png"), 128).is_err());
    assert!(render_thumbnail(&d.path().join("dir/a.png"), 128).is_err());
    let job = JobHandle::silent();
    job.cancel();
    assert!(matches!(
        render_thumbnail_checked(&path, 128, &job),
        Err(Error::Cancelled)
    ));
}
