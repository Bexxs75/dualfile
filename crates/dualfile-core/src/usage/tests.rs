use super::*;
use std::fs;
fn measure(p: &Path, o: &UsageOptions) -> UsageResult {
    measure_usage(p, o, &JobHandle::silent(), &mut |_| {}).unwrap()
}
fn file(p: &Path, size: usize) {
    fs::write(p, vec![42; size]).unwrap();
}

#[test]
fn nested_totals_sorted_drilldown_and_allocated() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("a/nested")).unwrap();
    file(&d.path().join("a/nested/big"), 9000);
    file(&d.path().join("small"), 12);
    file(&d.path().join("medium"), 40);
    let r = measure(d.path(), &UsageOptions::default());
    assert!(!r.incomplete);
    assert_eq!(r.root.totals.size, 9052);
    assert_eq!(r.root.totals.files, 3);
    assert_eq!(r.root.totals.directories, 3);
    assert_eq!(
        r.root
            .children
            .iter()
            .map(|n| n.totals.size)
            .collect::<Vec<_>>(),
        [9000, 40, 12]
    );
    assert_eq!(r.root.children[0].children[0].children[0].totals.size, 9000);
    let allocated: u64 = ["a/nested/big", "small", "medium"]
        .iter()
        .map(|p| scan_fs::metadata(&d.path().join(p)).unwrap().allocated)
        .sum();
    assert_eq!(r.root.totals.allocated, allocated);
}
#[cfg(unix)]
#[test]
fn links_are_not_followed_and_hardlinks_count_once() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    file(&outside.path().join("huge"), 100000);
    file(&d.path().join("a"), 15);
    fs::hard_link(d.path().join("a"), d.path().join("b")).unwrap();
    symlink(d.path(), d.path().join("loop")).unwrap();
    symlink(outside.path(), d.path().join("outside")).unwrap();
    let r = measure(d.path(), &UsageOptions::default());
    assert_eq!(
        r.root.totals.size,
        15 + fs::symlink_metadata(d.path().join("loop")).unwrap().len()
            + fs::symlink_metadata(d.path().join("outside"))
                .unwrap()
                .len()
    );
    assert_eq!(r.root.totals.files, 3);
    assert_eq!(r.root.totals.directories, 1);
    assert!(!r.incomplete);
    let root_link = outside.path().join("root");
    symlink(d.path(), &root_link).unwrap();
    assert_eq!(
        measure(&root_link, &UsageOptions::default()).root.totals,
        r.root.totals
    );
}
#[test]
fn budget_keeps_large_children_and_exact_other_total() {
    let d = tempfile::tempdir().unwrap();
    for i in 1..=10 {
        file(&d.path().join(i.to_string()), i);
    }
    let r = measure(
        d.path(),
        &UsageOptions {
            max_entries: 3,
            ..Default::default()
        },
    );
    assert_eq!(r.root.stored_entries(), 3);
    assert_eq!(r.root.totals.size, 55);
    assert_eq!(
        r.root
            .children
            .iter()
            .map(|n| n.totals.size)
            .collect::<Vec<_>>(),
        [10, 9]
    );
    assert_eq!(r.root.other.unwrap().size, 36);
    assert!(!r.incomplete);
    let r = measure(
        d.path(),
        &UsageOptions {
            max_entries: 1,
            ..Default::default()
        },
    );
    assert_eq!(r.root.stored_entries(), 1);
    assert_eq!(r.root.other.unwrap().size, 55);
}
#[test]
fn nested_budget_is_global_and_totals_survive() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..8 {
        let dir = d.path().join(i.to_string());
        fs::create_dir(&dir).unwrap();
        for j in 0..8 {
            file(&dir.join(j.to_string()), 2);
        }
    }
    for budget in 1..20 {
        let r = measure(
            d.path(),
            &UsageOptions {
                max_entries: budget,
                ..Default::default()
            },
        );
        assert!(r.root.stored_entries() <= budget);
        assert_eq!(r.root.totals.size, 128);
        assert_eq!(r.root.totals.files, 64);
    }
}
#[test]
fn cancellation_mid_scan_and_progress_throttling() {
    let d = tempfile::tempdir().unwrap();
    file(&d.path().join("a"), 3);
    let job = JobHandle::silent();
    let mut calls = 0;
    let r = measure_usage(d.path(), &UsageOptions::default(), &job, &mut |p| {
        calls += 1;
        assert_eq!(p.entries_seen, 1);
        job.cancel();
    })
    .unwrap();
    assert!(r.cancelled && r.incomplete);
    assert_eq!(calls, 1);
    let mut times = Vec::new();
    measure_usage(
        d.path(),
        &UsageOptions::default(),
        &JobHandle::silent(),
        &mut |_| times.push(Instant::now()),
    )
    .unwrap();
    assert!(!times.is_empty());
    assert!(times
        .windows(2)
        .all(|w| w[1].duration_since(w[0]) >= Duration::from_millis(100)));
}
#[test]
fn depth_and_identity_limits_are_explicit() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir(d.path().join("sub")).unwrap();
    file(&d.path().join("sub/a"), 4);
    let r = measure(
        d.path(),
        &UsageOptions {
            max_depth: 1,
            ..Default::default()
        },
    );
    assert!(r.incomplete);
    assert!(r.skipped.iter().any(|(_, e)| *e == ScanIssue::DepthLimit));
    #[cfg(unix)]
    {
        let r = measure(
            d.path(),
            &UsageOptions {
                max_identities: 1,
                ..Default::default()
            },
        );
        assert!(r.incomplete);
        assert!(r.skipped.iter().any(|(_, e)| *e == ScanIssue::EntryLimit));
    }
}
#[test]
fn device_boundary_comparison_and_options() {
    assert!(UsageOptions::default().one_file_system);
    assert!(same_device(Some(2), Some(2)));
    assert!(!same_device(Some(2), Some(3)));
    assert!(same_device(None, None));
    let d = tempfile::tempdir().unwrap();
    file(&d.path().join("a"), 5);
    assert_eq!(
        measure(d.path(), &UsageOptions::default()).root.totals,
        measure(
            d.path(),
            &UsageOptions {
                one_file_system: false,
                ..Default::default()
            }
        )
        .root
        .totals
    );
    // Exercise the actual boundary branch with a simulated root device; mounting is not required.
    #[cfg(unix)]
    {
        let mut callback = |_| {};
        let opts = UsageOptions::default();
        let job = JobHandle::silent();
        let mut w = Walker {
            opts: &opts,
            job: &job,
            progress: &mut callback,
            seen: HashSet::new(),
            device: Some(
                scan_fs::metadata(d.path())
                    .unwrap()
                    .identity
                    .unwrap()
                    .0
                    .wrapping_add(1),
            ),
            skipped: vec![],
            skipped_count: 0,
            stopped: false,
            cancelled: false,
            state: UsageProgress::default(),
            last: None,
        };
        assert_eq!(w.visit(d.path().to_path_buf(), 0, 10).totals.size, 0);
        assert_eq!(w.skipped[0].1, ScanIssue::FileSystemBoundary);
    }
}
#[cfg(unix)]
#[test]
fn unreadable_directory_is_reported_except_as_root() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let sub = d.path().join("locked");
    fs::create_dir(&sub).unwrap();
    file(&sub.join("file"), 9);
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o0)).unwrap();
    let r = measure(d.path(), &UsageOptions::default());
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(r.incomplete);
    assert_eq!(r.root.totals.directories, 2);
    assert!(r
        .skipped
        .iter()
        .any(|(_, e)| *e == ScanIssue::Io(io::ErrorKind::PermissionDenied)));
}
#[test]
fn errors_are_capped_and_invalid_budget_rejected() {
    let mut errors = vec![];
    let mut count = 0;
    for _ in 0..250 {
        issue(
            &mut errors,
            &mut count,
            Path::new("x"),
            ScanIssue::DepthLimit,
        );
    }
    assert_eq!(errors.len(), 200);
    assert_eq!(count, 250);
    let d = tempfile::tempdir().unwrap();
    assert!(measure_usage(
        d.path(),
        &UsageOptions {
            max_entries: 0,
            ..Default::default()
        },
        &JobHandle::silent(),
        &mut |_| {}
    )
    .is_err());
}

#[cfg(unix)]
#[test]
fn sparse_file_reports_logical_and_allocated_sizes_separately() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("sparse");
    fs::File::create(&path).unwrap().set_len(8 << 20).unwrap();
    let r = measure(d.path(), &UsageOptions::default());
    assert_eq!(r.root.totals.size, 8 << 20);
    assert_eq!(
        r.root.totals.allocated,
        scan_fs::metadata(&path).unwrap().allocated
    );
}

#[cfg(unix)]
#[test]
fn root_swap_after_resolution_cannot_follow_a_symlink() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    file(&outside.path().join("huge"), 100000);
    let root = d.path().join("root");
    fs::create_dir(&root).unwrap();
    let mut swapped = false;
    let r = measure_usage(
        &root,
        &UsageOptions::default(),
        &JobHandle::silent(),
        &mut |_| {
            if !swapped {
                swapped = true;
                fs::rename(&root, d.path().join("old")).unwrap();
                symlink(outside.path(), &root).unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(r.root.totals.size, 0);
    assert!(r.incomplete);
}
