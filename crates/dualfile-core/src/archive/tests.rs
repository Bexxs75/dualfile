//! Tests of the archive layer. All fixtures are generated here; no real data is used.

use super::*;
use crate::ops::{self, JobHandle};
use crate::vfs::LocalVfs;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::time::Duration;
use zip::unstable::write::FileOptionsExt;

// ---------------------------------------------------------------- fixtures

#[derive(Clone)]
enum Kind {
    File(Vec<u8>),
    Dir,
    Link(String),
    Fifo,
    HardLink(String),
}

#[derive(Clone)]
struct Item {
    name: String,
    kind: Kind,
    mode: u32,
}

fn file(name: &str, data: &[u8]) -> Item {
    Item {
        name: name.into(),
        kind: Kind::File(data.to_vec()),
        mode: 0o644,
    }
}
fn dir(name: &str) -> Item {
    Item {
        name: name.into(),
        kind: Kind::Dir,
        mode: 0o755,
    }
}
fn link(name: &str, target: &str) -> Item {
    Item {
        name: name.into(),
        kind: Kind::Link(target.into()),
        mode: 0o777,
    }
}
fn with_mode(mut item: Item, mode: u32) -> Item {
    item.mode = mode;
    item
}

const HELLO: &[u8] = b"Hello, Archive!\n";
const DEEP: &[u8] = b"deep content in a nested folder";
const UNI: &[u8] = "Inhalt \u{00fc}\u{00f6}\u{00e4} \u{1f600}".as_bytes();

fn standard_items() -> Vec<Item> {
    vec![
        file("hello.txt", HELLO),
        dir("dir/"),
        dir("dir/nested/"),
        file("dir/nested/deep.txt", DEEP),
        file("empty.txt", b""),
        dir("empty_dir/"),
        file("\u{00fc}n\u{00ef}/\u{00f6}.txt", UNI),
        link("link", "hello.txt"),
    ]
}

fn zip_bytes(items: &[Item], method: zip::CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for item in items {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(method)
            .unix_permissions(item.mode);
        match &item.kind {
            Kind::File(data) => {
                writer.start_file(item.name.as_str(), options).unwrap();
                writer.write_all(data).unwrap();
            }
            Kind::Dir => writer.add_directory(item.name.as_str(), options).unwrap(),
            Kind::Link(target) => writer
                .add_symlink(item.name.as_str(), target.as_str(), options)
                .unwrap(),
            _ => panic!("not for zip"),
        }
    }
    writer.finish().unwrap().into_inner()
}

fn tar_header(name: &str, typeflag: u8, size: u64, mode: u32, link: &str) -> [u8; 512] {
    let mut block = [0u8; 512];
    let put = |block: &mut [u8; 512], at: usize, text: &str| {
        block[at..at + text.len()].copy_from_slice(text.as_bytes());
    };
    assert!(name.len() <= 100, "tar test names must fit the header");
    put(&mut block, 0, name);
    put(&mut block, 100, &format!("{mode:07o}\0"));
    put(&mut block, 108, "0000000\0");
    put(&mut block, 116, "0000000\0");
    put(&mut block, 124, &format!("{size:011o}\0"));
    put(&mut block, 136, &format!("{:011o}\0", 1_700_000_000u64));
    block[148..156].copy_from_slice(b"        ");
    block[156] = typeflag;
    put(&mut block, 157, link);
    put(&mut block, 257, "ustar\0");
    put(&mut block, 263, "00");
    let sum: u32 = block.iter().map(|b| u32::from(*b)).sum();
    put(&mut block, 148, &format!("{sum:06o}\0 "));
    block
}

fn tar_bytes(items: &[Item]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        match &item.kind {
            Kind::File(data) => {
                out.extend_from_slice(&tar_header(
                    &item.name,
                    b'0',
                    data.len() as u64,
                    item.mode,
                    "",
                ));
                out.extend_from_slice(data);
                out.resize(out.len().div_ceil(512) * 512, 0);
            }
            Kind::Dir => out.extend_from_slice(&tar_header(&item.name, b'5', 0, item.mode, "")),
            Kind::Link(target) => {
                out.extend_from_slice(&tar_header(&item.name, b'2', 0, item.mode, target))
            }
            Kind::HardLink(target) => {
                out.extend_from_slice(&tar_header(&item.name, b'1', 0, item.mode, target))
            }
            Kind::Fifo => out.extend_from_slice(&tar_header(&item.name, b'6', 0, item.mode, "")),
        }
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

fn gz(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}
fn bz2(data: &[u8]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}
fn xz(data: &[u8]) -> Vec<u8> {
    let mut encoder =
        lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn sevenz_bytes(items: &[Item]) -> Vec<u8> {
    let mut writer = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    for item in items {
        match &item.kind {
            Kind::File(data) => {
                writer
                    .push_archive_entry(
                        sevenz_rust2::ArchiveEntry::new_file(&item.name),
                        Some(&data[..]),
                    )
                    .unwrap();
            }
            Kind::Dir => {
                writer
                    .push_archive_entry::<&[u8]>(
                        sevenz_rust2::ArchiveEntry::new_directory(item.name.trim_end_matches('/')),
                        None,
                    )
                    .unwrap();
            }
            Kind::Link(target) => {
                let mut entry = sevenz_rust2::ArchiveEntry::new_file(&item.name);
                entry.has_windows_attributes = true;
                entry.windows_attributes = (0o120_777u32 << 16) | 0x8000;
                writer
                    .push_archive_entry(entry, Some(target.as_bytes()))
                    .unwrap();
            }
            _ => panic!("not for 7z"),
        }
    }
    writer.finish().unwrap().into_inner()
}

/// Writes `bytes` as `name` in `dir`, opens it with the given limits.
fn open_with(dir: &Path, name: &str, bytes: &[u8], limits: ArchiveLimits) -> Result<ArchiveVfs> {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    ArchiveVfs::open_local_with(&path, limits)
}
fn open_bytes(dir: &Path, name: &str, bytes: &[u8]) -> ArchiveVfs {
    open_with(dir, name, bytes, ArchiveLimits::default()).unwrap()
}
fn ap(vfs: &ArchiveVfs, inner: &str) -> VPath {
    VPath::archive(vfs.outer().clone(), inner).unwrap()
}
fn names(vfs: &ArchiveVfs, inner: &str) -> Vec<String> {
    let mut names: Vec<String> = vfs
        .list(&ap(vfs, inner))
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    names.sort();
    names
}
fn read_all(vfs: &ArchiveVfs, inner: &str) -> Vec<u8> {
    let mut data = Vec::new();
    vfs.open_read(&ap(vfs, inner))
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    data
}

/// Same expectations for every format.
fn check_standard(vfs: &ArchiveVfs) {
    assert_eq!(
        names(vfs, "/"),
        vec![
            "dir".to_string(),
            "empty.txt".into(),
            "empty_dir".into(),
            "hello.txt".into(),
            "link".into(),
            "\u{00fc}n\u{00ef}".into(),
        ]
    );
    assert_eq!(read_all(vfs, "/hello.txt"), HELLO);
    // nested folder
    assert_eq!(names(vfs, "/dir"), vec!["nested".to_string()]);
    assert_eq!(names(vfs, "/dir/nested"), vec!["deep.txt".to_string()]);
    assert_eq!(read_all(vfs, "/dir/nested/deep.txt"), DEEP);
    // empty file and empty folder
    let empty = vfs.stat(&ap(vfs, "/empty.txt")).unwrap();
    assert_eq!((empty.kind, empty.size), (EntryKind::File, 0));
    assert!(read_all(vfs, "/empty.txt").is_empty());
    assert!(vfs.list(&ap(vfs, "/empty_dir")).unwrap().is_empty());
    // unicode
    assert_eq!(read_all(vfs, "/\u{00fc}n\u{00ef}/\u{00f6}.txt"), UNI);
    // size and kind
    let hello = vfs.stat(&ap(vfs, "/hello.txt")).unwrap();
    assert_eq!(hello.size, HELLO.len() as u64);
    assert_eq!(
        vfs.stat_file(&ap(vfs, "/hello.txt")).unwrap().size,
        HELLO.len() as u64
    );
    assert!(matches!(
        vfs.stat_file(&ap(vfs, "/dir")),
        Err(Error::IsADirectory(_))
    ));
    // links are listed as links and never followed
    let link = vfs.stat(&ap(vfs, "/link")).unwrap();
    assert_eq!(link.kind, EntryKind::Symlink);
    assert_eq!(link.link_target.as_deref(), Some("hello.txt"));
    assert_eq!(vfs.read_link(&ap(vfs, "/link")).unwrap(), "hello.txt");
    assert!(vfs.open_read(&ap(vfs, "/link")).is_err());
    assert!(matches!(
        vfs.read_link(&ap(vfs, "/hello.txt")),
        Err(Error::InvalidInput(_))
    ));
    // missing
    assert!(matches!(
        vfs.stat(&ap(vfs, "/nope")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        vfs.list(&ap(vfs, "/hello.txt")),
        Err(Error::NotADirectory(_))
    ));
}

// ------------------------------------------------------------ list and read

#[test]
fn lists_and_reads_zip_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = zip_bytes(&standard_items(), zip::CompressionMethod::Stored);
    let vfs = open_bytes(tmp.path(), "a.zip", &bytes);
    assert_eq!(vfs.format(), Format::Zip);
    check_standard(&vfs);
}

#[test]
fn lists_and_reads_zip_deflate() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = zip_bytes(&standard_items(), zip::CompressionMethod::Deflated);
    check_standard(&open_bytes(tmp.path(), "a.zip", &bytes));
}

#[test]
fn lists_and_reads_zip_bzip2() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = zip_bytes(&standard_items(), zip::CompressionMethod::Bzip2);
    check_standard(&open_bytes(tmp.path(), "a.zip", &bytes));
}

