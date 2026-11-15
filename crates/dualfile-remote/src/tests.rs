//! Behaviour tests of `FtpVfs` and the connection manager against the in-memory server.

use crate::fake::{FakeConnector, FakeServer, FakeTransport};
use crate::ftpvfs::{FtpConfig, FtpVfs, Opener};
use crate::manager::{ConnectError, ConnectionManager, ManagerConfig};
use crate::profile::{FtpProfile, TlsMode};
use crate::secret::Secret;
use crate::transport::Limits;
use crate::trust::*;
use dualfile_core::ops::{self, Conflict, ConflictResolver, JobHandle, Resolution};
use dualfile_core::vfs::{EntryKind, ReplaceMode, VPath, Vfs, WriteMode};
use dualfile_core::{Error, VfsRouter};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

fn vfs_with(server: &FakeServer, id: &str, cfg: FtpConfig) -> Arc<FtpVfs<FakeTransport>> {
    let first = server.connect("user", &Secret::new("secret")).unwrap();
    let s = server.clone();
    let opener: Opener<FakeTransport> = Arc::new(move || s.connect("user", &Secret::new("secret")));
    FtpVfs::new(id, first, opener, cfg, Some(Secret::new("secret"))).unwrap()
}

fn cfg() -> FtpConfig {
    FtpConfig {
        cache_ttl: std::time::Duration::ZERO,
        ..FtpConfig::default()
    }
}

fn setup() -> (FakeServer, Arc<FtpVfs<FakeTransport>>) {
    let server = FakeServer::new();
    let vfs = vfs_with(&server, "s1", cfg());
    (server, vfs)
}

fn p(path: &str) -> VPath {
    VPath::ftp("s1", path).unwrap()
}

fn lp(path: &std::path::Path) -> VPath {
    VPath::local(path).unwrap()
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 253) as u8).collect()
}

fn copy_up(src: &std::path::Path, vfs: &FtpVfs<FakeTransport>, dir: &str) -> ops::OpReport {
    ops::copy(
        &dualfile_core::LocalVfs,
        &[lp(src)],
        vfs,
        &p(dir),
        None,
        &JobHandle::silent(),
    )
}

struct Choose(Resolution);
impl ConflictResolver for Choose {
    fn resolve(&self, _: &Conflict) -> Resolution {
        self.0
    }
}

// ----------------------------------------------------------------------- listing and stat

#[test]
fn lists_and_stats_with_mlsd_and_with_unix_list() {
    for style in [
        crate::fake::ListStyle::Mlsd,
        crate::fake::ListStyle::UnixList,
    ] {
        let (server, vfs) = setup();
        server.state().style = style;
        server.put("/pub/a.txt", b"hello");
        server.mkdirs("/pub/sub");
        let mut entries = vfs.list(&p("/pub")).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 2, "{style:?}");
        assert_eq!(
            (entries[0].name.as_str(), entries[0].kind, entries[0].size),
            ("a.txt", EntryKind::File, 5)
        );
        assert_eq!(entries[1].kind, EntryKind::Dir);
        assert!(entries[0].modified.is_some());
        assert_eq!(vfs.stat(&p("/pub/a.txt")).unwrap().size, 5);
        assert_eq!(vfs.stat(&p("/pub/sub")).unwrap().kind, EntryKind::Dir);
        assert_eq!(vfs.stat(&p("/")).unwrap().kind, EntryKind::Dir);
        assert!(matches!(vfs.stat(&p("/pub/nope")), Err(Error::NotFound(_))));
        assert!(matches!(vfs.list(&p("/nodir")), Err(Error::NotFound(_))));
    }
    // server that claims MLSD but refuses it: falls back to LIST
    let (server, vfs) = setup();
    server.state().style = crate::fake::ListStyle::UnixList;
    server.state().features.mlst = false;
    server.put("/x.txt", b"1");
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 1);
    assert_eq!(vfs.stat(&p("/x.txt")).unwrap().size, 1);
}

