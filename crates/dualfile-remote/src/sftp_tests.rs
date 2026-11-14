//! Behaviour tests of `SftpVfs`, the SFTP host key policy and the connection manager (SFTP
//! sessions) against the in-memory server. Nothing here opens a socket.

use crate::error::{SftpStatus, TransportError};
use crate::fake::{FakeConnector, FakeServer};
use crate::ftpvfs::Opener;
use crate::manager::{ConnectError, ConnectionManager, ManagerConfig};
use crate::profile::{SftpAuth, SftpConnectParams, SftpProfile};
use crate::secret::Secret;
use crate::sftp_fake::{FakeSftpConnector, FakeSftpServer, FakeSftpTransport, HostKey, SNode};
use crate::sftpvfs::{SftpConfig, SftpVfs};
use crate::trust::*;
use dualfile_core::ops::{self, Conflict, ConflictResolver, JobHandle, Resolution};
use dualfile_core::vfs::{EntryKind, ReplaceMode, VPath, Vfs, WriteMode};
use dualfile_core::{Error, VfsRouter};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type TestVfs = Arc<SftpVfs<FakeSftpTransport>>;

fn vfs_with(server: &FakeSftpServer, id: &str, cfg: SftpConfig) -> TestVfs {
    let first = server
        .connect("user", &SftpAuth::Password, &Secret::new("secret"))
        .unwrap();
    let s = server.clone();
    let opener: Opener<FakeSftpTransport> =
        Arc::new(move || s.connect("user", &SftpAuth::Password, &Secret::new("secret")));
    SftpVfs::new(id, first, opener, cfg, Some(Secret::new("secret"))).unwrap()
}

fn cfg() -> SftpConfig {
    SftpConfig::default()
}

fn setup() -> (FakeSftpServer, TestVfs) {
    let server = FakeSftpServer::new();
    let vfs = vfs_with(&server, "s1", cfg());
    (server, vfs)
}

fn p(path: &str) -> VPath {
    VPath::sftp("s1", path).unwrap()
}

fn lp(path: &std::path::Path) -> VPath {
    VPath::local(path).unwrap()
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 253) as u8).collect()
}

fn local_file(dir: &std::path::Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path
}

