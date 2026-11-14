use super::*;
use std::fs;
fn run(p: &Path) -> DupeResult {
    find_duplicates(
        &[p.to_path_buf()],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap()
}
fn put(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
}
#[test]
fn three_folders_and_reclaimable_sorting() {
    let d = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        let p = d.path().join(name);
        fs::create_dir(&p).unwrap();
        put(&p.join("same"), b"identical");
        put(&p.join("tiny"), b"xx");
    }
    let r = run(d.path());
    assert!(!r.incomplete);
    assert_eq!(r.groups.len(), 2);
    assert_eq!(r.groups[0].size, 9);
    assert_eq!(r.groups[0].entries.len(), 3);
    assert_eq!(r.groups[0].reclaimable_bytes, 18);
    assert_eq!(r.groups[1].reclaimable_bytes, 4);
    assert_eq!(
        r.groups[0].hash,
        Sha256::digest(b"identical")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
}
#[test]
fn equal_size_different_content_and_empty_files_excluded() {
    let d = tempfile::tempdir().unwrap();
    for (name, data) in [("a", "abc"), ("b", "abd"), ("e1", ""), ("e2", "")] {
        put(&d.path().join(name), data.as_bytes());
    }
    assert!(run(d.path()).groups.is_empty());
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions {
            min_size: 0,
            ..Default::default()
        },
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap();
    assert!(r.groups.is_empty());
}
#[test]
fn same_prefix_different_end_and_large_different_middle() {
    for (size, difference, expected_full) in [
        (128 << 10, (128 << 10) - 1, true),
        (2 << 20, 1 << 20, true),
        (2 << 20, (2 << 20) - 1, false),
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut data = vec![42; size];
        put(&d.path().join("a"), &data);
        data[difference] = 43;
        put(&d.path().join("b"), &data);
        let mut full_bytes = 0;
        let r = find_duplicates(
            &[d.path().to_path_buf()],
            &DupeOptions::default(),
            &JobHandle::silent(),
            &mut |p| {
                if p.stage == DupeStage::FullHash {
                    full_bytes = p.total.unwrap();
                }
            },
        )
        .unwrap();
        assert!(r.groups.is_empty());
        assert_eq!(full_bytes > 0, expected_full);
    }
}
#[cfg(unix)]
#[test]
fn hardlinks_and_overlapping_roots_are_one_entry_and_symlinks_excluded() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    put(&d.path().join("a"), b"same");
    fs::hard_link(d.path().join("a"), d.path().join("alias")).unwrap();
    symlink("a", d.path().join("sym")).unwrap();
    symlink(d.path(), d.path().join("loop")).unwrap();
    assert!(run(d.path()).groups.is_empty());
    put(&d.path().join("b"), b"same");
    let r = find_duplicates(
        &[
            d.path().to_path_buf(),
            d.path().to_path_buf(),
            d.path().join("a"),
        ],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(r.groups.len(), 1);
    let g = &r.groups[0];
    assert_eq!(g.reclaimable_bytes, 4);
    assert_eq!(g.entries.len(), 2);
    assert_eq!(g.entries[0].path, d.path().join("a"));
    assert_eq!(g.entries[0].linked_paths, [d.path().join("alias")]);
    assert_eq!(g.entries[0].hard_links, 2);
}
#[test]
fn modifications_during_each_hash_stage_are_discarded() {
    for stage in [DupeStage::PartialHash, DupeStage::FullHash] {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        put(&a, &vec![0; 256 << 10]);
        put(&b, &vec![0; 256 << 10]);
        let mut changed = false;
        let mut start = None;
        let r = find_duplicates(
            &[d.path().to_path_buf()],
            &DupeOptions::default(),
            &JobHandle::silent(),
            &mut |p| {
                if p.stage == stage {
                    let initial = *start.get_or_insert(p.bytes_read);
                    if !changed && p.bytes_read > initial {
                        changed = true;
                        put(&a, b"changed");
                        put(&b, b"changed");
                    }
                }
            },
        )
        .unwrap();
        assert!(changed);
        assert!(r.groups.is_empty());
        assert!(r.incomplete);
        assert!(r.skipped.iter().any(|(_, e)| *e == ScanIssue::Changed));
    }
}
#[test]
fn same_size_mtime_change_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    let b = d.path().join("b");
    put(&a, b"aaaa");
    put(&b, b"aaaa");
    let mut changed = false;
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |p| {
            if p.stage == DupeStage::PartialHash && p.bytes_read > 0 && !changed {
                changed = true;
                for file in [&a, &b] {
                    fs::File::options()
                        .write(true)
                        .open(file)
                        .unwrap()
                        .set_modified(std::time::UNIX_EPOCH)
                        .unwrap();
                }
            }
        },
    )
    .unwrap();
    assert!(r.groups.is_empty());
    assert!(r.skipped.iter().any(|(_, e)| *e == ScanIssue::Changed));
}
#[test]
fn cancellation_in_enumeration_and_hashing() {
    for stage in [DupeStage::Size, DupeStage::PartialHash, DupeStage::FullHash] {
        let d = tempfile::tempdir().unwrap();
        put(&d.path().join("a"), &vec![0; 128 << 10]);
        put(&d.path().join("b"), &vec![0; 128 << 10]);
        let job = JobHandle::silent();
        let r = find_duplicates(
            &[d.path().to_path_buf()],
            &DupeOptions::default(),
            &job,
            &mut |p| {
                if p.stage == stage && (stage == DupeStage::Size || p.bytes_read > 0) {
                    job.cancel();
                }
            },
        )
        .unwrap();
        assert!(r.cancelled && r.incomplete);
    }
}
#[test]
fn file_read_error_is_reported_and_other_candidates_survive() {
    let d = tempfile::tempdir().unwrap();
    for n in ["a", "b", "c"] {
        put(&d.path().join(n), b"same");
    }
    let mut removed = false;
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |p| {
            if p.stage == DupeStage::PartialHash && !removed {
                removed = true;
                fs::remove_file(d.path().join("a")).unwrap();
            }
        },
    )
    .unwrap();
    assert!(r.incomplete);
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].entries.len(), 2);
    assert_eq!(r.skipped[0].1, ScanIssue::Io(std::io::ErrorKind::NotFound));
}
#[test]
fn budget_depth_minimum_and_error_cap() {
    let d = tempfile::tempdir().unwrap();
    put(&d.path().join("a"), b"x");
    put(&d.path().join("b"), b"x");
    for opts in [
        DupeOptions {
            max_entries: 1,
            ..Default::default()
        },
        DupeOptions {
            max_depth: 0,
            ..Default::default()
        },
    ] {
        let r = find_duplicates(
            &[d.path().to_path_buf()],
            &opts,
            &JobHandle::silent(),
            &mut |_| {},
        )
        .unwrap();
        assert!(r.incomplete);
    }
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions {
            min_size: 2,
            ..Default::default()
        },
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap();
    assert!(r.groups.is_empty());
    let roots: Vec<_> = (0..250).map(|i| d.path().join(i.to_string())).collect();
    let r = find_duplicates(
        &roots,
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(r.skipped_count, 250);
    assert_eq!(r.skipped.len(), 200);
}
#[test]
fn progress_has_all_stages_and_exact_hash_counts() {
    let d = tempfile::tempdir().unwrap();
    put(&d.path().join("a"), b"same");
    put(&d.path().join("b"), b"same");
    let mut events = Vec::new();
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |p| events.push(p),
    )
    .unwrap();
    assert!(events
        .iter()
        .any(|p| p.stage == DupeStage::Size && p.total.is_none()));
    for stage in [DupeStage::PartialHash, DupeStage::FullHash] {
        assert!(events
            .iter()
            .any(|p| p.stage == stage && p.done == 2 && p.total == Some(2)));
    }
    assert!(events
        .windows(2)
        .all(|p| p[0].bytes_read <= p[1].bytes_read));
    assert_eq!(r.bytes_read, 16);
}
#[test]
fn keep_rules_ties_unknown_dates_and_alias_folder() {
    let entry = |p: &str, seconds| DupeEntry {
        path: p.into(),
        linked_paths: vec![],
        modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
        hard_links: 1,
    };
    let mut g = DupeGroup {
        hash: String::new(),
        size: 1,
        reclaimable_bytes: 2,
        entries: vec![entry("/z/long", 1), entry("/a/long", 1), entry("/b/x", 2)],
    };
    for (rule, expected) in [
        (KeepRule::Oldest, "/a/long"),
        (KeepRule::Newest, "/b/x"),
        (KeepRule::ShortestPath, "/b/x"),
        (KeepRule::InFolder("/z".into()), "/z/long"),
        (KeepRule::InFolder("/none".into()), "/a/long"),
    ] {
        let s = suggest_keep(&g, rule).unwrap();
        assert_eq!(s.keep.path, Path::new(expected));
        assert_eq!(s.redundant.len(), 2);
    }
    g.entries[0].linked_paths.push("/preferred/alias".into());
    assert_eq!(
        suggest_keep(&g, KeepRule::InFolder("/preferred".into()))
            .unwrap()
            .keep
            .path,
        Path::new("/z/long")
    );
    for e in &mut g.entries {
        e.modified = None;
    }
    for rule in [KeepRule::Oldest, KeepRule::Newest] {
        assert_eq!(
            suggest_keep(&g, rule).unwrap().keep.path,
            Path::new("/a/long")
        );
    }
    g.entries[0].modified = Some(std::time::UNIX_EPOCH);
    for rule in [KeepRule::Oldest, KeepRule::Newest] {
        assert_eq!(
            suggest_keep(&g, rule).unwrap().keep.path,
            Path::new("/z/long")
        );
    }
    g.entries = vec![entry("/z/x", 2), entry("/a/x", 2)];
    for rule in [
        KeepRule::Oldest,
        KeepRule::Newest,
        KeepRule::ShortestPath,
        KeepRule::InFolder("/".into()),
    ] {
        assert_eq!(suggest_keep(&g, rule).unwrap().keep.path, Path::new("/a/x"));
    }
    g.entries.clear();
    assert!(suggest_keep(&g, KeepRule::Oldest).is_none());
}
#[cfg(unix)]
#[test]
fn replacing_parent_with_symlink_during_hash_does_not_escape() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sub = d.path().join("sub");
    fs::create_dir(&sub).unwrap();
    for n in ["a", "b"] {
        put(&sub.join(n), b"same");
        put(&outside.path().join(n), b"same");
    }
    let mut swapped = false;
    let r = find_duplicates(
        &[d.path().to_path_buf()],
        &DupeOptions::default(),
        &JobHandle::silent(),
        &mut |p| {
            if p.stage == DupeStage::PartialHash && !swapped {
                swapped = true;
                fs::rename(&sub, d.path().join("old")).unwrap();
                symlink(outside.path(), &sub).unwrap();
            }
        },
    )
    .unwrap();
    assert!(r.groups.is_empty());
    assert_eq!(r.bytes_read, 0);
    assert!(r.incomplete);
}

#[cfg(unix)]
#[test]
fn fifo_is_not_opened_and_unreadable_file_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        d.path().join("pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let r = run(d.path());
    assert!(r.groups.is_empty());
    assert_eq!(r.bytes_read, 0);
    if rustix::process::geteuid().is_root() {
        return;
    }
    let locked = d.path().join("locked");
    put(&locked, b"same");
    put(&d.path().join("readable"), b"same");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o0)).unwrap();
    let r = run(d.path());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(r.incomplete);
    assert!(r.groups.is_empty());
    assert!(r
        .skipped
        .iter()
        .any(|(_, kind)| *kind == ScanIssue::Io(std::io::ErrorKind::PermissionDenied)));
}