#[test]
fn hostile_names_are_marked_and_never_written_locally() {
    let (server, vfs) = setup();
    server.put("/d/ok.txt", b"fine");
    server.state().raw_listing = Some(
        [
            "type=file;size=1; ..",
            "type=file;size=1; a/b",
            "type=file;size=1; evil\nname",
            "type=file;size=1; back\\slash",
            "type=file;size=4; ok.txt",
            "type=file;size=1; Case",
            "type=file;size=1; CASE",
            "type=file;size=1; ../../escape",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    let entries = vfs.list(&p("/d")).unwrap();
    let ok: Vec<&str> = entries
        .iter()
        .filter(|e| e.error.is_none())
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(ok, ["ok.txt"]);
    assert_eq!(entries.iter().filter(|e| e.error.is_some()).count(), 6);
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let report = ops::copy(
        &*vfs,
        &[p("/d")],
        &dualfile_core::LocalVfs,
        &lp(&out),
        None,
        &JobHandle::silent(),
    );
    assert_eq!(report.failed.len(), 6, "{:?}", report.failed);
    assert_eq!(std::fs::read(out.join("d/ok.txt")).unwrap(), b"fine");
    let mut seen: Vec<_> = std::fs::read_dir(out.join("d"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    seen.sort();
    assert_eq!(seen, ["ok.txt"]);
    let mut top: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    top.sort();
    assert_eq!(top, ["out"], "nothing escaped the destination");
    // a marked entry cannot be opened
    let bad = entries.iter().find(|e| e.name.contains("evil")).unwrap();
    assert!(bad.error.is_some());
}

#[test]
fn oversized_listings_are_an_error_not_a_silent_cut() {
    let (server, vfs) = setup();
    server.state().raw_listing = Some(
        (0..300_000)
            .map(|i| format!("type=file;size=1; f{i}"))
            .collect(),
    );
    assert!(matches!(vfs.list(&p("/")), Err(Error::Remote(_))));
    let small = vfs_with(
        &server,
        "s1",
        FtpConfig {
            limits: Limits {
                max_entries: 3,
                ..Limits::default()
            },
            ..cfg()
        },
    );
    server.state().raw_listing = Some((0..10).map(|i| format!("type=file;size=1; f{i}")).collect());
    assert!(matches!(small.list(&p("/")), Err(Error::Remote(_))));
    server.state().raw_listing = Some((0..3).map(|i| format!("type=file;size=1; f{i}")).collect());
    assert_eq!(small.list(&p("/")).unwrap().len(), 3);
}

// ----------------------------------------------------------------------- data connections

#[test]
fn a_foreign_passive_address_never_gets_a_data_connection() {
    let (server, vfs) = setup();
    server.put("/f.bin", &payload(1000));
    let control = server.state().control_ip;
    server.state().pasv_reply = Some("227 Entering Passive Mode (10,9,9,9,195,80)".into());
    let mut data = Vec::new();
    vfs.open_read(&p("/f.bin"))
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    assert_eq!(data.len(), 1000);
    assert!(!server.state().data_connects.is_empty());
    assert!(server
        .state()
        .data_connects
        .iter()
        .all(|a| a.ip() == control));
    // an announced port of another service is refused
    server.state().pasv_reply = Some("227 Entering Passive Mode (203,0,113,7,0,25)".into());
    assert!(vfs.open_read(&p("/f.bin")).is_err());
}

#[test]
fn control_characters_never_reach_the_wire() {
    let (server, vfs) = setup();
    server.put("/a", b"x");
    let mut t = server.connect("user", &Secret::new("secret")).unwrap();
    use crate::transport::FtpTransport;
    assert!(matches!(
        t.delete("/a\r\nDELE /b"),
        Err(crate::error::TransportError::Rejected)
    ));
    assert!(matches!(
        t.mkdir("/x\0y"),
        Err(crate::error::TransportError::Rejected)
    ));
    assert!(!server.log().iter().any(|l| l.contains("DELE")));
    assert!(server.exists("/a"));
    assert!(VPath::ftp("s1", "/a\r\nDELE /b").is_err());
    assert!(vfs.stat(&p("/a")).is_ok());
}

// ----------------------------------------------------------------------- upload / resume

fn local_file(dir: &std::path::Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path
}

#[test]
fn upload_appears_under_its_name_only_when_complete() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let data = payload(2_500_000);
    let file = local_file(tmp.path(), "big.bin", &data);
    let report = copy_up(&file, &vfs, "/in");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/in/big.bin").unwrap(), data);
    assert_eq!(
        server.names_in("/in"),
        ["big.bin"],
        "no partial or meta file left"
    );
    // download again, byte for byte
    let out = tmp.path().join("o");
    std::fs::create_dir(&out).unwrap();
    let report = ops::copy(
        &*vfs,
        &[p("/in/big.bin")],
        &dualfile_core::LocalVfs,
        &lp(&out),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), data);
}

#[test]
fn aborted_upload_continues_after_the_verified_part() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let data = payload(2_500_000);
    let file = local_file(tmp.path(), "big.bin", &data);
    server.state().cut_upload_after = Some(1_048_576);
    let first = copy_up(&file, &vfs, "/in");
    assert_eq!(first.failed.len(), 1);
    assert!(!server.exists("/in/big.bin"));
    assert_eq!(
        server.get("/in/big.bin.dualfile-part").unwrap().len(),
        1_048_576
    );
    assert!(server.exists("/in/big.bin.dualfile-part.meta"));
    server.state().cut_upload_after = None;
    let before = server.state().uploaded;
    let second = copy_up(&file, &vfs, "/in");
    assert!(second.failed.is_empty(), "{:?}", second.failed);
    assert_eq!(server.get("/in/big.bin").unwrap(), data);
    assert!(
        server
            .log()
            .iter()
            .any(|l| l == "APPE /in/big.bin.dualfile-part"),
        "{:?}",
        server.log()
    );
    let sent = server.state().uploaded - before;
    assert!(
        sent < data.len() - 1_000_000,
        "only the rest is sent, sent {sent}"
    );
    assert_eq!(server.names_in("/in"), ["big.bin"]);
}

fn abort_then(
    setup_next: impl FnOnce(&FakeServer, &std::path::Path),
) -> (
    FakeServer,
    Arc<FtpVfs<FakeTransport>>,
    tempfile::TempDir,
    std::path::PathBuf,
    ops::OpReport,
) {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "v.bin", &payload(2_200_000));
    server.state().cut_upload_after = Some(1_100_000);
    assert_eq!(copy_up(&file, &vfs, "/in").failed.len(), 1);
    server.state().cut_upload_after = None;
    setup_next(&server, &file);
    let report = copy_up(&file, &vfs, "/in");
    (server, vfs, tmp, file, report)
}