fn copy_up(src: &std::path::Path, vfs: &TestVfs, dir: &str) -> ops::OpReport {
    ops::copy(
        &dualfile_core::LocalVfs,
        &[lp(src)],
        &**vfs,
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

fn raw(name: &str, kind: crate::sftp_transport::RawKind) -> crate::sftp_transport::RawEntry {
    crate::sftp_transport::RawEntry {
        name: name.into(),
        attrs: crate::sftp_transport::RawAttrs {
            kind,
            size: Some(1),
            mtime: None,
            atime: None,
            permissions: Some(0o644),
        },
    }
}

// ----------------------------------------------------------------------- listing and stat

#[test]
fn lists_and_stats_files_dirs_and_links() {
    let (server, vfs) = setup();
    server.put("/d/a.txt", b"hello");
    server.mkdirs("/d/sub");
    server.put_link("/d/l", "a.txt");
    let list = vfs.list(&p("/d")).unwrap();
    let names: Vec<_> = list.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(list.len(), 3, "'.' and '..' are not entries: {names:?}");
    let a = list.iter().find(|e| e.name == "a.txt").unwrap();
    assert_eq!((a.kind, a.size), (EntryKind::File, 5));
    assert!(a.modified.is_some() && a.error.is_none());
    let sub = list.iter().find(|e| e.name == "sub").unwrap();
    assert_eq!(sub.kind, EntryKind::Dir);
    let l = list.iter().find(|e| e.name == "l").unwrap();
    assert_eq!(l.kind, EntryKind::Symlink);
    assert_eq!(l.link_target.as_deref(), Some("a.txt"));
    assert_eq!(vfs.stat(&p("/d/a.txt")).unwrap().size, 5);
    assert_eq!(vfs.stat(&p("/d")).unwrap().kind, EntryKind::Dir);
    assert_eq!(vfs.stat(&p("/")).unwrap().kind, EntryKind::Dir);
    assert!(matches!(vfs.stat(&p("/nope")), Err(Error::NotFound(_))));
    assert!(matches!(vfs.list(&p("/nope")), Err(Error::NotFound(_))));
    assert!(matches!(vfs.list(&p("/d/a.txt")), Err(Error::Remote(_))));
    assert_eq!(vfs.stat_file(&p("/d/a.txt")).unwrap().size, 5);
    assert!(matches!(
        vfs.stat_file(&p("/d")),
        Err(Error::IsADirectory(_))
    ));
    // the start folder the server reports
    assert_eq!(vfs.home_dir().unwrap(), "/home/user");
}

#[test]
fn hostile_names_are_marked_and_never_usable() {
    use crate::sftp_transport::RawKind::*;
    let (server, vfs) = setup();
    server.mkdirs("/d");
    let long = "n".repeat(300);
    server.state().raw_names = Some(vec![
        raw("..", Dir),
        raw("a/b", File),
        raw("../x", File),
        raw("a\\b", File),
        raw("a\0b", File),
        raw("a\nb", File),
        raw(&long, File),
        raw("Readme", File),
        raw("README", File),
        raw("dup", File),
        raw("dup", File),
        raw("fine.txt", File),
        raw("fifo", Other),
        raw("evil\nlink", Symlink),
        raw("good-link", Symlink),
    ]);
    server.put_link("/d/good-link", "t\u{1b}[31marget");
    let list = vfs.list(&p("/d")).unwrap();
    // ".." (listed by name) is the only name dropped; everything else is shown, marked
    assert_eq!(
        list.len(),
        14,
        "{:?}",
        list.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    let marked = list.iter().filter(|e| e.error.is_some()).count();
    assert_eq!(marked, 12, "only fine.txt and good-link are usable");
    let fine = list.iter().find(|e| e.name == "fine.txt").unwrap();
    assert!(fine.error.is_none());
    for e in &list {
        assert!(!e.name.contains('/') && !e.name.contains('\\'));
        assert!(!e.name.chars().any(char::is_control), "{:?}", e.name);
        assert!(e.name.len() <= 255);
    }
    // link targets are shown but cleaned; no readlink for a bad name
    let good = list.iter().find(|e| e.name == "good-link").unwrap();
    assert!(!good.link_target.as_deref().unwrap().contains('\u{1b}'));
    assert!(
        !server.log().iter().any(|l| l.contains("evil")),
        "{:?}",
        server.log()
    );
    // the core refuses to even build a path with such a name (defense in depth: the vfs
    // checks the name as well, see `create_write`)
    server.state().raw_names = None;
    assert!(VPath::sftp("s1", &format!("/d/{long}")).is_err());
    assert!(VPath::sftp("s1", "/d/a\\b").is_err());
    // nothing was opened for any of the hostile entries
    assert!(!server.log().iter().any(|l| l.starts_with("OPEN-")));
}

#[test]
fn oversized_listings_and_slow_listings_are_errors_not_a_silent_cut() {
    let server = FakeSftpServer::new();
    for i in 0..50 {
        server.put(&format!("/d/f{i}"), b"x");
    }
    server.state().page_size = 7;
    let mut c = cfg();
    c.limits.max_entries = 10;
    let vfs = vfs_with(&server, "s1", c);
    assert!(matches!(vfs.list(&p("/d")), Err(Error::Remote(_))));
    // exactly at the limit is fine
    let mut c = cfg();
    c.limits.max_entries = 50;
    let vfs = vfs_with(&server, "s1", c);
    assert_eq!(vfs.list(&p("/d")).unwrap().len(), 50);
    // byte budget
    let mut c = cfg();
    c.limits.max_list_bytes = 500;
    let vfs = vfs_with(&server, "s1", c);
    assert!(matches!(vfs.list(&p("/d")), Err(Error::Remote(_))));
    // time budget: the listing has more pages than time
    let mut c = cfg();
    c.list_deadline = Duration::ZERO;
    let vfs = vfs_with(&server, "s1", c);
    let e = vfs.list(&p("/d")).unwrap_err();
    assert!(e.to_string().contains("zu lange"), "{e}");
    assert_eq!(vfs.connects(), 1, "a slow listing is no lost connection");
    // too many link reads are skipped, not endless
    let server = FakeSftpServer::new();
    for i in 0..10 {
        server.put_link(&format!("/l/k{i}"), "t");
    }
    let mut c = cfg();
    c.max_link_reads = 3;
    let vfs = vfs_with(&server, "s1", c);
    let with_target = vfs
        .list(&p("/l"))
        .unwrap()
        .iter()
        .filter(|e| e.link_target.is_some())
        .count();
    assert_eq!(with_target, 3);
}

// ----------------------------------------------------------------------- reading

#[test]
fn reads_whole_files_ranges_and_resumes_at_an_offset() {
    let (server, vfs) = setup();
    let data = payload(300_000);
    server.put("/f", &data);
    let mut got = Vec::new();
    vfs.open_read(&p("/f"))
        .unwrap()
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got, data);
    assert_eq!(vfs.read_range(&p("/f"), 100, 50).unwrap(), &data[100..150]);
    assert!(vfs.read_range(&p("/f"), 400_000, 50).unwrap().is_empty());
    let mut tail = Vec::new();
    vfs.open_read_from(&p("/f"), 299_000)
        .unwrap()
        .read_to_end(&mut tail)
        .unwrap();
    assert_eq!(tail, &data[299_000..]);
    assert!(server
        .log()
        .iter()
        .any(|l| l == "READ 1 299000" || l.starts_with("READ") && l.ends_with(" 299000")));
    assert!(matches!(
        vfs.open_read(&p("/missing")),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        vfs.open_read(&p("/")),
        Err(Error::IsADirectory(_))
    ));
    // every handle is closed again
    assert_eq!(server.state().open_handles, 0);
}

#[test]
fn links_are_links_and_are_never_followed() {
    let (server, vfs) = setup();
    server.put("/target.txt", b"keep me");
    server.put_link("/l", "/target.txt");
    let e = vfs.stat(&p("/l")).unwrap();
    assert_eq!(e.kind, EntryKind::Symlink);
    assert_eq!(e.link_target.as_deref(), Some("/target.txt"));
    assert_eq!(vfs.read_link(&p("/l")).unwrap(), "/target.txt");
    assert!(matches!(
        vfs.read_link(&p("/target.txt")),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        vfs.open_read(&p("/l")),
        Err(Error::Unsupported(_))
    ));
    assert!(!server.log().iter().any(|l| l.starts_with("OPEN-R")));
    // deleting the link removes the link only
    vfs.remove_permanent(&p("/l")).unwrap();
    assert!(!server.exists("/l"));
    assert_eq!(server.get("/target.txt").unwrap(), b"keep me");
    // a copy of a link down to disk is a link, not the target's content; a link that could
    // point out of the copied tree (absolute) is skipped with a warning, as for archives
    server.put_link("/abs", "/target.txt");
    server.put_link("/l2", "target.txt");
    let tmp = tempfile::tempdir().unwrap();
    let report = ops::copy(
        &*vfs,
        &[p("/abs"), p("/l2")],
        &dualfile_core::LocalVfs,
        &lp(tmp.path()),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.skipped.len(), 1);
    assert!(
        !tmp.path().join("abs").exists()
            && std::fs::symlink_metadata(tmp.path().join("abs")).is_err()
    );
    let made = tmp.path().join("l2");
    assert!(std::fs::symlink_metadata(&made)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!server.log().iter().any(|l| l.starts_with("OPEN-R")));
    // creating links
    vfs.create_symlink("rel/target", &p("/new")).unwrap();
    assert_eq!(vfs.read_link(&p("/new")).unwrap(), "rel/target");
    assert!(matches!(
        vfs.create_symlink("x", &p("/new")),
        Err(Error::Exists(_))
    ));
    for bad in ["", "a\nb", "a\0b"] {
        assert!(vfs.create_symlink(bad, &p("/bad")).is_err(), "{bad:?}");
    }
    assert!(!server.exists("/bad"));
}

// ----------------------------------------------------------------------- writing

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
    assert_eq!(server.state().open_handles, 0);
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
    // metadata (time, permissions) is carried along where the server can
    let meta = server.state().nodes.get("/in/big.bin").cloned().unwrap();
    assert!(matches!(meta, SNode::File { .. }));
}

#[test]
fn create_write_uses_a_temporary_name_and_cleans_up() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let mut w = vfs
        .create_write(&p("/in/t.txt"), WriteMode::NoReplace)
        .unwrap();
    w.write_all(b"abc").unwrap();
    // before commit only a temporary name exists
    let before = server.names_in("/in");
    assert_eq!(before.len(), 1);
    assert!(before[0].starts_with("t.txt.dualfile-tmp-"), "{before:?}");
    w.commit().unwrap();
    assert_eq!(server.names_in("/in"), ["t.txt"]);
    assert_eq!(server.get("/in/t.txt").unwrap(), b"abc");
    // dropped without commit: the temporary file is removed
    let mut w = vfs
        .create_write(&p("/in/u.txt"), WriteMode::NoReplace)
        .unwrap();
    w.write_all(b"zzz").unwrap();
    drop(w);
    assert_eq!(server.names_in("/in"), ["t.txt"]);
    // an empty file works
    let mut w = vfs
        .create_write(&p("/in/empty"), WriteMode::NoReplace)
        .unwrap();
    w.commit().unwrap();
    assert_eq!(server.get("/in/empty").unwrap().len(), 0);
    // a missing parent is "not found", nothing is created
    assert!(matches!(
        vfs.create_write(&p("/nodir/x"), WriteMode::NoReplace),
        Err(Error::NotFound(_))
    ));
    assert_eq!(server.state().open_handles, 0);
}