#[test]
fn lists_and_reads_plain_tar() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(tmp.path(), "a.tar", &tar_bytes(&standard_items()));
    assert_eq!(vfs.format(), Format::Tar(TarCompression::None));
    check_standard(&vfs);
}

#[test]
fn lists_and_reads_tar_gz() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(tmp.path(), "a.tar.gz", &gz(&tar_bytes(&standard_items())));
    assert_eq!(vfs.format(), Format::Tar(TarCompression::Gzip));
    check_standard(&vfs);
}

#[test]
fn lists_and_reads_tar_bz2() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(tmp.path(), "a.tar.bz2", &bz2(&tar_bytes(&standard_items())));
    assert_eq!(vfs.format(), Format::Tar(TarCompression::Bzip2));
    check_standard(&vfs);
}

#[test]
fn lists_and_reads_tar_xz() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(tmp.path(), "a.tar.xz", &xz(&tar_bytes(&standard_items())));
    assert_eq!(vfs.format(), Format::Tar(TarCompression::Xz));
    check_standard(&vfs);
}

#[test]
fn lists_and_reads_7z() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(tmp.path(), "a.7z", &sevenz_bytes(&standard_items()));
    assert_eq!(vfs.format(), Format::SevenZ);
    check_standard(&vfs);
}

#[test]
fn archive_root_entry_and_implicit_folders() {
    let tmp = tempfile::tempdir().unwrap();
    // No explicit folder entries at all: the folders exist implicitly.
    let bytes = zip_bytes(&[file("x/y/z.txt", b"z")], zip::CompressionMethod::Stored);
    let vfs = open_bytes(tmp.path(), "a.zip", &bytes);
    assert_eq!(names(&vfs, "/x/y"), vec!["z.txt".to_string()]);
    let root = vfs.stat(&vfs.root()).unwrap();
    assert_eq!(root.kind, EntryKind::Dir);
    assert_eq!(root.name, "a.zip");
}

#[test]
fn modes_never_keep_setuid_setgid_sticky_and_times_are_read() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [
        with_mode(file("suid", b"x"), 0o104_755),
        with_mode(dir("d/"), 0o1777),
    ];
    let vfs = open_bytes(tmp.path(), "a.tar", &tar_bytes(&items));
    let entry = vfs.stat(&ap(&vfs, "/suid")).unwrap();
    assert_eq!(entry.mode, Some(0o755));
    assert_eq!(vfs.stat(&ap(&vfs, "/d")).unwrap().mode, Some(0o777));
    assert_eq!(
        entry
            .modified
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        1_700_000_000
    );
}

#[test]
fn special_files_are_shown_only() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [
        file("normal", b"n"),
        Item {
            name: "pipe".into(),
            kind: Kind::Fifo,
            mode: 0o644,
        },
        Item {
            name: "hard".into(),
            kind: Kind::HardLink("normal".into()),
            mode: 0o644,
        },
    ];
    let vfs = open_bytes(tmp.path(), "a.tar", &tar_bytes(&items));
    for name in ["/pipe", "/hard"] {
        let entry = vfs.stat(&ap(&vfs, name)).unwrap();
        assert!(entry.error.is_some(), "{name} must carry a note");
        assert!(vfs.open_read(&ap(&vfs, name)).is_err());
        assert!(vfs.read_range(&ap(&vfs, name), 0, 10).is_err());
    }
    assert_eq!(read_all(&vfs, "/normal"), b"n");
}

// ------------------------------------------------------------ safety

#[test]
fn unsafe_entries_are_marked_and_never_readable_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let evil: Vec<(&str, &[u8])> = vec![
        ("../escape.txt", b"1"),
        ("/etc/abs.txt", b"2"),
        ("a/../../b.txt", b"3"),
        ("back\\slash.txt", b"4"),
        ("C:/drive.txt", b"5"),
        ("nul\0byte.txt", b"6"),
    ];
    let mut items = vec![file("ok.txt", b"ok")];
    items.extend(evil.iter().map(|(name, data)| file(name, data)));
    let bytes = zip_bytes(&items, zip::CompressionMethod::Stored);
    let vfs = open_bytes(tmp.path(), "evil.zip", &bytes);
    let listing = vfs.list(&vfs.root()).unwrap();
    let flagged: Vec<_> = listing.iter().filter(|e| e.error.is_some()).collect();
    assert_eq!(flagged.len(), evil.len(), "{listing:?}");
    for entry in &flagged {
        assert!(entry.name.starts_with("[unsicher "), "{}", entry.name);
        assert!(
            !entry.name.contains('/') && !entry.name.contains('\\') && !entry.name.contains('\0')
        );
        let path = ap(&vfs, &format!("/{}", entry.name));
        assert!(vfs.open_read(&path).is_err());
        assert!(vfs.read_range(&path, 0, 4).is_err());
        assert!(vfs.stat_file(&path).is_err());
    }
    // nothing was placed where the raw names point to
    assert!(vfs.stat(&ap(&vfs, "/escape.txt")).is_err());
    assert!(vfs.stat(&ap(&vfs, "/b.txt")).is_err());
    assert_eq!(read_all(&vfs, "/ok.txt"), b"ok");
}

#[test]
fn unsafe_entries_are_marked_in_tar_and_through_links() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [
        file("../x", b"1"),
        file("/abs", b"2"),
        file("a/../../b", b"3"),
        link("lnk", "/etc"),
        // the parent is a link: a write through it would leave the tree
        file("lnk/passwd", b"4"),
        file("good", b"g"),
    ];
    let vfs = open_bytes(tmp.path(), "evil.tar", &tar_bytes(&items));
    let unsafe_count = vfs
        .list(&vfs.root())
        .unwrap()
        .iter()
        .filter(|e| e.name.starts_with("[unsicher "))
        .count();
    assert_eq!(unsafe_count, 4);
    assert_eq!(vfs.read_link(&ap(&vfs, "/lnk")).unwrap(), "/etc");
    assert!(
        vfs.list(&ap(&vfs, "/lnk")).is_err(),
        "links are never followed"
    );
    assert_eq!(read_all(&vfs, "/good"), b"g");
}

#[test]
fn dot_components_and_double_slashes_are_normalized() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [file("./a//b.txt", b"b"), dir("./")];
    let vfs = open_bytes(tmp.path(), "n.tar", &tar_bytes(&items));
    assert_eq!(read_all(&vfs, "/a/b.txt"), b"b");
}

#[test]
fn zip_bomb_stops_at_the_actual_byte_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let zeros = vec![0u8; 4 << 20];
    let bytes = zip_bytes(
        &[file("bomb.bin", &zeros)],
        zip::CompressionMethod::Deflated,
    );
    let limits = ArchiveLimits {
        max_file_bytes: 256 * 1024,
        ratio_min_bytes: u64::MAX,
        ..ArchiveLimits::default()
    };
    let vfs = open_with(tmp.path(), "bomb.zip", &bytes, limits).unwrap();
    let mut sink = Vec::new();
    let error = vfs
        .open_read(&ap(&vfs, "/bomb.bin"))
        .unwrap()
        .read_to_end(&mut sink)
        .unwrap_err();
    assert!(error.to_string().contains("zu groß"), "{error}");
    // what was really decompressed is what counts, and the read stopped right after the cap
    let (counted, _) = vfs.cost();
    assert!(counted <= 256 * 1024 + 64 * 1024, "{counted}");
    assert!(sink.len() <= 256 * 1024);
}