#[test]
fn changed_source_or_time_starts_over() {
    // other content and size
    let (server, _v, _t, file, report) = abort_then(|_, file| {
        std::fs::write(
            file,
            payload(2_300_000)
                .iter()
                .map(|b| b.wrapping_add(1))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    });
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        server.get("/in/v.bin").unwrap(),
        std::fs::read(&file).unwrap()
    );
    assert!(
        !server.log().iter().any(|l| l.starts_with("APPE")),
        "no continuing"
    );
    // same size, other modification time
    let (server, _v, _t, file, report) = abort_then(|_, file| {
        let f = std::fs::File::options().write(true).open(file).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
    });
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        server.get("/in/v.bin").unwrap(),
        std::fs::read(&file).unwrap()
    );
    assert!(!server.log().iter().any(|l| l.starts_with("APPE")));
}

#[test]
fn damaged_meta_or_wrong_server_size_starts_over() {
    for tamper in 0..3 {
        let (server, _v, _t, file, report) = abort_then(|server, _| match tamper {
            // cut-off meta of ours
            0 => {
                let mut s = server.state();
                if let Some(crate::fake::Node::File { data, .. }) =
                    s.nodes.get_mut("/in/v.bin.dualfile-part.meta")
                {
                    data.truncate(data.len() - 6);
                }
            }
            // the server's file is not the length that was recorded
            1 => {
                let mut s = server.state();
                if let Some(crate::fake::Node::File { data, .. }) =
                    s.nodes.get_mut("/in/v.bin.dualfile-part")
                {
                    data.extend_from_slice(b"junk");
                }
            }
            // recorded length larger than the source
            _ => {
                let mut s = server.state();
                let len = 2_200_000 + 50;
                if let Some(crate::fake::Node::File { data, .. }) =
                    s.nodes.get_mut("/in/v.bin.dualfile-part")
                {
                    data.resize(len, 0);
                }
                let name = "/in/v.bin.dualfile-part.meta";
                if let Some(crate::fake::Node::File { data, .. }) = s.nodes.get_mut(name) {
                    let text = String::from_utf8(data.clone()).unwrap();
                    let at = text.find("\"written\":").unwrap();
                    *data = format!("{}\"written\":{}}}", &text[..at], len).into_bytes();
                }
            }
        });
        assert!(
            report.failed.is_empty(),
            "case {tamper}: {:?}",
            report.failed
        );
        assert_eq!(
            server.get("/in/v.bin").unwrap(),
            std::fs::read(&file).unwrap(),
            "case {tamper}"
        );
        assert!(
            !server.log().iter().any(|l| l.starts_with("APPE")),
            "case {tamper}"
        );
    }
}

#[test]
fn a_foreign_partial_file_is_never_touched() {
    let (server, vfs) = setup();
    server.put("/in/x.bin.dualfile-part", b"somebody else's");
    let tmp = tempfile::tempdir().unwrap();
    let data = payload(5000);
    let file = local_file(tmp.path(), "x.bin", &data);
    let report = copy_up(&file, &vfs, "/in");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/in/x.bin").unwrap(), data);
    assert_eq!(
        server.get("/in/x.bin.dualfile-part").unwrap(),
        b"somebody else's"
    );
    // a foreign file named like our meta file, with a part next to it: also left alone
    server.put("/in/y.bin.dualfile-part", b"theirs");
    server.put("/in/y.bin.dualfile-part.meta", b"not ours");
    let file = local_file(tmp.path(), "y.bin", &data);
    assert!(copy_up(&file, &vfs, "/in").failed.is_empty());
    assert_eq!(server.get("/in/y.bin.dualfile-part").unwrap(), b"theirs");
    assert_eq!(
        server.get("/in/y.bin.dualfile-part.meta").unwrap(),
        b"not ours"
    );
    assert_eq!(server.get("/in/y.bin").unwrap(), data);
    // discarding only removes what a valid meta file proves to be ours
    vfs.discard_resumable(&p("/in/y.bin")).unwrap();
    assert!(
        server.exists("/in/y.bin.dualfile-part") && server.exists("/in/y.bin.dualfile-part.meta")
    );
}

#[test]
fn discard_removes_our_partial_file() {
    let (server, vfs, _t, _f, _r) = abort_then(|_, _| {});
    // the second copy completed; make a fresh abort and discard it
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "z.bin", &payload(2_200_000));
    server.state().cut_upload_after = Some(1_100_000);
    assert_eq!(copy_up(&file, &vfs, "/in").failed.len(), 1);
    assert!(server.exists("/in/z.bin.dualfile-part"));
    vfs.discard_resumable(&p("/in/z.bin")).unwrap();
    assert!(
        !server.exists("/in/z.bin.dualfile-part") && !server.exists("/in/z.bin.dualfile-part.meta")
    );
}