#[test]
fn a_temporary_name_that_exists_is_skipped_and_never_overwritten() {
    // the fake refuses CreateNew on an existing name like OpenSSH: nothing is truncated
    let (server, vfs) = setup();
    server.mkdirs("/in");
    server.put("/in/a.txt", b"mine");
    let n_before = server.names_in("/in").len();
    let mut w = vfs
        .create_write(&p("/in/b.txt"), WriteMode::NoReplace)
        .unwrap();
    w.write_all(b"1").unwrap();
    w.commit().unwrap();
    assert_eq!(server.get("/in/a.txt").unwrap(), b"mine");
    assert_eq!(server.names_in("/in").len(), n_before + 1);
    assert!(server
        .log()
        .iter()
        .filter(|l| l.starts_with("OPEN-W"))
        .all(|l| l.contains("CreateNew")));
}

fn abort_then(
    setup_next: impl FnOnce(&FakeSftpServer, &std::path::Path),
) -> (
    FakeSftpServer,
    TestVfs,
    tempfile::TempDir,
    std::path::PathBuf,
    ops::OpReport,
) {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "v.bin", &payload(2_200_000));
    server.state().cut_writes_after = Some(1_100_000);
    assert_eq!(copy_up(&file, &vfs, "/in").failed.len(), 1);
    server.state().cut_writes_after = None;
    setup_next(&server, &file);
    let report = copy_up(&file, &vfs, "/in");
    (server, vfs, tmp, file, report)
}

fn continued(server: &FakeSftpServer) -> bool {
    server
        .log()
        .iter()
        .any(|l| l.starts_with("OPEN-W Existing"))
}

#[test]
fn aborted_upload_continues_after_the_verified_part() {
    let (server, vfs) = setup();
    server.mkdirs("/in");
    let tmp = tempfile::tempdir().unwrap();
    let data = payload(2_500_000);
    let file = local_file(tmp.path(), "big.bin", &data);
    server.state().cut_writes_after = Some(1_048_576);
    let first = copy_up(&file, &vfs, "/in");
    assert_eq!(first.failed.len(), 1);
    assert!(!server.exists("/in/big.bin"));
    // (the meta file's few bytes count against the cut, too)
    let part = server.get("/in/big.bin.dualfile-part").unwrap().len();
    assert!((1_000_000..=1_048_576).contains(&part), "{part}");
    let meta = String::from_utf8(server.get("/in/big.bin.dualfile-part.meta").unwrap()).unwrap();
    assert!(meta.contains(&format!("\"written\":{part}")), "{meta}");
    server.state().cut_writes_after = None;
    let before = server.state().written;
    let second = copy_up(&file, &vfs, "/in");
    assert!(second.failed.is_empty(), "{:?}", second.failed);
    assert_eq!(server.get("/in/big.bin").unwrap(), data);
    assert!(continued(&server), "{:?}", server.log());
    let sent = server.state().written - before;
    assert!(
        sent < data.len() - 1_000_000,
        "only the rest is sent, sent {sent}"
    );
    assert_eq!(server.names_in("/in"), ["big.bin"]);
    assert_eq!(server.state().open_handles, 0);
}

#[test]
fn changed_source_or_time_starts_over() {
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
    assert!(!continued(&server), "no continuing");
    let (server, _v, _t, file, report) = abort_then(|_, file| {
        let f = std::fs::File::options().write(true).open(file).unwrap();
        f.set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
    });
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        server.get("/in/v.bin").unwrap(),
        std::fs::read(&file).unwrap()
    );
    assert!(!continued(&server));
}