#[test]
fn zip_announced_size_is_not_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let data = vec![7u8; 100_000];
    let mut bytes = zip_bytes(&[file("lie.bin", &data)], zip::CompressionMethod::Deflated);
    // central directory header: signature 50 4b 01 02, uncompressed size at offset 24
    let at = bytes
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("central header");
    bytes[at + 24..at + 28].copy_from_slice(&10u32.to_le_bytes());
    let vfs = open_bytes(tmp.path(), "lie.zip", &bytes);
    assert_eq!(vfs.stat(&ap(&vfs, "/lie.bin")).unwrap().size, 10);
    let mut out = Vec::new();
    let error = vfs
        .open_read(&ap(&vfs, "/lie.bin"))
        .unwrap()
        .read_to_end(&mut out)
        .unwrap_err();
    assert!(error.to_string().contains("beschädigt"), "{error}");
    assert!(out.len() < 100_000, "stopped at the announced size");
}

#[test]
fn compression_ratio_guard_aborts_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let zeros = vec![0u8; 2 << 20];
    let bytes = zip_bytes(&[file("z.bin", &zeros)], zip::CompressionMethod::Deflated);
    let limits = ArchiveLimits {
        ratio_min_bytes: 64 * 1024,
        ratio_max: 100,
        ..ArchiveLimits::default()
    };
    let vfs = open_with(tmp.path(), "z.zip", &bytes, limits).unwrap();
    let mut out = Vec::new();
    let error = vfs
        .open_read(&ap(&vfs, "/z.bin"))
        .unwrap()
        .read_to_end(&mut out)
        .unwrap_err();
    assert!(
        error.to_string().contains("Kompressionsverhältnis"),
        "{error}"
    );
}

#[test]
fn ratio_guard_applies_while_indexing_a_compressed_tar() {
    let tmp = tempfile::tempdir().unwrap();
    let zeros = vec![0u8; 4 << 20];
    let bytes = gz(&tar_bytes(&[file("z.bin", &zeros)]));
    let limits = ArchiveLimits {
        ratio_min_bytes: 64 * 1024,
        ratio_max: 100,
        ..ArchiveLimits::default()
    };
    let error = open_with(tmp.path(), "z.tar.gz", &bytes, limits)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("Kompressionsverhältnis"),
        "{error}"
    );
}

#[test]
fn ratio_guard_does_not_trip_on_a_plain_uncompressed_tar() {
    // A plain tar is not compressed, so reading a large entry must not look like a bomb.
    let tmp = tempfile::tempdir().unwrap();
    let data = vec![0u8; 256 * 1024];
    let bytes = tar_bytes(&[file("big.bin", &data)]);
    let limits = ArchiveLimits {
        ratio_min_bytes: 64 * 1024,
        ratio_max: 2,
        ..ArchiveLimits::default()
    };
    let vfs = open_with(tmp.path(), "a.tar", &bytes, limits).unwrap();
    let mut out = Vec::new();
    vfs.open_read(&ap(&vfs, "/big.bin"))
        .unwrap()
        .read_to_end(&mut out)
        .expect("plain tar entry must not trip the ratio guard");
    assert_eq!(out.len(), 256 * 1024);
}

#[test]
fn index_byte_budget_stops_a_tar_bomb_while_parsing() {
    let tmp = tempfile::tempdir().unwrap();
    // Low ratio (random-looking data is not needed: ratio guard is off here).
    let data: Vec<u8> = (0..(1 << 20))
        .map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let bytes = gz(&tar_bytes(&[
        file("big.bin", &data),
        file("after.txt", b"x"),
    ]));
    let limits = ArchiveLimits {
        index_bytes_tar: 128 * 1024,
        ratio_min_bytes: u64::MAX,
        ..ArchiveLimits::default()
    };
    let error = open_with(tmp.path(), "b.tar.gz", &bytes, limits)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("zu groß für die Anzeige"),
        "{error}"
    );
}

#[test]
fn index_time_budget_aborts() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = zip_bytes(&standard_items(), zip::CompressionMethod::Stored);
    let limits = ArchiveLimits {
        index_time: Duration::ZERO,
        ..ArchiveLimits::default()
    };
    let error = open_with(tmp.path(), "t.zip", &bytes, limits)
        .err()
        .unwrap();
    assert!(error.to_string().contains("zu langsam"), "{error}");
}

#[test]
fn one_index_budget_is_shared_over_several_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..(256 * 1024))
        .map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect();
    let bytes = gz(&tar_bytes(&[file("big.bin", &data)]));
    let path = tmp.path().join("s.tar.gz");
    fs::write(&path, &bytes).unwrap();
    let limits = ArchiveLimits {
        ratio_min_bytes: u64::MAX,
        ..ArchiveLimits::default()
    };
    let outer = VPath::local(&path).unwrap();
    // measure what one index costs
    let probe = Budget::new(u64::MAX, None);
    ArchiveVfs::open(
        outer.clone(),
        Source::File(path.clone()),
        limits.clone(),
        probe.clone(),
    )
    .unwrap();
    let one = probe.used_bytes();
    assert!(one > 256 * 1024);
    let shared = Budget::new(one + one / 2, None);
    ArchiveVfs::open(
        outer.clone(),
        Source::File(path.clone()),
        limits.clone(),
        shared.clone(),
    )
    .unwrap();
    let second = ArchiveVfs::open(outer, Source::File(path), limits, shared);
    assert!(
        second.is_err(),
        "the second level must hit the shared budget"
    );
}

#[test]
fn entry_count_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let items: Vec<Item> = (0..6).map(|i| file(&format!("f{i}"), b"x")).collect();
    let limits = ArchiveLimits {
        max_entries: 5,
        ..ArchiveLimits::default()
    };
    let bytes = zip_bytes(&items, zip::CompressionMethod::Stored);
    assert!(open_with(tmp.path(), "many.zip", &bytes, limits.clone()).is_err());
    let tar = tar_bytes(&items);
    assert!(open_with(tmp.path(), "many.tar", &tar, limits.clone()).is_err());
    let ok = ArchiveLimits {
        max_entries: 6,
        ..ArchiveLimits::default()
    };
    assert!(open_with(tmp.path(), "many.zip", &bytes, ok).is_ok());
}

#[test]
fn depth_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        max_depth: 4,
        ..ArchiveLimits::default()
    };
    let deep = tar_bytes(&[file("a/b/c/d/e", b"x")]);
    assert!(open_with(tmp.path(), "deep.tar", &deep, limits.clone()).is_err());
    let fine = tar_bytes(&[file("a/b/c/d", b"x")]);
    assert!(open_with(tmp.path(), "fine.tar", &fine, limits).is_ok());
}

#[test]
fn name_length_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let long = "n".repeat(256);
    let bytes = zip_bytes(&[file(&long, b"x")], zip::CompressionMethod::Stored);
    assert!(open_with(tmp.path(), "long.zip", &bytes, ArchiveLimits::default()).is_err());
    let ok = "n".repeat(255);
    let bytes = zip_bytes(&[file(&ok, b"x")], zip::CompressionMethod::Stored);
    assert!(open_with(tmp.path(), "ok.zip", &bytes, ArchiveLimits::default()).is_ok());
}

#[test]
fn index_memory_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let items: Vec<Item> = (0..200)
        .map(|i| file(&format!("file-{i:04}"), b"x"))
        .collect();
    let bytes = zip_bytes(&items, zip::CompressionMethod::Stored);
    let limits = ArchiveLimits {
        index_memory: 4096,
        ..ArchiveLimits::default()
    };
    let error = open_with(tmp.path(), "mem.zip", &bytes, limits)
        .err()
        .unwrap();
    assert!(error.to_string().contains("zu groß"), "{error}");
}

// ------------------------------------------------------------ damaged input

fn assert_rejected(dir: &Path, name: &str, bytes: &[u8]) {
    let result = std::panic::catch_unwind(|| ());
    assert!(result.is_ok());
    match open_with(dir, name, bytes, ArchiveLimits::default()) {
        Err(_) => {}
        Ok(vfs) => {
            // Opened (e.g. truncated inside data): reading everything must fail cleanly.
            let mut failed = false;
            fn walk(vfs: &ArchiveVfs, inner: &str, failed: &mut bool) {
                let Ok(entries) = vfs.list(&ap(vfs, inner)) else {
                    *failed = true;
                    return;
                };
                for entry in entries {
                    let path = format!("{}/{}", inner.trim_end_matches('/'), entry.name);
                    match entry.kind {
                        EntryKind::Dir => walk(vfs, &path, failed),
                        EntryKind::File => {
                            let mut out = Vec::new();
                            match vfs.open_read(&ap(vfs, &path)) {
                                Ok(mut reader) => {
                                    if reader.read_to_end(&mut out).is_err() {
                                        *failed = true;
                                    }
                                }
                                Err(_) => *failed = true,
                            }
                        }
                        EntryKind::Symlink => {}
                    }
                }
            }
            walk(&vfs, "/", &mut failed);
            assert!(failed, "{name}: damaged archive was accepted silently");
        }
    }
}