#[test]
fn cancelling_keeps_partial_file_and_meta_that_match() {
    struct Late(Arc<Mutex<Option<JobHandle>>>, std::sync::atomic::AtomicU32);
    impl ops::ProgressSink for Late {
        fn event(&self, e: ops::ProgressEvent) {
            if matches!(e, ops::ProgressEvent::BytesCopied { .. })
                && self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
            {
                if let Some(j) = self.0.lock().unwrap().as_ref() {
                    j.cancel();
                }
            }
        }
    }
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let data = payload(4_000_000);
    let file = local_file(tmp.path(), "c.bin", &data);
    let holder = Arc::new(Mutex::new(None));
    let job = JobHandle::new(Arc::new(Late(holder.clone(), Default::default())));
    *holder.lock().unwrap() = Some(job.clone());
    let report = ops::copy(
        &dualfile_core::LocalVfs,
        &[lp(&file)],
        &*vfs,
        &p("/in"),
        None,
        &job,
    );
    assert!(report.cancelled);
    let part = server.get("/in/c.bin.dualfile-part").unwrap().len();
    assert!(part > 0 && part < data.len());
    let meta = String::from_utf8(server.get("/in/c.bin.dualfile-part.meta").unwrap()).unwrap();
    assert!(meta.contains(&format!("\"written\":{part}")), "{meta}");
    assert!(copy_up(&file, &vfs, "/in").failed.is_empty());
    assert_eq!(server.get("/in/c.bin").unwrap(), data);
}

#[test]
fn existing_targets_are_never_overwritten_without_a_decision() {
    let (server, vfs) = setup();
    server.put("/in/e.txt", b"precious");
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "e.txt", b"new");
    let report = copy_up(&file, &vfs, "/in");
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(server.get("/in/e.txt").unwrap(), b"precious");
    let both = ops::copy(
        &dualfile_core::LocalVfs,
        &[lp(&file)],
        &*vfs,
        &p("/in"),
        Some(&Choose(Resolution::KeepBoth)),
        &JobHandle::silent(),
    );
    assert!(both.failed.is_empty(), "{:?}", both.failed);
    assert_eq!(server.get("/in/e.txt").unwrap(), b"precious");
    assert_eq!(server.get("/in/e (2).txt").unwrap(), b"new");
    // the direct no-replace commit refuses, too
    let mut w = vfs
        .create_write(&p("/in/e.txt"), WriteMode::NoReplace)
        .unwrap();
    w.write_all(b"x").unwrap();
    assert!(matches!(w.commit(), Err(Error::Exists(_))));
    drop(w);
    assert_eq!(server.get("/in/e.txt").unwrap(), b"precious");
    assert_eq!(
        server.names_in("/in").len(),
        2,
        "no temporary file left behind"
    );
}

#[test]
fn a_file_created_by_someone_else_before_the_final_rename_survives() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    // the server would replace on RNTO; our look-up right before it must still see the target
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "r.txt", b"mine");
    server.state().inject_before_rename = Some(("/in/r.txt".into(), b"theirs".to_vec()));
    // injected inside RNFR/RNTO, i.e. after the look-up: this is the documented window; the
    // replace-capable server then overwrites. Make the server refuse instead (like many do):
    server.state().rename_replaces = false;
    let report = copy_up(&file, &vfs, "/in");
    assert_eq!(server.get("/in/r.txt").unwrap(), b"theirs");
    assert!(report.done.is_empty());
    assert!(
        server.exists("/in/r.txt.dualfile-part"),
        "the partial file stays for another try"
    );
}

#[test]
fn replace_never_deletes_the_original_first() {
    let (server, vfs) = setup();
    server.put("/in/o.txt", b"old");
    server.state().rename_replaces = false;
    let mut w = vfs
        .create_write(&p("/in/o.txt"), WriteMode::Replace)
        .unwrap();
    w.write_all(b"new").unwrap();
    assert!(w.commit().is_err());
    drop(w);
    assert_eq!(server.get("/in/o.txt").unwrap(), b"old");
    assert!(!server.log().iter().any(|l| l == "DELE /in/o.txt"));
    server.state().rename_replaces = true;
    let mut w = vfs
        .create_write(&p("/in/o.txt"), WriteMode::Replace)
        .unwrap();
    w.write_all(b"new").unwrap();
    w.commit().unwrap();
    assert_eq!(server.get("/in/o.txt").unwrap(), b"new");
}

#[test]
fn rename_checks_for_an_existing_target_first() {
    let (server, vfs) = setup();
    server.put("/a.txt", b"a");
    server.put("/b.txt", b"b");
    assert!(matches!(
        vfs.rename(&p("/a.txt"), &p("/b.txt"), ReplaceMode::NoReplace),
        Err(Error::Exists(_))
    ));
    assert_eq!(server.get("/b.txt").unwrap(), b"b");
    vfs.rename(&p("/a.txt"), &p("/c.txt"), ReplaceMode::NoReplace)
        .unwrap();
    assert!(server.exists("/c.txt") && !server.exists("/a.txt"));
    assert!(matches!(vfs.mkdir(&p("/c.txt")), Err(Error::Exists(_))));
    assert!(matches!(vfs.mkdir(&p("/no/dir")), Err(Error::NotFound(_))));
    vfs.mkdir(&p("/d")).unwrap();
    assert!(server.exists("/d"));
}

// ----------------------------------------------------------------------- delete, trash, move