#[test]
fn damaged_meta_or_wrong_server_size_starts_over() {
    for tamper in 0..3 {
        let (server, _v, _t, file, report) = abort_then(|server, _| match tamper {
            0 => {
                let mut s = server.state();
                if let Some(SNode::File { data, .. }) =
                    s.nodes.get_mut("/in/v.bin.dualfile-part.meta")
                {
                    data.truncate(data.len() - 6);
                }
            }
            1 => {
                let mut s = server.state();
                if let Some(SNode::File { data, .. }) = s.nodes.get_mut("/in/v.bin.dualfile-part") {
                    data.extend_from_slice(b"junk");
                }
            }
            _ => {
                let mut s = server.state();
                let len = 2_200_000 + 50;
                if let Some(SNode::File { data, .. }) = s.nodes.get_mut("/in/v.bin.dualfile-part") {
                    data.resize(len, 0);
                }
                let name = "/in/v.bin.dualfile-part.meta";
                if let Some(SNode::File { data, .. }) = s.nodes.get_mut(name) {
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
        assert!(!continued(&server), "case {tamper}");
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
    vfs.discard_resumable(&p("/in/y.bin")).unwrap();
    assert!(
        server.exists("/in/y.bin.dualfile-part") && server.exists("/in/y.bin.dualfile-part.meta")
    );
    // no truncating open ever hit a foreign name
    assert!(!server
        .log()
        .iter()
        .any(|l| l.starts_with("OPEN-W CreateTruncate") && l.contains("dualfile-part ")));
}

#[test]
fn a_foreign_directory_in_the_place_of_the_partial_file_is_skipped() {
    let (server, vfs) = setup();
    server.mkdirs("/in/d.bin.dualfile-part");
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "d.bin", &payload(1000));
    assert!(copy_up(&file, &vfs, "/in").failed.is_empty());
    assert_eq!(server.get("/in/d.bin").unwrap().len(), 1000);
    assert!(matches!(
        server.state().nodes.get("/in/d.bin.dualfile-part"),
        Some(SNode::Dir)
    ));
}

#[test]
fn discard_removes_our_partial_file() {
    let (server, vfs, _t, _f, _r) = abort_then(|_, _| {});
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "z.bin", &payload(2_200_000));
    server.state().cut_writes_after = Some(1_100_000);
    assert_eq!(copy_up(&file, &vfs, "/in").failed.len(), 1);
    assert!(server.exists("/in/z.bin.dualfile-part"));
    vfs.discard_resumable(&p("/in/z.bin")).unwrap();
    assert!(
        !server.exists("/in/z.bin.dualfile-part") && !server.exists("/in/z.bin.dualfile-part.meta")
    );
}

#[test]
fn cancelling_keeps_partial_file_and_meta_that_match() {
    struct Late(Arc<Mutex<Option<JobHandle>>>, AtomicU32);
    impl ops::ProgressSink for Late {
        fn event(&self, e: ops::ProgressEvent) {
            if matches!(e, ops::ProgressEvent::BytesCopied { .. })
                && self.1.fetch_add(1, Ordering::SeqCst) == 1
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

// ----------------------------------------------------------------------- conflicts, rename

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
    let tmp = tempfile::tempdir().unwrap();
    let file = local_file(tmp.path(), "r.txt", b"mine");
    // between our look-up and the rename another client creates the target; a server that
    // refuses (like OpenSSH) keeps it, and we report "exists"
    server.state().inject_before_rename = Some(("/in/r.txt".into(), b"theirs".to_vec()));
    let report = copy_up(&file, &vfs, "/in");
    assert_eq!(server.get("/in/r.txt").unwrap(), b"theirs");
    assert!(report.done.is_empty());
    assert!(
        server.exists("/in/r.txt.dualfile-part"),
        "the partial file stays for another try"
    );
    // a direct write: the temporary file is removed again
    server.state().inject_before_rename = Some(("/in/q.txt".into(), b"theirs".to_vec()));
    let mut w = vfs
        .create_write(&p("/in/q.txt"), WriteMode::NoReplace)
        .unwrap();
    w.write_all(b"mine").unwrap();
    assert!(matches!(w.commit(), Err(Error::Exists(_))));
    drop(w);
    assert_eq!(server.get("/in/q.txt").unwrap(), b"theirs");
    assert!(!server
        .names_in("/in")
        .iter()
        .any(|n| n.contains("dualfile-tmp")));
}

#[test]
fn replace_uses_posix_rename_and_never_deletes_the_original_first() {
    let (server, vfs) = setup();
    server.put("/in/o.txt", b"old");
    let mut w = vfs
        .create_write(&p("/in/o.txt"), WriteMode::Replace)
        .unwrap();
    w.write_all(b"new").unwrap();
    w.commit().unwrap();
    assert_eq!(server.get("/in/o.txt").unwrap(), b"new");
    assert!(server.log().iter().any(|l| l.starts_with("POSIX-RENAME")));
    assert!(!server.log().iter().any(|l| l == "REMOVE /in/o.txt"));
    // a server without posix-rename: replacing an existing file is refused, the original stays
    let server = FakeSftpServer::new();
    server.state().features.posix_rename = false;
    server.put("/in/o.txt", b"old");
    let vfs = vfs_with(&server, "s1", cfg());
    let mut w = vfs
        .create_write(&p("/in/o.txt"), WriteMode::Replace)
        .unwrap();
    w.write_all(b"new").unwrap();
    assert!(matches!(w.commit(), Err(Error::Unsupported(_))));
    drop(w);
    assert_eq!(server.get("/in/o.txt").unwrap(), b"old");
    assert!(!server.log().iter().any(|l| l == "REMOVE /in/o.txt"));
    assert_eq!(
        server.names_in("/in"),
        ["o.txt"],
        "temporary file cleaned up"
    );
    // replace of a name that does not exist is a plain rename
    let mut w = vfs
        .create_write(&p("/in/n.txt"), WriteMode::Replace)
        .unwrap();
    w.write_all(b"fresh").unwrap();
    w.commit().unwrap();
    assert_eq!(server.get("/in/n.txt").unwrap(), b"fresh");
    // vfs.rename with Replace
    let (server, vfs) = setup();
    server.put("/a", b"1");
    server.put("/b", b"2");
    vfs.rename(&p("/a"), &p("/b"), ReplaceMode::Replace)
        .unwrap();
    assert_eq!(server.get("/b").unwrap(), b"1");
    assert!(!server.exists("/a"));
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
    assert!(
        !server.log().iter().any(|l| l.contains("RENAME")),
        "no rename was sent"
    );
    vfs.rename(&p("/a.txt"), &p("/c.txt"), ReplaceMode::NoReplace)
        .unwrap();
    assert!(server.exists("/c.txt") && !server.exists("/a.txt"));
    assert!(matches!(vfs.mkdir(&p("/c.txt")), Err(Error::Exists(_))));
    assert!(matches!(vfs.mkdir(&p("/no/dir")), Err(Error::NotFound(_))));
    vfs.mkdir(&p("/d")).unwrap();
    assert!(server.exists("/d"));
    assert!(matches!(
        vfs.rename(&p("/missing"), &p("/z"), ReplaceMode::NoReplace),
        Err(Error::NotFound(_))
    ));
    // a server that replaces on plain rename: the look-up still protects the common case
    server.state().rename_replaces = true;
    server.put("/e", b"e");
    assert!(matches!(
        vfs.rename(&p("/c.txt"), &p("/e"), ReplaceMode::NoReplace),
        Err(Error::Exists(_))
    ));
    assert_eq!(server.get("/e").unwrap(), b"e");
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
        .any(|l| l.starts_with("REMOVE") || l.starts_with("RMDIR")));
    server.put("/d/g.txt", b"y");
    let plan = ops::plan_delete(&*vfs, &[p("/d")]);
    assert!(!plan.incomplete && plan.files == 1 && plan.dirs == 1);
    let report = ops::delete_permanent(&*vfs, &plan, plan.token, &JobHandle::silent());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!server.exists("/d") && server.exists("/f.txt"));
    // a folder with content is not removed, and says why
    server.put("/full/x", b"1");
    assert!(matches!(
        vfs.remove_permanent(&p("/full")),
        Err(Error::DirectoryNotEmpty(_))
    ));
    assert!(server.exists("/full/x"));
}

#[test]
fn a_delete_plan_of_another_connection_is_refused() {
    let server = FakeSftpServer::new();
    server.put("/d/f", b"x");
    let a = vfs_with(&server, "s1", cfg());
    let b = vfs_with(&server, "s2", cfg());
    let plan = ops::plan_delete(&*a, &[p("/d")]);
    let report = ops::delete_permanent(&*b, &plan, plan.token, &JobHandle::silent());
    assert!(report.done.is_empty() && !report.failed.is_empty());
    assert!(server.exists("/d/f"));
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
    server.state().cut_writes_after = Some(10);
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
fn move_on_one_connection_is_a_rename_and_between_connections_a_copy() {
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
    assert!(server
        .log()
        .iter()
        .any(|l| l.starts_with("RENAME /a/f.txt")));
    // two connections (here to the same fake server): never a rename across them
    let other = vfs_with(&server, "s2", cfg());
    server.mkdirs("/c");
    let before = server.log().len();
    let report = ops::move_(
        &*vfs,
        &[p("/b/f.txt")],
        &*other,
        &VPath::sftp("s2", "/c").unwrap(),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/c/f.txt").unwrap(), b"1");
    assert!(!server.exists("/b/f.txt"));
    assert!(
        !server.log()[before..]
            .iter()
            .any(|l| l.starts_with("RENAME /b/f.txt /c/f.txt")),
        "copy + verify + remove"
    );
}

#[test]
fn set_metadata_sets_what_the_server_takes_and_leaves_links_alone() {
    let (server, vfs) = setup();
    server.put("/f", b"x");
    server.put_link("/l", "/f");
    let when = std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    vfs.set_metadata(&p("/f"), Some(0o600), Some(when)).unwrap();
    match server.state().nodes.get("/f").unwrap() {
        SNode::File { mode, mtime, .. } => assert_eq!((*mode, *mtime), (0o600, 1_600_000_000)),
        other => panic!("{other:?}"),
    }
    let before = server.log().len();
    vfs.set_metadata(&p("/l"), Some(0o777), Some(when)).unwrap();
    assert!(!server.log()[before..]
        .iter()
        .any(|l| l.starts_with("SETSTAT")));
    vfs.set_metadata(&p("/f"), None, None).unwrap();
    server.state().fail_next = Some((SftpStatus::PermissionDenied, "no".into()));
    // (the lstat consumes the injected failure: any error is reported to the caller)
    assert!(vfs.set_metadata(&p("/f"), Some(0o644), None).is_err());
    assert!(matches!(
        vfs.set_metadata(&p("/missing"), Some(0o644), None),
        Err(Error::NotFound(_))
    ));
}

// ----------------------------------------------------------------------- budgets and limits

#[test]
fn a_lost_connection_is_replaced_within_the_budget() {
    let server = FakeSftpServer::new();
    server.put("/f", b"x");
    let vfs = vfs_with(
        &server,
        "s1",
        SftpConfig {
            max_reconnects: 2,
            ..cfg()
        },
    );
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 1);
    server.disconnect_everybody();
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 1);
    assert_eq!(vfs.connects(), 2);
    assert!(vfs.is_live());
    // a changing request is not repeated blindly
    server.disconnect_everybody();
    assert!(matches!(vfs.mkdir(&p("/m")), Err(Error::Disconnected)));
    assert!(!server.exists("/m"));
    // A typed disconnect no longer needs the generic-error "already exists" probe.
    assert!(!vfs.is_live());
    assert_eq!(
        server
            .log()
            .iter()
            .filter(|l| l.starts_with("MKDIR"))
            .count(),
        0
    );
    assert!(vfs.mkdir(&p("/m")).is_ok());
    assert_eq!(vfs.connects(), 3);
    assert!(vfs.is_live());
    server.disconnect_everybody();
    let _ = vfs.list(&p("/"));
    server.disconnect_everybody();
    let _ = vfs.list(&p("/"));
    server.disconnect_everybody();
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    assert!(!vfs.is_live());
    assert!(vfs.connects() <= 3 + 2 + 1);
}

#[test]
fn a_request_that_times_out_is_an_error_and_cannot_hang_the_session() {
    let server = FakeSftpServer::new();
    let vfs = vfs_with(
        &server,
        "s1",
        SftpConfig {
            max_reconnects: 0,
            ..cfg()
        },
    );
    server.state().hang_next = true;
    assert!(vfs.list(&p("/")).is_err());
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    let vfs = vfs_with(&server, "s1", cfg());
    server.state().hang_next = true;
    assert!(vfs.list(&p("/")).is_ok());
}

#[test]
fn a_transfer_never_continues_on_a_replaced_connection() {
    let (server, vfs) = setup();
    server.put("/f", &payload(500_000));
    server.put("/g", &payload(500_000));
    let mut reader = vfs.open_read(&p("/f")).unwrap();
    let mut buf = [0u8; 1000];
    assert_eq!(reader.read(&mut buf).unwrap(), 1000);
    server.disconnect_everybody();
    // another operation makes a new connection...
    assert_eq!(vfs.list(&p("/")).unwrap().len(), 2);
    // ...the old handle is not used on it
    let before = server.log().len();
    assert!(reader.read(&mut buf).is_err());
    assert!(!server.log()[before..].iter().any(|l| l.starts_with("READ")));
    drop(reader);
    // and a fresh download works
    let mut got = Vec::new();
    vfs.open_read(&p("/g"))
        .unwrap()
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got.len(), 500_000);
}

#[test]
fn the_number_of_parallel_transfers_is_capped() {
    let server = FakeSftpServer::new();
    server.put("/f", &payload(100));
    let vfs = vfs_with(
        &server,
        "s1",
        SftpConfig {
            max_transfers: 2,
            ..cfg()
        },
    );
    let one = vfs.open_read(&p("/f")).unwrap();
    let _two = vfs.open_read(&p("/f")).unwrap();
    assert!(matches!(vfs.open_read(&p("/f")), Err(Error::Remote(_))));
    drop(one);
    assert!(vfs.open_read(&p("/f")).is_ok());
    // writers count, too
    let w1 = vfs.create_write(&p("/w1"), WriteMode::NoReplace).unwrap();
    assert!(vfs.create_write(&p("/w2"), WriteMode::NoReplace).is_err());
    drop(w1);
}

#[test]
fn closing_disconnects_everything() {
    let (_server, vfs) = setup();
    assert!(vfs.list(&p("/")).is_ok());
    vfs.close();
    assert!(vfs.is_closed());
    assert!(matches!(vfs.list(&p("/")), Err(Error::Disconnected)));
    assert!(matches!(vfs.open_read(&p("/f")), Err(Error::Disconnected)));
    assert!(SftpVfs::<FakeSftpTransport>::new(
        "BAD",
        FakeSftpServer::new()
            .connect("user", &SftpAuth::Password, &Secret::new("secret"))
            .unwrap(),
        Arc::new(|| Err(TransportError::Timeout)),
        cfg(),
        None
    )
    .is_err());
    // a path of another scheme or connection is not served
    let (_s, vfs) = setup();
    assert!(matches!(
        vfs.stat(&VPath::ftp("s1", "/").unwrap()),
        Err(Error::Disconnected)
    ));
}

#[test]
fn control_characters_and_overlong_paths_never_reach_the_wire() {
    let (server, vfs) = setup();
    server.put("/x", b"1");
    for bad in ["/a\nb", "/a\rb", "/a\u{1b}b", "/a\u{7f}b"] {
        let path = VPath::sftp("s1", bad);
        if let Ok(path) = path {
            assert!(vfs.stat(&path).is_err(), "{bad:?}");
            assert!(vfs.mkdir(&path).is_err());
            assert!(vfs.list(&path).is_err());
            assert!(vfs.open_read(&path).is_err());
            assert!(vfs.remove_permanent(&path).is_err());
            assert!(vfs.rename(&p("/x"), &path, ReplaceMode::NoReplace).is_err());
            assert!(vfs.create_symlink("t", &path).is_err());
        }
    }
    let long = format!("/{}", "d/".repeat(3000));
    if let Ok(path) = VPath::sftp("s1", &long) {
        assert!(matches!(vfs.stat(&path), Err(Error::InvalidPath(_))));
    }
    assert!(
        !server
            .log()
            .iter()
            .any(|l| l.contains('\n') || l.contains('\r') || l.contains('\u{1b}')),
        "{:?}",
        server.log()
    );
}

// ----------------------------------------------------------------------- profiles and manager

fn profile() -> SftpProfile {
    SftpProfile {
        transport: crate::profile::SshTransport::Builtin,
        name: "Test".into(),
        host: "sftp.example.org".into(),
        port: 22,
        user: "user".into(),
        auth: SftpAuth::Password,
        start_folder: Some("/pub".into()),
    }
}

#[test]
fn bad_profiles_are_refused_before_any_network_use() {
    assert!(profile().validate().is_ok());
    for mutate in [
        (|p: &mut SftpProfile| p.host = String::new()) as fn(&mut SftpProfile),
        |p| p.host = "a b".into(),
        |p| p.host = "a\r\nb".into(),
        |p| p.host = "a/b".into(),
        |p| p.port = 0,
        |p| p.user = String::new(),
        |p| p.user = "u\nx".into(),
        |p| p.start_folder = Some("rel".into()),
        |p| p.start_folder = Some("/a\nb".into()),
        |p| {
            p.auth = SftpAuth::KeyFile {
                path: String::new(),
            }
        },
        |p| {
            p.auth = SftpAuth::KeyFile {
                path: "/k\n".into(),
            }
        },
    ] {
        let mut p = profile();
        mutate(&mut p);
        assert!(p.validate().is_err(), "{p:?}");
    }
    let json = serde_json::to_string(&profile()).unwrap();
    assert!(!json.to_lowercase().contains("password\":"), "{json}");
    let back: SftpProfile = serde_json::from_str(&json).unwrap();
    assert_eq!(back, profile());
    let key = SftpProfile {
        auth: SftpAuth::KeyFile {
            path: "/home/u/.ssh/id".into(),
        },
        ..profile()
    };
    assert_eq!(
        serde_json::from_str::<SftpProfile>(&serde_json::to_string(&key).unwrap()).unwrap(),
        key
    );
}

type TestManager = ConnectionManager<FakeConnector, FakeSftpConnector>;

fn manager(
    sftp: &FakeSftpServer,
    ftp: &FakeServer,
    max: usize,
) -> (TestManager, Arc<VfsRouter>, Arc<MemoryPins>) {
    let router = Arc::new(VfsRouter::new());
    let pins = Arc::new(MemoryPins::new());
    let m = ConnectionManager::with_sftp(
        FakeConnector {
            server: ftp.clone(),
        },
        FakeSftpConnector {
            server: sftp.clone(),
        },
        router.clone(),
        pins.clone(),
        ManagerConfig {
            max_connections: max,
            ..ManagerConfig::default()
        },
    );
    (m, router, pins)
}

struct Ask(TrustDecision, AtomicUsize);
impl ConfirmTrust for Ask {
    fn confirm(&self, _: &TrustRequest) -> TrustDecision {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0
    }
}
impl Ask {
    fn new(d: TrustDecision) -> Arc<Self> {
        Arc::new(Self(d, AtomicUsize::new(0)))
    }
    fn asked(&self) -> usize {
        self.1.load(Ordering::SeqCst)
    }
}

fn yes() -> Arc<Ask> {
    Ask::new(TrustDecision::TrustAlways)
}

#[test]
fn the_manager_registers_sftp_sessions_and_caps_the_total() {
    let sftp = FakeSftpServer::new();
    sftp.mkdirs("/pub");
    let ftp = FakeServer::new();
    ftp.mkdirs("/pub");
    let (m, router, _) = manager(&sftp, &ftp, 3);
    let a = m
        .connect_sftp(profile(), Secret::new("secret"), yes())
        .unwrap();
    assert_eq!(a.protocol, Protocol::Sftp);
    assert!(a.tls.is_none());
    assert_eq!(
        dualfile_core::vfs::format_ipc_path(&a.root),
        format!("sftp:{}!/pub", a.id)
    );
    let b = m
        .connect_sftp(profile(), Secret::new("secret"), yes())
        .unwrap();
    assert_ne!(a.id, b.id);
    // the router serves them
    let vfs = router.vfs_for(&a.root).unwrap();
    assert_eq!(
        vfs.remote_endpoint().map(|(s, _)| s),
        Some(dualfile_core::vfs::Scheme::Sftp)
    );
    assert!(!vfs.capabilities().can_trash);
    assert!(vfs.list(&a.root).is_ok());
    assert_eq!(m.connections().len(), 2);
    // FTP and SFTP share the cap
    let c = m
        .connect(
            crate::profile::FtpProfile {
                name: "f".into(),
                host: "ftp.example.org".into(),
                port: 21,
                user: "user".into(),
                tls: crate::profile::TlsMode::None,
                allow_plaintext: true,
                start_folder: None,
            },
            Secret::new("secret"),
            Arc::new(RejectAll),
        )
        .unwrap();
    assert_eq!(c.protocol, Protocol::Ftps);
    assert!(matches!(
        m.connect_sftp(profile(), Secret::new("secret"), yes()),
        Err(ConnectError::TooManyConnections)
    ));
    // disconnecting frees a slot, makes the locations "disconnected" at once, closes it
    assert!(m.disconnect(&a.id));
    assert!(matches!(vfs.list(&a.root), Err(Error::Disconnected)));
    assert!(matches!(router.vfs_for(&a.root), Err(Error::Disconnected)));
    assert!(m
        .connect_sftp(profile(), Secret::new("secret"), yes())
        .is_ok());
    m.disconnect_all();
    assert!(m.connections().is_empty());
    assert!(router.remote_ids().is_empty());
}

#[test]
fn start_folder_defaults_to_the_servers_home_and_bad_logins_register_nothing() {
    let sftp = FakeSftpServer::new();
    let ftp = FakeServer::new();
    let (m, router, _) = manager(&sftp, &ftp, 4);
    let mut no_folder = profile();
    no_folder.start_folder = None;
    let info = m
        .connect_sftp(no_folder.clone(), Secret::new("secret"), yes())
        .unwrap();
    assert!(dualfile_core::vfs::format_ipc_path(&info.root).ends_with("!/home/user"));
    let e = m
        .connect_sftp(no_folder, Secret::new("wrong"), yes())
        .unwrap_err();
    assert!(
        matches!(e, ConnectError::Transport(TransportError::Auth)),
        "{e}"
    );
    assert_eq!(router.remote_ids().len(), 1);
    let mut bad = profile();
    bad.port = 0;
    assert!(matches!(
        m.connect_sftp(bad, Secret::new("secret"), yes()),
        Err(ConnectError::Invalid(_))
    ));
    // a manager without SFTP refuses
    let plain = ConnectionManager::new(
        FakeConnector {
            server: FakeServer::new(),
        },
        Arc::new(VfsRouter::new()),
        Arc::new(MemoryPins::new()),
        ManagerConfig::default(),
    );
    assert!(plain
        .connect_sftp(profile(), Secret::new("secret"), yes())
        .is_err());
}

#[test]
fn key_file_logins_use_the_passphrase_as_the_secret() {
    let sftp = FakeSftpServer::new();
    sftp.state().key_passphrase = Some("phrase".into());
    let ftp = FakeServer::new();
    let (m, _, _) = manager(&sftp, &ftp, 4);
    let mut key = profile();
    key.auth = SftpAuth::KeyFile {
        path: "/k/id".into(),
    };
    assert!(m
        .connect_sftp(key.clone(), Secret::new("phrase"), yes())
        .is_ok());
    let e = m.connect_sftp(key, Secret::new("nope"), yes()).unwrap_err();
    assert!(matches!(e, ConnectError::Transport(TransportError::Auth)));
    assert!(sftp.log().iter().any(|l| l == "PUBLICKEY ***"));
}

// ----------------------------------------------------------------------- host key trust

fn key_of(seed: u8) -> HostKey {
    HostKey::ed25519(seed)
}

fn rsa_key(seed: u8, algorithm: &str) -> HostKey {
    HostKey {
        signature_algorithm: algorithm.into(),
        key_type: "ssh-rsa".into(),
        blob: vec![seed; 300],
    }
}

fn pin_key() -> PinKey {
    PinKey::new(Protocol::Sftp, "sftp.example.org", 22)
}

#[test]
fn trust_on_first_use_needs_confirmation_then_pins() {
    let sftp = FakeSftpServer::new();
    sftp.state().host_key = key_of(1);
    let ftp = FakeServer::new();
    let (m, router, pins) = manager(&sftp, &ftp, 4);
    // not confirmed: no connection, nothing registered or stored
    let reject = Ask::new(TrustDecision::Reject);
    let e = m
        .connect_sftp(profile(), Secret::new("secret"), reject.clone())
        .unwrap_err();
    match e.trust_error() {
        Some(TrustError::NotConfirmed(request)) => {
            assert_eq!(request.key, pin_key());
            assert_eq!(request.key_type.as_deref(), Some("ssh-ed25519"));
            assert_eq!(request.sha256.len(), 64);
            assert_eq!(request.reasons, vec![TrustReason::UnknownIssuer]);
        }
        other => panic!("{other:?}"),
    }
    assert!(router.remote_ids().is_empty() && pins.get(&pin_key()).is_none());
    assert!(
        !sftp.log().iter().any(|l| l.starts_with("USER")),
        "no login before trust"
    );
    // once: connects, nothing stored, asked again next time
    let once = Ask::new(TrustDecision::TrustOnce);
    let c = m
        .connect_sftp(profile(), Secret::new("secret"), once.clone())
        .unwrap();
    assert!(pins.get(&pin_key()).is_none());
    m.disconnect(&c.id);
    m.connect_sftp(profile(), Secret::new("secret"), once.clone())
        .unwrap();
    assert_eq!(once.asked(), 2);
    m.disconnect_all();
    // always: pinned with fingerprint and key type, never asked again
    let always = yes();
    let c = m
        .connect_sftp(profile(), Secret::new("secret"), always.clone())
        .unwrap();
    let pin = pins.get(&pin_key()).unwrap();
    assert_eq!(pin.protocol, Protocol::Sftp);
    assert_eq!(pin.key_type.as_deref(), Some("ssh-ed25519"));
    assert_eq!(pin.sha256, hex(&sha2::Sha256::digest(&key_of(1).blob)));
    m.disconnect(&c.id);
    let c = m
        .connect_sftp(
            profile(),
            Secret::new("secret"),
            Ask::new(TrustDecision::Reject),
        )
        .unwrap();
    m.disconnect(&c.id);
    assert_eq!(always.asked(), 1);
    // the same host on another port is another pin
    let mut other = profile();
    other.port = 2222;
    assert!(matches!(
        m.connect_sftp(
            other,
            Secret::new("secret"),
            Ask::new(TrustDecision::Reject)
        )
        .unwrap_err()
        .trust_error(),
        Some(TrustError::NotConfirmed(_))
    ));
}

use sha2::Digest;

#[test]
fn a_changed_host_key_is_refused_without_any_exception() {
    let sftp = FakeSftpServer::new();
    sftp.state().host_key = key_of(1);
    let ftp = FakeServer::new();
    let (m, router, pins) = manager(&sftp, &ftp, 4);
    let always = yes();
    let c = m
        .connect_sftp(profile(), Secret::new("secret"), always.clone())
        .unwrap();
    m.disconnect(&c.id);
    let expected = pins.get(&pin_key()).unwrap().sha256;
    // another key of the same type
    sftp.state().host_key = key_of(2);
    let e = m
        .connect_sftp(profile(), Secret::new("secret"), always.clone())
        .unwrap_err();
    match e.trust_error() {
        Some(TrustError::PinMismatch {
            expected: old,
            actual,
        }) => {
            assert_eq!(old, &expected);
            assert_ne!(actual, old);
        }
        other => panic!("{other:?}"),
    }
    // another key type
    sftp.state().host_key = rsa_key(1, "rsa-sha2-512");
    let e = m
        .connect_sftp(profile(), Secret::new("secret"), always.clone())
        .unwrap_err();
    assert!(
        matches!(e.trust_error(), Some(TrustError::PinMismatch { .. })),
        "{e}"
    );
    // nobody was asked again, nothing was logged in with, the pin is unchanged
    assert_eq!(always.asked(), 1);
    assert!(router.remote_ids().is_empty());
    assert_eq!(pins.get(&pin_key()).unwrap().sha256, expected);
    assert_eq!(
        sftp.log().iter().filter(|l| l.starts_with("USER")).count(),
        1
    );
    // only the explicit reset step makes room for the new key
    pins.remove(&pin_key());
    sftp.state().host_key = rsa_key(1, "rsa-sha2-512");
    assert!(m
        .connect_sftp(profile(), Secret::new("secret"), always.clone())
        .is_ok());
    assert_eq!(
        pins.get(&pin_key()).unwrap().key_type.as_deref(),
        Some("ssh-rsa")
    );
}

#[test]
fn a_key_change_during_a_reconnect_is_refused_too() {
    let sftp = FakeSftpServer::new();
    sftp.state().host_key = key_of(1);
    sftp.mkdirs("/pub");
    let ftp = FakeServer::new();
    let (m, _router, _pins) = manager(&sftp, &ftp, 4);
    let c = m
        .connect_sftp(profile(), Secret::new("secret"), yes())
        .unwrap();
    let vfs = m.router().vfs_for(&c.root).unwrap();
    assert!(vfs.list(&c.root).is_ok());
    sftp.state().host_key = key_of(9);
    sftp.disconnect_everybody();
    let e = vfs.list(&c.root).unwrap_err();
    assert!(e.to_string().contains("geändert"), "{e}");
}

#[test]
fn weak_host_key_algorithms_are_refused_even_when_confirmed() {
    let sftp = FakeSftpServer::new();
    let ftp = FakeServer::new();
    let (m, router, pins) = manager(&sftp, &ftp, 4);
    let always = yes();
    for (sig, ty) in [
        ("ssh-dss", "ssh-dss"),
        ("ssh-rsa", "ssh-rsa"),
        ("ssh-ed448", "ssh-ed448"),
        ("rsa-sha2-256-cert-v01@openssh.com", "ssh-rsa"),
    ] {
        sftp.state().host_key = HostKey {
            signature_algorithm: sig.into(),
            key_type: ty.into(),
            blob: vec![5; 80],
        };
        let e = m
            .connect_sftp(profile(), Secret::new("secret"), always.clone())
            .unwrap_err();
        assert!(
            matches!(e.trust_error(), Some(TrustError::AlgorithmNotAllowed(_))),
            "{sig}: {e}"
        );
    }
    assert_eq!(always.asked(), 0, "never even asked");
    assert!(pins.get(&pin_key()).is_none() && router.remote_ids().is_empty());
    // accepted ones
    for (sig, ty) in [
        ("ssh-ed25519", "ssh-ed25519"),
        ("ecdsa-sha2-nistp256", "ecdsa-sha2-nistp256"),
        ("ecdsa-sha2-nistp384", "ecdsa-sha2-nistp384"),
        ("ecdsa-sha2-nistp521", "ecdsa-sha2-nistp521"),
        ("rsa-sha2-256", "ssh-rsa"),
        ("rsa-sha2-512", "ssh-rsa"),
    ] {
        sftp.state().host_key = HostKey {
            signature_algorithm: sig.into(),
            key_type: ty.into(),
            blob: vec![6; 80],
        };
        let c = m
            .connect_sftp(
                profile(),
                Secret::new("secret"),
                Ask::new(TrustDecision::TrustOnce),
            )
            .unwrap_or_else(|e| panic!("{sig}: {e}"));
        m.disconnect(&c.id);
    }
}

// ----------------------------------------------------------------------- secrets

#[test]
fn passwords_and_passphrases_never_show_up_in_errors_debug_output_or_logs() {
    const MARKER: &str = "pw-MARKER-7f3a91";
    const PHRASE: &str = "phrase-MARKER-c4d2";
    let sftp = FakeSftpServer::new();
    sftp.mkdirs("/pub");
    sftp.state().password = MARKER.into();
    sftp.state().key_passphrase = Some(PHRASE.into());
    let ftp = FakeServer::new();
    let (m, _router, _pins) = manager(&sftp, &ftp, 4);
    let mut all = Vec::new();
    let mut key_profile = profile();
    key_profile.auth = SftpAuth::KeyFile {
        path: "/k/id".into(),
    };
    for (prof, secret) in [
        (profile(), Secret::new(MARKER)),
        (key_profile.clone(), Secret::new(PHRASE)),
    ] {
        // wrong login with the secret in hand
        sftp.state().password = "other".into();
        sftp.state().key_passphrase = Some("other".into());
        let e = m
            .connect_sftp(prof.clone(), secret.clone(), yes())
            .unwrap_err();
        all.push(e.to_string());
        all.push(format!("{e:?}"));
        sftp.state().password = MARKER.into();
        sftp.state().key_passphrase = Some(PHRASE.into());
        let info = m.connect_sftp(prof.clone(), secret.clone(), yes()).unwrap();
        all.push(format!("{info:?}"));
        all.push(format!("{m:?}"));
        all.push(format!("{prof:?}"));
        all.push(format!("{:?}", SftpConnectParams::new(prof.clone())));
        all.push(format!("{secret:?} {secret}"));
        sftp.state().echo_in_errors = Some(secret.expose().to_owned());
        let vfs = m.router().vfs_for(&info.root).unwrap();
        for (status, text) in [
            (SftpStatus::Failure, "boom"),
            (SftpStatus::PermissionDenied, "denied"),
            (SftpStatus::NoSuchFile, "gone"),
            (SftpStatus::ConnectionLost, "bye"),
            (SftpStatus::BadMessage, "bad"),
            (SftpStatus::OpUnsupported, "no"),
        ] {
            for step in 0..4 {
                sftp.state().fail_next = Some((status, text.into()));
                let result = match step {
                    0 => vfs.list(&info.root).map(|_| ()),
                    1 => vfs.mkdir(&VPath::sftp(&info.id, "/pub/x").unwrap()),
                    2 => vfs
                        .open_read(&VPath::sftp(&info.id, "/pub/y").unwrap())
                        .map(|_| ()),
                    _ => vfs
                        .create_write(
                            &VPath::sftp(&info.id, "/pub/z").unwrap(),
                            WriteMode::NoReplace,
                        )
                        .map(|_| ()),
                };
                if let Err(e) = result {
                    all.push(e.to_string());
                    all.push(format!("{e:?}"));
                }
            }
        }
        sftp.state().fail_next = None;
        sftp.state().echo_in_errors = None;
        // the connection is refused later on (reconnect failure text)
        sftp.disconnect_everybody();
        sftp.state().refuse_connects_after = Some(0);
        all.push(format!("{:?}", vfs.list(&info.root)));
        sftp.state().refuse_connects_after = None;
        m.disconnect(&info.id);
    }
    // a hostile server that echoes the secret in the status of a failed request
    for text in &all {
        assert!(
            !text.contains(MARKER) && !text.contains(PHRASE),
            "leak in: {text}"
        );
    }
    assert!(!all.is_empty());
    assert!(
        !sftp.log().iter().any(|l| l.contains("MARKER")),
        "{:?}",
        sftp.log()
    );
}

#[test]
fn an_error_that_echoes_the_secret_is_scrubbed() {
    // a library error text is built from what the server said: the vfs scrubs the secret
    let (server, vfs) = setup();
    server.mkdirs("/d");
    server.state().echo_in_errors = Some("secret".into());
    server.state().fail_next = Some((SftpStatus::Failure, "nope".into()));
    let e = vfs.mkdir(&p("/d/x")).unwrap_err();
    assert!(!e.to_string().contains("secret"), "{e}");
    server.state().echo_in_errors = None;
    // and the transport error type scrubs on request
    let status = TransportError::status(SftpStatus::Failure, "x secret y");
    let scrubbed = status.scrubbed(&Secret::new("secret"));
    assert!(!scrubbed.to_string().contains("secret"), "{scrubbed}");
}