#[test]
fn damaged_and_truncated_archives_fail_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..50_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 9) as u8)
        .collect();
    let items = [file("data.bin", &payload), file("more.txt", b"more")];
    let zip = zip_bytes(&items, zip::CompressionMethod::Deflated);
    let tar = tar_bytes(&items);
    let targz = gz(&tar);
    let tarbz2 = bz2(&tar);
    let tarxz = xz(&tar);
    let sevenz = sevenz_bytes(&items);
    for (name, bytes) in [
        ("a.zip", &zip),
        ("a.tar", &tar),
        ("a.tar.gz", &targz),
        ("a.tar.bz2", &tarbz2),
        ("a.tar.xz", &tarxz),
        ("a.7z", &sevenz),
    ] {
        // A plain tar has no checksums over data and ends in padding: cutting the padding or
        // flipping data bytes yields a valid tar, so only real truncation is checked there.
        let plain_tar = name == "a.tar";
        let cuts = if plain_tar {
            vec![bytes.len() / 3, bytes.len() / 2]
        } else {
            vec![bytes.len() / 3, bytes.len() / 2, bytes.len() - 7]
        };
        for cut in cuts {
            assert_rejected(tmp.path(), name, &bytes[..cut]);
        }
        if !plain_tar {
            let mut flipped = bytes.clone();
            for i in (bytes.len() / 4..bytes.len() / 2).step_by(3) {
                flipped[i] ^= 0xa5;
            }
            assert_rejected(tmp.path(), name, &flipped);
        }
    }
    // plain garbage behind each signature
    for (name, magic) in [
        ("g.zip", &b"PK\x03\x04"[..]),
        ("g.7z", &[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c][..]),
        ("g.tar.gz", &[0x1f, 0x8b][..]),
        ("g.tar.bz2", &b"BZh"[..]),
        ("g.tar.xz", &[0xfd, b'7', b'z', b'X', b'Z', 0][..]),
    ] {
        let mut bytes = magic.to_vec();
        bytes.extend((0..300u32).map(|i| (i * 31 % 251) as u8));
        assert_rejected(tmp.path(), name, &bytes);
    }
    assert_rejected(tmp.path(), "empty.zip", b"");
}

#[test]
fn corrupted_zip_data_is_caught_by_the_checksum() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = zip_bytes(
        &[file("f.txt", b"checksummed content")],
        zip::CompressionMethod::Stored,
    );
    let mut bad = bytes.clone();
    let at = bad.windows(8).position(|w| w == b"checksum").unwrap();
    bad[at] ^= 0xff;
    let vfs = open_bytes(tmp.path(), "c.zip", &bad);
    let mut out = Vec::new();
    let error = vfs
        .open_read(&ap(&vfs, "/f.txt"))
        .unwrap()
        .read_to_end(&mut out)
        .unwrap_err();
    assert!(error.to_string().contains("beschädigt"), "{error}");
}

#[test]
fn parser_panic_becomes_an_error() {
    let result: Result<()> = guarded(|| panic!("simulated parser bug"));
    assert!(matches!(result, Err(Error::Archive(_))));
    let mut reader = Guarded(Box::new(PanicReader));
    assert!(reader.read(&mut [0u8; 4]).is_err());
}
struct PanicReader;
impl Read for PanicReader {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("simulated decoder bug")
    }
}

#[test]
fn encrypted_zip_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_deprecated_encryption(b"secret")
        .unwrap();
    writer.start_file("locked.txt", options).unwrap();
    writer.write_all(b"top secret").unwrap();
    writer
        .start_file("open.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"fine").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let vfs = open_bytes(tmp.path(), "enc.zip", &bytes);
    let locked = vfs.stat(&ap(&vfs, "/locked.txt")).unwrap();
    assert_eq!(
        locked.error.as_deref(),
        Some("verschlüsselt, nicht unterstützt")
    );
    let error = vfs.open_read(&ap(&vfs, "/locked.txt")).err().unwrap();
    assert!(error
        .to_string()
        .contains("verschlüsselt, nicht unterstützt"));
    assert!(vfs.read_range(&ap(&vfs, "/locked.txt"), 0, 4).is_err());
    assert_eq!(read_all(&vfs, "/open.txt"), b"fine");
}

#[test]
fn encrypted_7z_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    for encrypt_header in [false, true] {
        let mut writer = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.set_content_methods(vec![
            sevenz_rust2::encoder_options::AesEncoderOptions::new("pw".into()).into(),
            sevenz_rust2::EncoderMethod::LZMA2.into(),
        ]);
        writer.set_encrypt_header(encrypt_header);
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("secret.txt"),
                Some(&b"top secret"[..]),
            )
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        match open_with(tmp.path(), "enc.7z", &bytes, ArchiveLimits::default()) {
            Err(error) => assert!(error.to_string().contains("verschlüsselt"), "{error}"),
            Ok(vfs) => {
                let entry = vfs.stat(&ap(&vfs, "/secret.txt")).unwrap();
                assert_eq!(
                    entry.error.as_deref(),
                    Some("verschlüsselt, nicht unterstützt")
                );
                assert!(vfs.open_read(&ap(&vfs, "/secret.txt")).is_err());
            }
        }
    }
}

// Hand-made 7z headers: tiny files that declare more work than the limits allow.

fn seven_number(value: u64) -> Vec<u8> {
    let mut first = 0u8;
    let mut mask = 0x80u8;
    let mut extra = 0usize;
    while extra < 8 {
        if value < (1u64 << (7 * (extra + 1))) {
            first |= (value >> (8 * extra)) as u8;
            break;
        }
        first |= mask;
        mask >>= 1;
        extra += 1;
    }
    let mut out = vec![first];
    out.extend((0..extra).map(|k| (value >> (8 * k)) as u8));
    out
}

/// 7z file with `packed` as pack area and `header` as next header.
fn seven_container(packed: &[u8], header: &[u8]) -> Vec<u8> {
    let mut tail = Vec::new();
    tail.extend_from_slice(&(packed.len() as u64).to_le_bytes());
    tail.extend_from_slice(&(header.len() as u64).to_le_bytes());
    tail.extend_from_slice(&crc32fast::hash(header).to_le_bytes());
    let mut out = b"7z\xbc\xaf\x27\x1c\x00\x04".to_vec();
    out.extend_from_slice(&crc32fast::hash(&tail).to_le_bytes());
    out.extend_from_slice(&tail);
    out.extend_from_slice(packed);
    out.extend_from_slice(header);
    out
}

/// StreamsInfo with one pack stream and one folder with one coder.
fn seven_streams(method: &[u8], props: &[u8], unpack: u64, pack: u64) -> Vec<u8> {
    let mut h = vec![0x06];
    h.extend(seven_number(0));
    h.extend(seven_number(1));
    h.push(0x09);
    h.extend(seven_number(pack));
    h.push(0x00);
    h.extend([0x07, 0x0b]);
    h.extend(seven_number(1));
    h.push(0x00);
    h.extend(seven_number(1));
    h.push(method.len() as u8 | if props.is_empty() { 0 } else { 0x20 });
    h.extend(method);
    if !props.is_empty() {
        h.extend(seven_number(props.len() as u64));
        h.extend(props);
    }
    h.push(0x0c);
    h.extend(seven_number(unpack));
    h.push(0x00);
    h.push(0x00);
    h
}

fn open_seven(bytes: &[u8]) -> Result<ArchiveVfs> {
    let tmp = tempfile::tempdir().unwrap();
    open_with(tmp.path(), "crafted.7z", bytes, ArchiveLimits::default())
}

fn assert_too_large(result: Result<ArchiveVfs>) {
    match result {
        Err(error) => assert_eq!(error.to_string(), "Archiv zu groß für die Anzeige"),
        Ok(_) => panic!("must be rejected"),
    }
}

#[test]
fn seven_huge_lzma2_dictionary_is_rejected_before_allocating() {
    // Encoded header, LZMA2 dictionary property 0x24 = 1 GiB, one end-of-stream byte.
    let mut header = vec![0x17];
    header.extend(seven_streams(&[0x21], &[0x24], 64, 1));
    assert_too_large(open_seven(&seven_container(&[0x00], &header)));
    // The same with an LZMA coder (dictionary 1 GiB) and a PPMd coder (1 GiB of memory).
    let lzma = [&[0x5d][..], &(1u32 << 30).to_le_bytes()].concat();
    let mut header = vec![0x17];
    header.extend(seven_streams(&[0x03, 0x01, 0x01], &lzma, 64, 1));
    assert_too_large(open_seven(&seven_container(&[0x00], &header)));
    let ppmd = [&[6][..], &(1u32 << 30).to_le_bytes()].concat();
    let mut header = vec![0x17];
    header.extend(seven_streams(&[0x03, 0x04, 0x01], &ppmd, 64, 1));
    assert_too_large(open_seven(&seven_container(&[0x00], &header)));
}