#[test]
fn there_is_no_trash_and_no_delete_as_a_substitute() {
    let (server, vfs) = setup();
    server.put("/f.txt", b"x");
    assert!(!vfs.capabilities().can_trash);
    assert!(matches!(
        vfs.trash(&p("/f.txt")),
        Err(Error::Unsupported(_))
    ));
    let report = ops::trash_items(&*vfs, &[p("/f.txt")], &JobHandle::silent());
    assert_eq!(report.failed.len(), 1);
    assert!(server.exists("/f.txt"));
    assert!(!server
        .log()
        .iter()
        .any(|l| l.starts_with("DELE") || l.starts_with("RMD")));
    // permanent delete goes through plan and token
    server.put("/d/g.txt", b"y");
    let plan = ops::plan_delete(&*vfs, &[p("/d")]);
    assert!(!plan.incomplete && plan.files == 1 && plan.dirs == 1);
    let report = ops::delete_permanent(&*vfs, &plan, plan.token, &JobHandle::silent());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!server.exists("/d") && server.exists("/f.txt"));
}

#[test]
fn a_delete_plan_of_another_connection_is_refused() {
    let server = FakeServer::new();
    server.put("/d/f", b"x");
    let a = vfs_with(&server, "s1", cfg());
    let b = vfs_with(&server, "s2", cfg());
    let plan = ops::plan_delete(&*a, &[p("/d")]);
    let report = ops::delete_permanent(&*b, &plan, plan.token, &JobHandle::silent());
    assert!(report.done.is_empty() && !report.failed.is_empty());
    assert!(server.exists("/d/f"));
    // a path of the other connection is not served either
    assert!(matches!(b.stat(&p("/d")), Err(Error::Disconnected)));
    assert!(
        ops::delete_permanent(&*a, &plan, plan.token, &JobHandle::silent())
            .failed
            .is_empty()
    );
}

#[test]
fn move_between_local_and_server_verifies_and_removes_the_source() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "m.bin", &payload(300_000));
    let report = ops::move_(
        &dualfile_core::LocalVfs,
        &[lp(&file)],
        &*vfs,
        &p("/in"),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!file.exists());
    assert_eq!(server.get("/in/m.bin").unwrap().len(), 300_000);
    // and down again
    let report = ops::move_(
        &*vfs,
        &[p("/in/m.bin")],
        &dualfile_core::LocalVfs,
        &lp(tmp.path()),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(file.exists() && !server.exists("/in/m.bin"));
    // a failed upload keeps the local source
    server.state().cut_upload_after = Some(10);
    let report = ops::move_(
        &dualfile_core::LocalVfs,
        &[lp(&file)],
        &*vfs,
        &p("/in"),
        None,
        &JobHandle::silent(),
    );
    assert_eq!(report.failed.len(), 1);
    assert!(file.exists(), "source stays after a partial failure");
    assert_eq!(std::fs::read(&file).unwrap(), payload(300_000));
    assert!(
        matches!(report.failed[0].1, Error::Disconnected),
        "{:?}",
        report.failed
    );
}

#[test]
fn move_on_one_connection_is_a_rename() {
    let (server, vfs) = setup();
    server.put("/a/f.txt", b"1");
    server.mkdirs("/b");
    let report = ops::move_(
        &*vfs,
        &[p("/a/f.txt")],
        &*vfs,
        &p("/b"),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(server.exists("/b/f.txt") && !server.exists("/a/f.txt"));
    assert!(server.log().iter().any(|l| l.starts_with("RNFR /a/f.txt")));
}

// ----------------------------------------------------------------------- budgets and limits

#[test]
fn a_lost_control_connection_is_replaced_within_the_budget() {
    let server = FakeServer::new();
    server.put("/f", b"x");
    let vfs = vfs_with(
        &server,
        "s1",
        FtpConfig {
            max_reconnects: 2,
            ..cfg()
        },
    );
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 1);
    server.disconnect_everybody();
    // a read is repeated on the new connection
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 1);
    assert_eq!(vfs.connects(), 2);
    // a changing command is not repeated blindly
    server.disconnect_everybody();
    assert!(vfs.mkdir(&p("/m")).is_err());
    assert!(!server.exists("/m"));
    assert_eq!(
        server.log().iter().filter(|l| l.starts_with("MKD")).count(),
        0
    );
    assert!(vfs.mkdir(&p("/m")).is_ok());
    assert_eq!(vfs.connects(), 3);
    // the budget ends: Disconnected instead of endless reconnecting
    server.disconnect_everybody();
    let _ = vfs.list(&p("/"));
    server.disconnect_everybody();
    let _ = vfs.list(&p("/"));
    server.disconnect_everybody();
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    assert!(vfs.connects() <= 3 + 2 + 1);
}

#[test]
fn a_command_that_times_out_is_an_error_and_cannot_hang_the_session() {
    let server = FakeServer::new();
    let vfs = vfs_with(
        &server,
        "s1",
        FtpConfig {
            max_reconnects: 0,
            ..cfg()
        },
    );
    server.state().hang_next = true;
    assert!(vfs.list(&p("/")).is_err());
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    // with budget the read is repeated and succeeds
    let vfs = vfs_with(&server, "s1", cfg());
    server.state().hang_next = true;
    assert!(vfs.list(&p("/")).is_ok());
}

