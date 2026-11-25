//! Load measurement for issue #28 (large folders). Rust part only, no UI.
//!
//! Ignored by default (creates 50,000 entries, takes seconds). Run with:
//!
//! ```text
//! cargo test -p dualfile-core --test perf_listing --release -- --ignored --nocapture --test-threads=1
//! ```
//!
//! What is measured: `LocalVfs::list` (the call behind the `list` command), the owner-name
//! resolution and the JSON serialisation that `state.list` + Tauri IPC add (a local mirror of
//! `EntryDto`, since the real one lives in the Tauri crate), and the resident-memory growth.
//! Sorting and filtering are NOT done in Rust (they run in the web view, `src/state/sort.ts`
//! and `src/state/filter.ts`), so they are not measured here.
//!
//! The wall-clock numbers are printed for humans. The only asserts are count and shape
//! checks (entry count, linear growth within a generous factor), so the test cannot flake on
//! a slow runner.

use dualfile_core::vfs::{local_owner_name, Entry, EntryKind, LocalVfs, VPath, Vfs};
use serde::Serialize;
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const RUNS: usize = 5;
const TOTAL: usize = 50_000;

/// Mirror of `EntryDto` in `src-tauri/src/convert.rs` (same fields, same camelCase).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dto {
    name: String,
    kind: &'static str,
    size: u64,
    modified: Option<i64>,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    owner_name: Option<String>,
    group_name: Option<String>,
    permissions: Option<String>,
    hidden: bool,
    link_target: Option<String>,
    broken_link: bool,
    error: Option<String>,
}

fn dto(entry: &Entry) -> Dto {
    Dto {
        name: entry.name.clone(),
        kind: match entry.kind {
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
            EntryKind::Symlink => "symlink",
        },
        size: entry.size,
        modified: entry.modified.map(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        }),
        mode: entry.mode,
        uid: entry.uid,
        gid: entry.gid,
        owner_name: entry.uid.and_then(|id| local_owner_name(id, false)),
        group_name: entry.gid.and_then(|id| local_owner_name(id, true)),
        permissions: entry.mode.map(dualfile_core::vfs::format_permissions),
        hidden: entry.hidden,
        link_target: entry.link_target.clone(),
        broken_link: entry.broken_link,
        error: entry.error.clone(),
    }
}

fn file_name(i: usize) -> String {
    match i % 8 {
        0 => format!("IMG_{i:05}.jpg"),
        1 => format!("Bericht_2026-{:02}_{i}.pdf", i % 12 + 1),
        2 => format!("Müller_Rechnung_{i}.docx"),
        3 => format!("日本語_{i}.txt"),
        4 => format!("résumé_{i}.md"),
        5 => format!("Datei {i} (Kopie).txt"),
        6 => format!("scan-{i}.PNG"),
        _ => format!(".verborgen_{i}"),
    }
}

fn dir_name(i: usize) -> String {
    match i % 4 {
        0 => format!("Projekt_{i}"),
        1 => format!("Übersicht {i}"),
        2 => format!("ordner-{i}"),
        _ => format!("Ärger_{i}"),
    }
}

/// `dirs` empty sub folders and `files` files with varying size (sparse) and mtime.
fn populate(root: &Path, files: usize, dirs: usize) {
    for i in 0..files {
        let f = File::create(root.join(file_name(i))).unwrap();
        f.set_len(((i * 7919) % 100_000) as u64).unwrap();
        let t = SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_500_000_000 + (i as u64 * 104_729) % 300_000_000);
        f.set_modified(t).unwrap();
    }
    for i in 0..dirs {
        fs::create_dir(root.join(dir_name(i))).unwrap();
    }
}

fn rss_kib() -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// CPU time of the calling thread in ms. Less sensitive to a busy machine than wall-clock time.
fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 * 1000.0 + ts.tv_nsec as f64 / 1e6
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn stats(label: &str, mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    println!(
        "  {label:<34} median {med:8.1} ms   min {:8.1}   max {:8.1}   (n={})",
        v[0],
        v[v.len() - 1],
        v.len()
    );
    med
}

fn measure(label: &str, root: &Path, expect: usize) -> f64 {
    let vfs = LocalVfs;
    let path = VPath::local(root).unwrap();
    let mut t_list = Vec::new();
    let mut c_list = Vec::new();
    let mut t_dto = Vec::new();
    let mut t_json = Vec::new();
    let mut json_bytes = 0;
    let mut rss_rows = Vec::new();
    for run in 0..RUNS {
        let before = rss_kib();
        let t = Instant::now();
        let c = thread_cpu_ms();
        let entries = vfs.list(&path).unwrap();
        c_list.push(thread_cpu_ms() - c);
        t_list.push(ms(t.elapsed()));
        assert_eq!(entries.len(), expect);
        let after_list = rss_kib();
        let t = Instant::now();
        let dtos: Vec<Dto> = entries.iter().map(dto).collect();
        t_dto.push(ms(t.elapsed()));
        let t = Instant::now();
        let json = serde_json::to_vec(&dtos).unwrap();
        t_json.push(ms(t.elapsed()));
        json_bytes = json.len();
        let after_all = rss_kib();
        rss_rows.push((run, before, after_list, after_all));
        drop((entries, dtos, json));
    }
    println!("\n[{label}] {expect} entries");
    let med = stats("list (LocalVfs::list)", t_list);
    stats("list, thread CPU time", c_list);
    stats("DTO + owner names (state.list)", t_dto);
    stats("JSON serialise (IPC proxy)", t_json);
    println!(
        "  JSON payload: {:.1} MiB ({} bytes)",
        json_bytes as f64 / 1048576.0,
        json_bytes
    );
    for (run, b, l, a) in rss_rows {
        println!(
            "  run {}: RSS before {} MiB, +list {:+} MiB, +DTO+JSON {:+} MiB",
            run + 1,
            b / 1024,
            (l as i64 - b as i64) / 1024,
            (a as i64 - l as i64) / 1024
        );
    }
    med
}

#[test]
#[ignore = "load measurement (issue #28); run with --ignored --nocapture"]
fn listing_50k_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let t = Instant::now();
    populate(dir.path(), TOTAL * 6 / 10, TOTAL * 4 / 10);
    println!(
        "\nsetup: {} files + {} dirs in {:.1} s",
        TOTAL * 6 / 10,
        TOTAL * 4 / 10,
        t.elapsed().as_secs_f64()
    );
    measure("50k: 60 % files, 40 % empty sub folders", dir.path(), TOTAL);
}

#[test]
#[ignore = "load measurement (issue #28); run with --ignored --nocapture"]
fn listing_50k_files_only() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), TOTAL, 0);
    measure("50k: files only", dir.path(), TOTAL);
}

#[test]
#[ignore = "load measurement (issue #28); run with --ignored --nocapture"]
fn listing_scales_linearly() {
    // Complexity check instead of a millisecond threshold: 10x the entries must not cost
    // more than ~30x the time (quadratic behaviour would give ~100x).
    let mut medians = Vec::new();
    for n in [5_000usize, 50_000] {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), n * 6 / 10, n * 4 / 10);
        medians.push(measure(&format!("scaling n={n}"), dir.path(), n));
    }
    let ratio = medians[1] / medians[0].max(0.001);
    println!("\nscaling 5k -> 50k: factor {ratio:.1} (linear = 10)");
    assert!(
        ratio < 30.0,
        "listing does not scale linearly: factor {ratio}"
    );
}