#[test]
fn seven_huge_dictionary_in_a_plain_header_is_rejected() {
    let mut header = vec![0x01, 0x04];
    header.extend(seven_streams(&[0x21], &[0x24], 64, 1));
    header.push(0x00);
    assert_too_large(open_seven(&seven_container(&[0x00], &header)));
}

#[test]
fn seven_small_dictionary_is_not_a_limit_error() {
    // 1 MiB dictionary: the bogus stream is simply damaged, as before.
    let mut header = vec![0x17];
    header.extend(seven_streams(&[0x21], &[0x10], 64, 1));
    match open_seven(&seven_container(&[0x00], &header)) {
        Err(error) => assert!(error.to_string().contains("beschädigt"), "{error}"),
        Ok(_) => panic!("bogus archive"),
    }
}

#[test]
fn seven_huge_file_count_is_rejected_before_allocating() {
    // Plain header: FilesInfo declaring 8 Mi files in an 8 MiB header.
    let mut header = vec![0x01, 0x05];
    header.extend(seven_number((8 << 20) - 16));
    header.resize(8 << 20, 0);
    assert_too_large(open_seven(&seven_container(&[], &header)));
    // Encoded header (Copy coder) hiding the same declaration.
    let mut inner = vec![0x01, 0x05];
    inner.extend(seven_number(10_000_000));
    inner.resize(64, 0);
    let mut header = vec![0x17];
    header.extend(seven_streams(
        &[0x00],
        &[],
        inner.len() as u64,
        inner.len() as u64,
    ));
    assert_too_large(open_seven(&seven_container(&inner, &header)));
    // A realistic count in the same shape is no limit error.
    let mut header = vec![0x01, 0x05];
    header.extend(seven_number(10));
    header.resize(64, 0);
    match open_seven(&seven_container(&[], &header)) {
        Err(error) => assert!(error.to_string().contains("beschädigt"), "{error}"),
        Ok(_) => panic!("bogus archive"),
    }
}

// ------------------------------------------------------------ range and cost

fn noise(len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect()
}

fn check_ranges(vfs: &ArchiveVfs, expected: &[u8]) {
    let path = ap(vfs, "/big.bin");
    for (offset, len) in [
        (0usize, 100usize),
        (50_000, 64),
        (70_000, 5000),
        (123, 77),
        (199_990, 100),
    ] {
        let end = (offset + len).min(expected.len());
        assert_eq!(
            vfs.read_range(&path, offset as u64, len).unwrap(),
            expected[offset..end],
            "offset {offset}"
        );
    }
    assert!(vfs
        .read_range(&path, expected.len() as u64, 10)
        .unwrap()
        .is_empty());
    assert!(vfs
        .read_range(&path, expected.len() as u64 + 99, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn read_range_in_the_middle_of_compressed_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let data = noise(200_000);
    let items = [file("big.bin", &data)];
    check_ranges(
        &open_bytes(
            tmp.path(),
            "a.zip",
            &zip_bytes(&items, zip::CompressionMethod::Deflated),
        ),
        &data,
    );
    check_ranges(&open_bytes(tmp.path(), "a.tar", &tar_bytes(&items)), &data);
    check_ranges(
        &open_bytes(tmp.path(), "a.tar.gz", &gz(&tar_bytes(&items))),
        &data,
    );
    check_ranges(
        &open_bytes(tmp.path(), "a.tar.bz2", &bz2(&tar_bytes(&items))),
        &data,
    );
    check_ranges(
        &open_bytes(tmp.path(), "a.tar.xz", &xz(&tar_bytes(&items))),
        &data,
    );
    check_ranges(
        &open_bytes(tmp.path(), "a.7z", &sevenz_bytes(&items)),
        &data,
    );
}

#[test]
fn cursor_skips_forward_and_restarts_backward_and_both_count() {
    let tmp = tempfile::tempdir().unwrap();
    let data = noise(100_000);
    let bytes = zip_bytes(&[file("big.bin", &data)], zip::CompressionMethod::Deflated);
    let vfs = open_bytes(tmp.path(), "a.zip", &bytes);
    let path = ap(&vfs, "/big.bin");
    assert_eq!(vfs.cost().0, 0);
    vfs.read_range(&path, 0, 10).unwrap();
    assert_eq!(vfs.cost().0, 10);
    // forward: the 49,990 bytes in between are decompressed and counted
    vfs.read_range(&path, 50_000, 10).unwrap();
    assert_eq!(vfs.cost().0, 50_010);
    // straight on: no extra cost for the part already behind the cursor
    vfs.read_range(&path, 50_010, 10).unwrap();
    assert_eq!(vfs.cost().0, 50_020);
    // backward: restart, everything up to the new position is counted again
    vfs.read_range(&path, 100, 10).unwrap();
    assert_eq!(vfs.cost().0, 50_020 + 110);
    vfs.reset_job_budget();
    assert_eq!(vfs.cost().0, 0);
}

#[test]
fn compressed_tar_counts_the_decompressed_prefix_too() {
    let tmp = tempfile::tempdir().unwrap();
    let data = noise(100_000);
    let items = [file("first.bin", &noise(60_000)), file("big.bin", &data)];
    let vfs = open_bytes(tmp.path(), "a.tar.gz", &gz(&tar_bytes(&items)));
    let path = ap(&vfs, "/big.bin");
    vfs.read_range(&path, 0, 10).unwrap();
    assert!(
        vfs.cost().0 >= 60_000 + 10,
        "reaching the entry decompresses what lies before"
    );
}

#[test]
fn job_budget_stops_reads_by_bytes_and_by_time() {
    let tmp = tempfile::tempdir().unwrap();
    let data = noise(50_000);
    let bytes = zip_bytes(&[file("big.bin", &data)], zip::CompressionMethod::Stored);
    let vfs = open_bytes(tmp.path(), "a.zip", &bytes);
    let path = ap(&vfs, "/big.bin");
    vfs.set_job_budget(Budget::new(10_000, None));
    let error = vfs.read_range(&path, 0, 30_000).unwrap_err();
    assert!(error.to_string().contains("Auftrag"), "{error}");
    vfs.set_job_budget(Budget::new(u64::MAX, Some(Duration::ZERO)));
    assert!(vfs.read_range(&path, 0, 10).is_err());
    assert!(vfs.open_read(&path).unwrap().read(&mut [0u8; 8]).is_err());
    vfs.reset_job_budget();
    assert_eq!(vfs.read_range(&path, 0, 10).unwrap(), data[..10]);
}

#[test]
fn per_operation_budget_applies_to_each_read_range() {
    let tmp = tempfile::tempdir().unwrap();
    let data = noise(50_000);
    let bytes = zip_bytes(&[file("big.bin", &data)], zip::CompressionMethod::Stored);
    let limits = ArchiveLimits {
        read_bytes: 1000,
        ..ArchiveLimits::default()
    };
    let vfs = open_with(tmp.path(), "a.zip", &bytes, limits).unwrap();
    let path = ap(&vfs, "/big.bin");
    assert_eq!(vfs.read_range(&path, 0, 900).unwrap(), data[..900]);
    // each call has its own allowance, a large skip does not
    assert_eq!(vfs.read_range(&path, 900, 900).unwrap(), data[900..1800]);
    assert!(vfs.read_range(&path, 20_000, 10).is_err());
    let slow = ArchiveLimits {
        read_time: Duration::ZERO,
        ..ArchiveLimits::default()
    };
    let bytes = zip_bytes(&[file("big.bin", &data)], zip::CompressionMethod::Stored);
    let vfs = open_with(tmp.path(), "b.zip", &bytes, slow).unwrap();
    assert!(vfs.read_range(&ap(&vfs, "/big.bin"), 0, 10).is_err());
}

#[test]
fn per_job_total_limit_spans_files() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [file("a.bin", &noise(30_000)), file("b.bin", &noise(30_000))];
    let bytes = zip_bytes(&items, zip::CompressionMethod::Stored);
    let limits = ArchiveLimits {
        job_bytes: 40_000,
        ..ArchiveLimits::default()
    };
    let vfs = open_with(tmp.path(), "a.zip", &bytes, limits).unwrap();
    let mut buffer = Vec::new();
    vfs.open_read(&ap(&vfs, "/a.bin"))
        .unwrap()
        .read_to_end(&mut buffer)
        .unwrap();
    let error = vfs
        .open_read(&ap(&vfs, "/b.bin"))
        .unwrap()
        .read_to_end(&mut buffer)
        .unwrap_err();
    assert!(error.to_string().contains("Auftrag"), "{error}");
}

#[test]
fn interleaved_entries_read_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let a = noise(20_000);
    let b: Vec<u8> = noise(20_000).iter().map(|x| x ^ 0x5a).collect();
    let bytes = tar_bytes(&[file("a.bin", &a), file("b.bin", &b)]);
    let vfs = open_bytes(tmp.path(), "i.tar.gz", &gz(&bytes));
    for _ in 0..2 {
        assert_eq!(
            vfs.read_range(&ap(&vfs, "/a.bin"), 100, 50).unwrap(),
            a[100..150]
        );
        assert_eq!(
            vfs.read_range(&ap(&vfs, "/b.bin"), 150, 50).unwrap(),
            b[150..200]
        );
        assert_eq!(
            vfs.read_range(&ap(&vfs, "/a.bin"), 10_000, 50).unwrap(),
            a[10_000..10_050]
        );
    }
}