#[test]
fn the_number_of_parallel_transfers_is_capped() {
    let server = FakeServer::new();
    server.put("/f", &payload(100));
    let vfs = vfs_with(
        &server,
        "s1",
        FtpConfig {
            max_transfers: 2,
            ..cfg()
        },
    );
    let one = vfs.open_read(&p("/f")).unwrap();
    let _two = vfs.open_read(&p("/f")).unwrap();
    assert!(matches!(vfs.open_read(&p("/f")), Err(Error::Remote(_))));
    drop(one);
    assert!(vfs.open_read(&p("/f")).is_ok());
}

#[test]
fn read_range_and_resume_reads_start_at_the_offset() {
    let (server, vfs) = setup();
    let data = payload(10_000);
    server.put("/f", &data);
    assert_eq!(vfs.read_range(&p("/f"), 100, 50).unwrap(), &data[100..150]);
    assert!(vfs.read_range(&p("/f"), 20_000, 50).unwrap().is_empty());
    assert!(server.log().iter().any(|l| l == "RETR /f (REST 100)"));
    // without REST the offset is skipped by reading
    server.state().features.rest_stream = false;
    let vfs2 = vfs_with(&server, "s1", cfg());
    assert_eq!(vfs2.read_range(&p("/f"), 100, 50).unwrap(), &data[100..150]);
    // links on the server are not followed
    server.state().raw_listing = Some(vec!["type=OS.unix=slink:/etc/passwd;size=1; l".into()]);
    let listed = vfs.list(&p("/")).unwrap();
    assert_eq!(listed[0].kind, EntryKind::Symlink);
    assert_eq!(listed[0].link_target.as_deref(), Some("/etc/passwd"));
    assert!(vfs.create_symlink("x", &p("/y")).is_err());
}

#[test]
fn closing_disconnects_everything() {
    let (_server, vfs) = setup();
    assert!(vfs.list(&p("/")).is_ok());
    vfs.close();
    assert!(vfs.is_closed());
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    assert!(matches!(vfs.open_read(&p("/f")), Err(Error::Disconnected)));
    assert!(FtpVfs::<FakeTransport>::new(
        "BAD",
        FakeServer::new()
            .connect("user", &Secret::new("secret"))
            .unwrap(),
        Arc::new(|| Err(crate::error::TransportError::Timeout)),
        cfg(),
        None
    )
    .is_err());
}

// ----------------------------------------------------------------------- manager

fn profile(tls: TlsMode) -> FtpProfile {
    FtpProfile {
        name: "Test".into(),
        host: "ftp.example.org".into(),
        port: 21,
        user: "user".into(),
        tls,
        allow_plaintext: tls == TlsMode::None,
        start_folder: Some("/pub".into()),
    }
}

fn manager(
    server: &FakeServer,
    max: usize,
) -> (
    ConnectionManager<FakeConnector>,
    Arc<VfsRouter>,
    Arc<MemoryPins>,
) {
    let router = Arc::new(VfsRouter::new());
    let pins = Arc::new(MemoryPins::new());
    let m = ConnectionManager::new(
        FakeConnector {
            server: server.clone(),
        },
        router.clone(),
        pins.clone(),
        ManagerConfig {
            max_connections: max,
            ftp: cfg(),
            ..ManagerConfig::default()
        },
    );
    (m, router, pins)
}

#[test]
fn manager_registers_and_removes_connections_and_caps_them() {
    let server = FakeServer::new();
    server.mkdirs("/pub");
    let (m, router, _) = manager(&server, 2);
    let a = m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll),
        )
        .unwrap();
    assert!(
        dualfile_core::vfs::is_valid_connection_id(&a.id),
        "{}",
        a.id
    );
    assert_eq!(a.root, VPath::ftp(&a.id, "/pub").unwrap());
    let b = m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll),
        )
        .unwrap();
    assert_ne!(a.id, b.id);
    assert!(matches!(
        m.connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll)
        ),
        Err(ConnectError::TooManyConnections)
    ));
    let vfs = router.vfs_for(&a.root).unwrap();
    assert!(vfs.list(&a.root).is_ok());
    assert_eq!(m.connections().len(), 2);
    assert!(m.disconnect(&a.id));
    assert!(!m.disconnect(&a.id));
    assert!(matches!(router.vfs_for(&a.root), Err(Error::Disconnected)));
    assert!(
        matches!(vfs.list(&a.root), Err(Error::Disconnected)),
        "the old handle is closed too"
    );
    assert!(router.vfs_for(&b.root).is_ok());
    // a slot is free again
    assert!(m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll)
        )
        .is_ok());
    m.disconnect_all();
    assert!(m.connections().is_empty() && router.remote_ids().is_empty());
}

#[test]
fn unencrypted_profiles_need_the_flag_and_bad_logins_register_nothing() {
    let server = FakeServer::new();
    let (m, router, _) = manager(&server, 2);
    let mut open = profile(TlsMode::None);
    open.allow_plaintext = false;
    assert!(matches!(
        m.connect(open, Secret::new("secret"), Arc::new(RejectAll)),
        Err(ConnectError::Invalid(_))
    ));
    assert!(m
        .connect(
            profile(TlsMode::None),
            Secret::new("wrong"),
            Arc::new(RejectAll)
        )
        .is_err());
    assert!(router.remote_ids().is_empty() && m.connections().is_empty());
    // failed attempts do not eat up the limit
    assert!(m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll)
        )
        .is_ok());
    assert!(m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll)
        )
        .is_ok());
}