// ------------------------------------------------------------ detection

#[test]
fn format_detection_uses_signature_and_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = zip_bytes(&[file("a", b"a")], zip::CompressionMethod::Stored);
    let tar = tar_bytes(&[file("a", b"a")]);
    let write = |name: &str, bytes: &[u8]| {
        let path = tmp.path().join(name);
        fs::write(&path, bytes).unwrap();
        Source::File(path)
    };
    // the signature wins over a wrong extension
    assert_eq!(
        detect_format("x.7z", &write("1", &zip)).unwrap(),
        Some(Format::Zip)
    );
    // a plain tar needs its extension or the ustar signature
    assert_eq!(
        detect_format("x.tar", &write("2", &tar)).unwrap(),
        Some(Format::Tar(TarCompression::None))
    );
    assert_eq!(
        detect_format("x.bin", &write("3", &tar)).unwrap(),
        Some(Format::Tar(TarCompression::None))
    );
    // compressed tar without a tar extension is recognized by its content ...
    assert_eq!(
        detect_format("x.bin", &write("4", &gz(&tar))).unwrap(),
        Some(Format::Tar(TarCompression::Gzip))
    );
    // ... a plain compressed file is no archive
    assert_eq!(
        detect_format("x.gz", &write("5", &gz(b"just text, no tar"))).unwrap(),
        None
    );
    assert_eq!(
        detect_format("x.zip", &write("6", b"not a zip at all")).unwrap(),
        None
    );
    assert_eq!(
        detect_format("x.tgz", &write("7", &gz(&tar))).unwrap(),
        Some(Format::Tar(TarCompression::Gzip))
    );
    assert_eq!(
        detect_format("x.rar", &write("8", b"Rar!\x1a\x07\x00rest")).unwrap(),
        Some(Format::Rar)
    );
    let error = open_with(
        tmp.path(),
        "plain.zip",
        b"hello world",
        ArchiveLimits::default(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("kein erkanntes Archiv"));
}

// ------------------------------------------------------------ read only

#[test]
fn all_writing_methods_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(
        tmp.path(),
        "a.zip",
        &zip_bytes(&standard_items(), zip::CompressionMethod::Stored),
    );
    assert!(vfs.is_read_only() && !vfs.is_local());
    let p = ap(&vfs, "/hello.txt");
    let q = ap(&vfs, "/new");
    let refused = |result: Result<()>| {
        assert!(
            matches!(&result, Err(Error::Unsupported(m)) if *m == "Archive sind schreibgeschützt"),
            "{result:?}"
        )
    };
    refused(vfs.create_write(&q, WriteMode::NoReplace).map(|_| ()));
    refused(vfs.create_write(&p, WriteMode::Replace).map(|_| ()));
    refused(vfs.mkdir(&q));
    refused(vfs.rename(&p, &q, ReplaceMode::NoReplace));
    refused(vfs.create_symlink("x", &q));
    refused(vfs.trash(&p));
    refused(vfs.remove_permanent(&p));
    refused(vfs.set_metadata(&p, Some(0o600), None));
    // paths of another archive are not served
    let other = open_bytes(
        tmp.path(),
        "b.zip",
        &zip_bytes(&standard_items(), zip::CompressionMethod::Stored),
    );
    assert!(vfs.stat(&ap(&other, "/hello.txt")).is_err());
    assert!(vfs.stat(&VPath::local(tmp.path()).unwrap()).is_err());
}

#[test]
fn ops_refuse_to_change_archives_in_the_core() {
    let tmp = tempfile::tempdir().unwrap();
    let vfs = open_bytes(
        tmp.path(),
        "a.zip",
        &zip_bytes(&standard_items(), zip::CompressionMethod::Stored),
    );
    let local = LocalVfs;
    let job = JobHandle::silent();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("f.txt"), b"f").unwrap();
    let local_file = VPath::local(outside.join("f.txt")).unwrap();
    let archive_root = vfs.root();
    // copy INTO an archive
    let report = ops::copy(
        &local,
        std::slice::from_ref(&local_file),
        &vfs,
        &archive_root,
        None,
        &job,
    );
    assert_eq!((report.done.len(), report.failed.len()), (0, 1));
    // move OUT of an archive must neither copy nor delete
    let target = VPath::local(&outside).unwrap();
    let report = ops::move_(&vfs, &[ap(&vfs, "/hello.txt")], &local, &target, None, &job);
    assert_eq!((report.done.len(), report.failed.len()), (0, 1));
    assert!(!outside.join("hello.txt").exists());
    // move into
    let report = ops::move_(
        &local,
        std::slice::from_ref(&local_file),
        &vfs,
        &archive_root,
        None,
        &job,
    );
    assert_eq!(report.failed.len(), 1);
    assert!(outside.join("f.txt").exists());
    // trash, delete, rename, make_dir
    assert_eq!(
        ops::trash_items(&vfs, &[ap(&vfs, "/hello.txt")], &job)
            .failed
            .len(),
        1
    );
    let plan = ops::plan_delete(&vfs, &[ap(&vfs, "/hello.txt")]);
    let report = ops::delete_permanent(&vfs, &plan, plan.token, &job);
    assert!(report.done.is_empty());
    assert!(ops::rename_entry(&vfs, &ap(&vfs, "/hello.txt"), "x").is_err());
    assert!(ops::make_dir(&vfs, &archive_root, "new").is_err());
}

// ------------------------------------------------------------ copy out

fn copy_all(vfs: &ArchiveVfs, sources: &[&str], dst: &Path) -> ops::OpReport {
    let sources: Vec<VPath> = sources.iter().map(|s| ap(vfs, s)).collect();
    ops::copy(
        vfs,
        &sources,
        &LocalVfs,
        &VPath::local(dst).unwrap(),
        None,
        &JobHandle::silent(),
    )
}

#[test]
fn copy_from_archive_to_local_gives_equal_content() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        (
            "a.zip",
            zip_bytes(&standard_items(), zip::CompressionMethod::Deflated),
        ),
        ("a.tar", tar_bytes(&standard_items())),
        ("a.tar.gz", gz(&tar_bytes(&standard_items()))),
        ("a.7z", sevenz_bytes(&standard_items())),
    ] {
        let vfs = open_bytes(tmp.path(), name, &bytes);
        let dst = tmp.path().join(format!("out-{name}"));
        fs::create_dir(&dst).unwrap();
        let report = copy_all(
            &vfs,
            &["/hello.txt", "/dir", "/empty.txt", "/empty_dir", "/link"],
            &dst,
        );
        assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
        assert_eq!(fs::read(dst.join("hello.txt")).unwrap(), HELLO, "{name}");
        assert_eq!(fs::read(dst.join("dir/nested/deep.txt")).unwrap(), DEEP);
        assert_eq!(fs::read(dst.join("empty.txt")).unwrap(), b"");
        assert!(dst.join("empty_dir").is_dir());
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            Path::new("hello.txt")
        );
        assert_eq!(
            fs::read(dst.join("link")).unwrap(),
            HELLO,
            "relative link resolves"
        );
    }
}

#[test]
fn copy_never_takes_unsafe_entries_and_no_special_bits() {
    let tmp = tempfile::tempdir().unwrap();
    let items = [
        file("good.txt", b"good"),
        file("../escaped.txt", b"bad"),
        file("/abs-escaped.txt", b"bad"),
        with_mode(file("suid", b"s"), 0o106_755),
        link("abslink", "/etc/passwd"),
        link("upslink", "../outside"),
        link("okalink", "good.txt"),
        file("abslink/through", b"bad"),
    ];
    let vfs = open_bytes(tmp.path(), "evil.tar", &tar_bytes(&items));
    let dst = tmp.path().join("out");
    fs::create_dir(&dst).unwrap();
    let report = copy_all(&vfs, &["/"], &dst);
    let root = dst.join("evil.tar");
    assert_eq!(fs::read(root.join("good.txt")).unwrap(), b"good");
    // every unsafe entry failed, none was written anywhere
    assert!(report.failed.len() >= 3, "{:?}", report.failed);
    assert!(!tmp.path().join("escaped.txt").exists());
    assert!(!dst.join("escaped.txt").exists());
    assert!(!root.join("escaped.txt").exists());
    assert!(!Path::new("/abs-escaped.txt").exists());
    assert!(fs::read_dir(&root).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("[unsicher")));
    // links that could leave the tree are skipped with a warning, safe ones are created
    assert!(fs::symlink_metadata(root.join("abslink")).is_err());
    assert!(fs::symlink_metadata(root.join("upslink")).is_err());
    assert!(report.warnings.len() >= 2);
    assert_eq!(
        fs::read_link(root.join("okalink")).unwrap(),
        Path::new("good.txt")
    );
    // setuid/setgid/sticky are not taken over
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(root.join("suid"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7000, 0, "{mode:o}");
}