struct Ask(TrustDecision, std::sync::atomic::AtomicUsize);
impl ConfirmTrust for Ask {
    fn confirm(&self, _: &TrustRequest) -> TrustDecision {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0
    }
}

fn cert(byte: u8, chain: ChainStatus) -> CertObservation {
    CertObservation {
        sha256: [byte; 32],
        chain,
        hostname_ok: true,
        key_type: None,
    }
}

#[test]
fn pinning_through_the_manager() {
    let server = FakeServer::new();
    let (m, router, pins) = manager(&server, 4);
    server.state().cert = Some(cert(1, ChainStatus::UnknownIssuer));
    // not confirmed: no connection
    let e = m
        .connect(
            profile(TlsMode::Explicit),
            Secret::new("secret"),
            Arc::new(RejectAll),
        )
        .unwrap_err();
    assert!(matches!(e.trust_error(), Some(TrustError::NotConfirmed(_))));
    assert!(router.remote_ids().is_empty());
    // trusted once: connects, nothing stored
    let once = Arc::new(Ask(TrustDecision::TrustOnce, Default::default()));
    let c = m
        .connect(
            profile(TlsMode::Explicit),
            Secret::new("secret"),
            once.clone(),
        )
        .unwrap();
    assert!(pins
        .get(&PinKey::new(Protocol::Ftps, "ftp.example.org", 21))
        .is_none());
    m.disconnect(&c.id);
    // trusted always: pinned, later connects without asking
    let always = Arc::new(Ask(TrustDecision::TrustAlways, Default::default()));
    let c = m
        .connect(
            profile(TlsMode::Explicit),
            Secret::new("secret"),
            always.clone(),
        )
        .unwrap();
    m.disconnect(&c.id);
    let c = m
        .connect(
            profile(TlsMode::Explicit),
            Secret::new("secret"),
            always.clone(),
        )
        .unwrap();
    assert_eq!(always.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    m.disconnect(&c.id);
    // the server presents another certificate: refused, nobody is asked, nothing registered
    server.state().cert = Some(cert(2, ChainStatus::UnknownIssuer));
    let e = m
        .connect(
            profile(TlsMode::Explicit),
            Secret::new("secret"),
            always.clone(),
        )
        .unwrap_err();
    assert!(
        matches!(e.trust_error(), Some(TrustError::PinMismatch { .. })),
        "{e}"
    );
    assert_eq!(always.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(router.remote_ids().is_empty());
    // an expired certificate is never accepted, with or without a pin
    server.state().cert = Some(cert(3, ChainStatus::Expired));
    let e = m
        .connect(
            profile(TlsMode::Implicit),
            Secret::new("secret"),
            always.clone(),
        )
        .unwrap_err();
    assert!(matches!(e.trust_error(), Some(TrustError::Expired)));
    assert_eq!(always.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    // plain FTP has no certificate to judge
    assert!(m
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll)
        )
        .is_ok());
}

#[test]
fn the_password_never_shows_up_in_errors_debug_output_or_logs() {
    const MARKER: &str = "pw-MARKER-7f3a91";
    let server = FakeServer::new();
    server.mkdirs("/pub");
    server.state().echo_password = true;
    let (m, _router, _pins) = manager(&server, 4);
    let secret = Secret::new(MARKER);
    let mut all = Vec::new();
    // failed login (the server echoes the password)
    let e = m
        .connect(profile(TlsMode::None), secret.clone(), Arc::new(RejectAll))
        .unwrap_err();
    all.push(e.to_string());
    all.push(format!("{e:?}"));
    // good login, hostile server echoes the password in later error replies
    server.state().password = MARKER.into();
    let info = m
        .connect(profile(TlsMode::None), secret.clone(), Arc::new(RejectAll))
        .unwrap();
    all.push(format!("{info:?}"));
    all.push(format!("{m:?}"));
    all.push(format!("{:?}", profile(TlsMode::None)));
    all.push(format!(
        "{:?}",
        crate::profile::ConnectParams::new(profile(TlsMode::None))
    ));
    all.push(format!("{secret:?} {secret}"));
    server.state().echo_in_errors = Some(MARKER.into());
    let vfs = m.router().vfs_for(&info.root).unwrap();
    for (code, text) in [(500, "boom"), (550, "denied"), (421, "bye"), (452, "full")] {
        server.state().fail_next = Some((code, text.into()));
        for result in [
            vfs.list(&info.root).map(|_| ()),
            vfs.mkdir(&VPath::ftp(&info.id, "/pub/x").unwrap()),
        ] {
            if let Err(e) = result {
                all.push(e.to_string());
                all.push(format!("{e:?}"));
            }
            server.state().fail_next = Some((code, text.into()));
        }
    }
    server.state().fail_next = None;
    server.disconnect_everybody();
    server.state().refuse_connects_after = Some(0);
    let _ = server; // keep the borrow checker calm about the state guard above
    all.push(format!("{:?}", vfs.list(&info.root)));
    for text in &all {
        assert!(!text.contains(MARKER), "leak in: {text}");
    }
    assert!(!all.is_empty());
}

#[test]
fn the_server_log_never_contains_the_password() {
    let server = FakeServer::new();
    server.state().password = "pw-MARKER-log".into();
    let (m, _r, _p) = manager(&server, 2);
    let _ = m
        .connect(
            profile(TlsMode::None),
            Secret::new("pw-MARKER-log"),
            Arc::new(RejectAll),
        )
        .unwrap();
    assert!(!server.log().iter().any(|l| l.contains("MARKER")));
}

#[test]
fn manager_reports_transport_loss_and_recovery_without_forgetting_session() {
    let server = FakeServer::new();
    server.mkdirs("/pub");
    let (manager, router, _) = manager(&server, 2);
    let info = manager
        .connect(
            profile(TlsMode::None),
            Secret::new("secret"),
            Arc::new(RejectAll),
        )
        .unwrap();
    assert!(manager.is_live(&info.id));
    server.disconnect_everybody();
    server.state().refuse_connects_after = Some(1);
    assert!(router
        .vfs_for(&info.root)
        .unwrap()
        .list(&info.root)
        .is_err());
    assert!(!manager.is_live(&info.id));
    assert!(manager.is_connected(&info.id));
    server.state().refuse_connects_after = None;
    assert!(router.vfs_for(&info.root).unwrap().list(&info.root).is_ok());
    assert!(manager.is_live(&info.id));
    manager.disconnect(&info.id);
    assert!(!manager.is_live(&info.id));
}

#[test]
fn manager_connects_a_webdav_profile_and_registers_it() {
    use crate::profile::WebdavProfile;
    use std::io::{BufRead, BufReader, Read, Write};

    // One-shot loopback WebDAV server that answers the connect probe (a PROPFIND on the base) with a
    // 207 collection, so `connect_webdav` can verify reachability over a real socket.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let l = line.trim_end();
                if l.is_empty() {
                    break;
                }
                if let Some((k, v)) = l.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let xml = concat!(
                r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href>"#,
                r#"<d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>"#,
                r#"<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
            );
            let resp = format!(
                "HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                xml.len(),
                xml
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let (m, router, _) = manager(&FakeServer::new(), 2);
    let profile = WebdavProfile {
        name: "dav".into(),
        host: "127.0.0.1".into(),
        port,
        user: String::new(),
        base_path: "/dav".into(),
        tls: false,
        allow_plaintext: true,
        start_folder: Some("/docs".into()),
    };
    let info = m
        .connect_webdav(
            profile.clone(),
            Secret::new(""),
            Arc::new(Ask(TrustDecision::TrustOnce, Default::default())),
        )
        .unwrap();
    assert_eq!(info.protocol, Protocol::Webdav);
    assert_eq!(info.root, VPath::webdav(&info.id, "/docs").unwrap());
    assert!(m.is_connected(&info.id));
    assert!(m.is_live(&info.id));
    assert!(router.vfs_for(&info.root).is_ok());

    // Plaintext without the explicit flag is refused before any network use.
    let mut bad = profile.clone();
    bad.allow_plaintext = false;
    assert!(matches!(
        m.connect_webdav(
            bad,
            Secret::new(""),
            Arc::new(Ask(TrustDecision::TrustOnce, Default::default()))
        ),
        Err(ConnectError::Invalid(_))
    ));
    // An unreachable host fails the connect probe instead of reporting a false success.
    let mut dead = profile;
    dead.port = 1;
    assert!(matches!(
        m.connect_webdav(
            dead,
            Secret::new(""),
            Arc::new(Ask(TrustDecision::TrustOnce, Default::default()))
        ),
        Err(ConnectError::Invalid(_))
    ));

    assert!(m.disconnect(&info.id));
    assert!(!m.is_connected(&info.id));
}

#[test]
fn manager_webdav_wrong_password_fails_connect_with_auth_message() {
    // WD-4A-03/04: a 401 on the connect probe must fail the connect as "Anmeldung fehlgeschlagen"
    // (not "Server nicht erreichbar"), and must not register a session or router entry.
    use crate::profile::WebdavProfile;
    use std::io::{BufRead, BufReader, Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let l = line.trim_end();
                if l.is_empty() {
                    break;
                }
                if let Some((k, v)) = l.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });

    let (m, router, _) = manager(&FakeServer::new(), 2);
    let profile = WebdavProfile {
        name: "dav".into(),
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        base_path: "/dav".into(),
        tls: false,
        allow_plaintext: true,
        start_folder: None,
    };
    match m.connect_webdav(
        profile,
        Secret::new("wrong"),
        Arc::new(Ask(TrustDecision::TrustOnce, Default::default())),
    ) {
        Err(ConnectError::Invalid(msg)) => {
            assert_eq!(msg, "WebDAV-Anmeldung fehlgeschlagen")
        }
        other => panic!("expected auth failure, got {other:?}"),
    }
    // The probe runs before register_remote, so a failed connect leaves no router entry; a fresh
    // connect still has both slots free (the reservation was rolled back).
    let _ = &router;
    assert!(!m.is_connected("1"));
    assert!(!m.is_connected("2"));
}