// ------------------------------------------------------------ RAR (dummy programs)

mod rar_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Starting programs that were just written races with other threads that fork;
    /// run the RAR tests one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    const LISTING_7Z: &str = "\n7-Zip [64] 17.05 : Copyright (c) 1999-2021 Igor Pavlov\n\nScanning the drive for archives:\n1 file, 100 bytes (1 KiB)\n\nListing archive: x.rar\n\n--\nPath = x.rar\nType = Rar\nPhysical Size = 100\n\n----------\nPath = docs\nFolder = +\nSize = 0\nModified = 2024-03-05 10:11:12\nAttributes = D drwxr-xr-x\nEncrypted = -\n\nPath = docs/readme.txt\nFolder = -\nSize = 5\nModified = 2024-03-05 10:11:12\nAttributes = A -rw-r--r--\nEncrypted = -\n\nPath = -dash.txt\nFolder = -\nSize = 3\nModified = 2024-03-05 10:11:12\nAttributes = A -rw-r--r--\nEncrypted = -\n\nPath = lock.txt\nFolder = -\nSize = 4\nEncrypted = +\n\nPath = star*.txt\nFolder = -\nSize = 1\nEncrypted = -\n\n";

    fn script(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn plain_env() -> crate::helpers::HelperEnv {
        crate::helpers::HelperEnv {
            packaging: dualfile_platform::Packaging::Plain,
            appdir: None,
            exe_dir: None,
            flatpak_bin: std::path::PathBuf::from("/nonexistent"),
        }
    }

    fn config(dir: &Path) -> RarConfig {
        RarConfig {
            search_path: Some(dir.as_os_str().to_owned()),
            // no shipped helper, whatever the test machine looks like
            helper_env: Some(plain_env()),
            timeout: Duration::from_millis(400),
            output_limit: 1 << 20,
        }
    }

    fn rar_file(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"Rar!\x1a\x07\x00 not really a rar archive").unwrap();
        path
    }

    fn open_rar(path: &Path, cfg: RarConfig) -> Result<ArchiveVfs> {
        ArchiveVfs::open_local_with(
            path,
            ArchiveLimits {
                rar: cfg,
                ..ArchiveLimits::default()
            },
        )
    }

    #[test]
    fn reads_a_real_rar_archive_with_the_system_tool() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Needs a real unrar/7z on PATH; skip where none is installed (e.g. a minimal CI image).
        if rar::find_tool(&RarConfig::default()).is_none() {
            return;
        }
        let fixture = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/archive/testdata/sample.rar"
        ));
        let vfs = open_rar(fixture, RarConfig::default()).unwrap();
        assert!(names(&vfs, "/").contains(&"docs".to_string()));
        assert!(names(&vfs, "/").contains(&"top.txt".to_string()));
        assert_eq!(names(&vfs, "/docs"), vec!["readme.txt".to_string()]);
        assert_eq!(vfs.stat(&ap(&vfs, "/docs/readme.txt")).unwrap().size, 5);
        assert_eq!(read_all(&vfs, "/docs/readme.txt"), b"hello");
        assert_eq!(read_all(&vfs, "/top.txt"), b"AB");
    }

    #[test]
    fn finds_unrar_first_then_7zz_then_7z_and_none() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tools = tempfile::tempdir().unwrap();
        let cfg = config(tools.path());
        assert!(rar::find_tool(&cfg).is_none());
        script(tools.path(), "7z", "exit 0");
        assert_eq!(rar::find_tool(&cfg).unwrap().kind, rar::ToolKind::SevenZip);
        assert!(rar::find_tool(&cfg).unwrap().program.ends_with("7z"));
        script(tools.path(), "7zz", "exit 0");
        assert!(rar::find_tool(&cfg).unwrap().program.ends_with("7zz"));
        script(tools.path(), "unrar", "exit 0");
        let tool = rar::find_tool(&cfg).unwrap();
        assert_eq!(tool.kind, rar::ToolKind::Unrar);
        // a file without the execute bit does not count
        let plain = tempfile::tempdir().unwrap();
        fs::write(plain.path().join("unrar"), "x").unwrap();
        assert!(rar::find_tool(&config(plain.path())).is_none());
    }

    #[test]
    fn shipped_7zz_comes_after_system_unrar_and_before_path_7zz() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sys = tempfile::tempdir().unwrap();
        let shipped = tempfile::tempdir().unwrap();
        script(shipped.path(), "7zz", "exit 0");
        let mut cfg = config(sys.path());
        cfg.helper_env = Some(crate::helpers::HelperEnv {
            packaging: dualfile_platform::Packaging::Flatpak,
            flatpak_bin: shipped.path().to_path_buf(),
            ..plain_env()
        });
        // only the shipped copy
        let tool = rar::find_tool(&cfg).unwrap();
        assert_eq!(tool.kind, rar::ToolKind::SevenZip);
        assert_eq!(tool.program, shipped.path().join("7zz"));
        // a 7zz / 7z in PATH does not replace it
        script(sys.path(), "7zz", "exit 0");
        script(sys.path(), "7z", "exit 0");
        assert_eq!(
            rar::find_tool(&cfg).unwrap().program,
            shipped.path().join("7zz")
        );
        // an installed unrar still wins, as before
        script(sys.path(), "unrar", "exit 0");
        let tool = rar::find_tool(&cfg).unwrap();
        assert_eq!(tool.kind, rar::ToolKind::Unrar);
        assert_eq!(tool.program, sys.path().join("unrar"));
    }

    #[test]
    fn missing_helper_gives_the_clear_message() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let path = rar_file(tmp.path(), "a.rar");
        let error = open_rar(&path, config(empty.path())).err().unwrap();
        assert!(matches!(&error, Error::Archive(m) if *m == "RAR benötigt unrar oder 7z"));
    }

    #[test]
    fn lists_and_reads_through_a_7z_helper_with_a_safe_argument_list() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tools = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("calls.log");
        let listing = tmp.path().join("listing.txt");
        fs::write(&listing, LISTING_7Z).unwrap();
        script(
            tools.path(),
            "7z",
            &format!(
                "echo \"$@\" >> '{log}'\ncase \"$1\" in\n l) cat '{listing}';;\n e) printf 'hello' ;;\nesac",
                log = log.display(),
                listing = listing.display()
            ),
        );
        let path = rar_file(tmp.path(), "weird.rar");
        let vfs = open_rar(&path, config(tools.path())).unwrap();
        assert_eq!(vfs.format(), Format::Rar);
        assert_eq!(
            names(&vfs, "/"),
            vec!["-dash.txt", "docs", "lock.txt", "star*.txt"]
        );
        let readme = vfs.stat(&ap(&vfs, "/docs/readme.txt")).unwrap();
        assert_eq!(readme.size, 5);
        assert_eq!(read_all(&vfs, "/docs/readme.txt"), b"hello");
        // a wrong announced size is detected
        assert!(vfs
            .open_read(&ap(&vfs, "/-dash.txt"))
            .unwrap()
            .read_to_end(&mut Vec::new())
            .is_err());
        // encrypted entries and names with wildcards are never handed to the helper
        assert_eq!(
            vfs.stat(&ap(&vfs, "/lock.txt")).unwrap().error.as_deref(),
            Some("verschlüsselt, nicht unterstützt")
        );
        assert!(vfs.open_read(&ap(&vfs, "/star*.txt")).is_err());
        let calls = fs::read_to_string(&log).unwrap();
        let list_call = calls.lines().next().unwrap();
        let archive_arg = path.to_string_lossy();
        assert_eq!(list_call, format!("l -slt -bd -y -- {archive_arg}"));
        let extract = calls.lines().find(|l| l.starts_with("e ")).unwrap();
        assert_eq!(
            extract,
            format!("e -so -bd -y -- {archive_arg} docs/readme.txt")
        );
        // the option separator always comes right before the archive and names
        let args = rar::extract_args(
            &rar::Tool {
                kind: rar::ToolKind::Unrar,
                program: "unrar".into(),
            },
            Path::new("-evil.rar"),
            "-rf",
        );
        let args: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["p", "-inul", "-p-", "--", "./-evil.rar", "-rf"]);
    }

    #[test]
    fn parses_unrar_and_7z_listings() {
        let items = rar::parse_7z_listing(LISTING_7Z);
        assert_eq!(items.len(), 5);
        assert!(items[0].is_dir && items[0].mode == Some(0o755));
        assert_eq!(
            (items[1].name.as_str(), items[1].size),
            ("docs/readme.txt", 5)
        );
        assert_eq!(items[1].mode, Some(0o644));
        assert!(items[3].encrypted);
        let unrar = "UNRAR 6.24 freeware\n\nArchive: x.rar\nDetails: RAR 5\n\n        Name: a.txt\n        Type: File\n        Size: 3\n Packed size: 3\n       mtime: 2023-01-02 03:04:05,000000000\n  Attributes: -rw-r--r--\n       Flags: encrypted\n\n        Name: sub\n        Type: Directory\n        Size: 0\n  Attributes: drwxr-xr-x\n\n        Name: l\n        Type: Unix symbolic link\n        Size: 1\n";
        let items = rar::parse_unrar_listing(unrar);
        assert_eq!(items.len(), 3);
        assert!(items[0].encrypted && items[0].size == 3 && items[0].modified.is_some());
        assert!(items[1].is_dir);
        assert!(items[2].symlink);
    }

    #[test]
    fn a_hanging_helper_is_killed_at_the_time_limit() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tools = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        script(tools.path(), "unrar", "exec sleep 30");
        let path = rar_file(tmp.path(), "a.rar");
        let started = std::time::Instant::now();
        let error = open_rar(&path, config(tools.path())).err().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(error.to_string().contains("Zeitlimit"), "{error}");
    }

    #[test]
    fn failing_helper_and_oversized_output_are_errors() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tools = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = rar_file(tmp.path(), "a.rar");
        script(tools.path(), "unrar", "echo broken; exit 3");
        let error = open_rar(&path, config(tools.path())).err().unwrap();
        assert!(error.to_string().contains("Fehler"), "{error}");
        fs::remove_file(tools.path().join("unrar")).unwrap();
        script(tools.path(), "unrar", "exec yes");
        let mut cfg = config(tools.path());
        cfg.output_limit = 4096;
        cfg.timeout = Duration::from_secs(5);
        let error = open_rar(&path, cfg).err().unwrap();
        assert!(error.to_string().contains("zu groß"), "{error}");
    }

    #[test]
    fn helper_runs_without_environment_in_an_empty_work_folder() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tools = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let report = tmp.path().join("env.txt");
        script(
            tools.path(),
            "unrar",
            &format!(
                "env > '{r}'; echo \"cwd=$(pwd)\" >> '{r}'; ls -A >> '{r}'; exit 0",
                r = report.display()
            ),
        );
        let path = rar_file(tmp.path(), "a.rar");
        let _ = open_rar(&path, config(tools.path()));
        let text = fs::read_to_string(&report).unwrap();
        assert!(text.contains("LC_ALL=C"));
        let allowed = ["LC_ALL", "LANG", "HOME", "PWD", "OLDPWD", "SHLVL", "_"];
        for line in text.lines().take_while(|l| !l.starts_with("cwd=")) {
            let key = line.split('=').next().unwrap();
            assert!(allowed.contains(&key), "unexpected variable {key}");
        }
        let cwd = text.lines().find(|l| l.starts_with("cwd=")).unwrap();
        assert!(cwd.contains("dualfile-rar-"), "{cwd}");
        let after_cwd: Vec<_> = text
            .lines()
            .skip_while(|l| !l.starts_with("cwd="))
            .skip(1)
            .collect();
        assert!(
            after_cwd.is_empty(),
            "the work folder must be empty: {after_cwd:?}"
        );
        // and it is removed again
        assert!(!Path::new(cwd.trim_start_matches("cwd=")).exists());
    }
}

#[test]
fn sevenz_encoded_header_is_bounded_before_decompression() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        index_dir_bytes: 16 * 1024,
        ..ArchiveLimits::default()
    };
    // Repeated names make header compression worthwhile; a tiny header is stored plain.
    let expected: Vec<_> = (0..32).map(|i| format!("normal-{i:02}.txt")).collect();
    let items: Vec<_> = expected.iter().map(|name| file(name, b"ok")).collect();
    let normal = sevenz_bytes(&items);
    let offset = u64::from_le_bytes(normal[12..20].try_into().unwrap()) as usize + 32;
    assert_eq!(normal[offset], 0x17, "fixture must have an encoded header");
    let vfs = open_with(tmp.path(), "normal.7z", &normal, limits.clone()).unwrap();
    assert_eq!(names(&vfs, "/"), expected);

    // A complete encoded-header descriptor with an LZMA coder and no large payload.
    let mut descriptor = vec![
        0x17, 0x06, 0, 1, 0x09, 1, 0, 0x07, 0x0b, 1, 0, 1, 0x23, 0x03, 0x01, 0x01, 5, 0x5d, 0, 0,
        0x80, 0, 0x0c, 0xff,
    ];
    descriptor.extend_from_slice(&(limits.index_dir_bytes + 1).to_le_bytes());
    descriptor.extend_from_slice(&[0, 0]);
    let mut bytes = vec![0u8; 32];
    bytes[..8].copy_from_slice(b"7z\xbc\xaf\x27\x1c\x00\x04");
    bytes[20..28].copy_from_slice(&(descriptor.len() as u64).to_le_bytes());
    bytes[28..32].copy_from_slice(&crc32fast::hash(&descriptor).to_le_bytes());
    let crc = crc32fast::hash(&bytes[12..32]);
    bytes[8..12].copy_from_slice(&crc.to_le_bytes());
    bytes.extend_from_slice(&descriptor);
    assert!(matches!(
        open_with(tmp.path(), "large.7z", &bytes, limits),
        Err(Error::Archive(super::budget::MSG_INDEX_BYTES))
    ));
}

#[test]
fn tar_metadata_size_is_bounded_before_reading_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        max_entry_meta: 256,
        ..ArchiveLimits::default()
    };
    for kind in *b"LKxg" {
        // No payload: the limit error must take precedence over a truncated-data error.
        let oversized = tar_header("metadata", kind, 257, 0o644, "");
        for (name, bytes) in [
            ("bad.tar", oversized.to_vec()),
            ("bad.tar.gz", gz(&oversized)),
            ("bad.tar.bz2", bz2(&oversized)),
            ("bad.tar.xz", xz(&oversized)),
        ] {
            assert!(
                matches!(
                    open_with(tmp.path(), name, &bytes, limits.clone()),
                    Err(Error::Archive(super::budget::MSG_INDEX_BYTES))
                ),
                "metadata kind {kind}, {name}"
            );
        }
    }
    let long_name = "n".repeat(150);
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(2);
    header.set_cksum();
    builder
        .append_data(&mut header, &long_name, &b"ok"[..])
        .unwrap();
    let normal = builder.into_inner().unwrap();
    for (name, bytes) in [
        ("normal.tar", normal.clone()),
        ("normal.tar.gz", gz(&normal)),
    ] {
        let vfs = open_with(tmp.path(), name, &bytes, limits.clone()).unwrap();
        assert_eq!(names(&vfs, "/"), vec![long_name.clone()]);
    }

    // A PAX size override places the next metadata header beyond the legacy size field.
    let mut pax = tar_header("pax", b'x', 12, 0o644, "").to_vec();
    pax.extend_from_slice(b"12 size=512\n");
    pax.resize(1024, 0);
    pax.extend_from_slice(&tar_header("data", b'0', 0, 0o644, ""));
    pax.extend_from_slice(&[0; 512]);
    pax.extend_from_slice(&tar_header("metadata", b'L', 257, 0o644, ""));
    assert!(matches!(
        open_with(tmp.path(), "pax.tar.gz", &gz(&pax), limits),
        Err(Error::Archive(super::budget::MSG_INDEX_BYTES))
    ));
}

#[test]
fn compressed_tar_index_charges_one_decompression_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tar_bytes(&[file("data.txt", &[b'a'; 16 * 1024])]);
    let limits = ArchiveLimits {
        index_bytes_tar: data.len() as u64 + 512,
        ..ArchiveLimits::default()
    };
    assert!(limits.index_bytes_tar < 2 * data.len() as u64);
    for (name, bytes) in [
        ("once.tar.gz", gz(&data)),
        ("once.tar.bz2", bz2(&data)),
        ("once.tar.xz", xz(&data)),
    ] {
        let vfs = open_with(tmp.path(), name, &bytes, limits.clone()).unwrap();
        assert_eq!(names(&vfs, "/"), vec!["data.txt"]);
    }
}
