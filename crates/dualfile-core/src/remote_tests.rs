//! Tests for the server-facing parts of the core: location format, router registration,
//! resumable copies, cross-back-end moves and delete plans, all against an in-memory fake
//! server back end (no network).

use crate::error::{Error, Result};
use crate::ops::{self, Conflict, ConflictResolver, JobHandle, Resolution};
use crate::resume::{ResumableWrite, SourceIdentity};
use crate::router::VfsRouter;
use crate::vfs::{
    format_ipc_path, parse_ipc_path, Caps, Entry, EntryKind, ReplaceMode, Scheme, VPath, Vfs,
    WriteFinish, WriteMode,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
enum Node {
    Dir,
    File(Vec<u8>, SystemTime),
}

#[derive(Default)]
struct State {
    nodes: BTreeMap<String, Node>,
    /// path -> (source identity, bytes) of an unfinished resumable upload
    parts: HashMap<String, (SourceIdentity, Vec<u8>)>,
    /// abort writers after this many bytes (simulates a lost connection)
    fail_after: Option<usize>,
    /// remember what happened
    resumed_from: Vec<u64>,
    /// pretend the stored file is one byte shorter than what was written
    truncate_on_commit: bool,
    /// a file that "another client" creates right before the next commit
    inject_before_commit: Option<(String, Vec<u8>)>,
}

/// A fake server connection kept in memory.
pub struct MemServer {
    scheme: Scheme,
    id: String,
    state: Arc<Mutex<State>>,
}

impl MemServer {
    pub fn new(scheme: Scheme, id: &str) -> Arc<Self> {
        let mut state = State::default();
        state.nodes.insert("/".into(), Node::Dir);
        Arc::new(Self {
            scheme,
            id: id.into(),
            state: Arc::new(Mutex::new(state)),
        })
    }
    pub fn put(&self, path: &str, data: &[u8], secs: u64) {
        self.state.lock().unwrap().nodes.insert(
            path.into(),
            Node::File(data.to_vec(), UNIX_EPOCH + Duration::from_secs(secs)),
        );
    }
    pub fn dir(&self, path: &str) {
        self.state
            .lock()
            .unwrap()
            .nodes
            .insert(path.into(), Node::Dir);
    }
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self.state.lock().unwrap().nodes.get(path) {
            Some(Node::File(data, _)) => Some(data.clone()),
            _ => None,
        }
    }
    fn has(&self, path: &str) -> bool {
        self.state.lock().unwrap().nodes.contains_key(path)
    }
    pub fn vp(&self, path: &str) -> VPath {
        match self.scheme {
            Scheme::Plugin => VPath::plugin(&self.id, path).unwrap(),
            Scheme::Sftp => VPath::sftp(&self.id, path).unwrap(),
            _ => VPath::ftp(&self.id, path).unwrap(),
        }
    }
    fn entry(name: &str, node: &Node) -> Entry {
        let (kind, size, modified) = match node {
            Node::Dir => (EntryKind::Dir, 0, None),
            Node::File(data, time) => (EntryKind::File, data.len() as u64, Some(*time)),
        };
        Entry {
            name: name.into(),
            kind,
            size,
            modified,
            mode: None,
            hidden: false,
            link_target: None,
            broken_link: false,
            error: None,
        }
    }
}

struct MemWrite {
    state: Arc<Mutex<State>>,
    target: String,
    mode: WriteMode,
    data: Vec<u8>,
    /// resumable: write straight into `parts`
    part_key: Option<String>,
    done: bool,
}

impl Write for MemWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        if let Some(limit) = state.fail_after {
            let have = match &self.part_key {
                Some(key) => state.parts[key].1.len(),
                None => self.data.len(),
            };
            if have >= limit {
                return Err(std::io::Error::other("connection lost"));
            }
            let room = limit - have;
            let take = buf.len().min(room);
            match &self.part_key {
                Some(key) => state
                    .parts
                    .get_mut(key)
                    .unwrap()
                    .1
                    .extend_from_slice(&buf[..take]),
                None => self.data.extend_from_slice(&buf[..take]),
            }
            return Ok(take.max(1).min(buf.len()));
        }
        match &self.part_key {
            Some(key) => state.parts.get_mut(key).unwrap().1.extend_from_slice(buf),
            None => self.data.extend_from_slice(buf),
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl WriteFinish for MemWrite {
    fn commit(&mut self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some((path, data)) = state.inject_before_commit.take() {
            state
                .nodes
                .insert(path, Node::File(data, SystemTime::now()));
        }
        let mut data = match &self.part_key {
            Some(key) => state.parts[key].1.clone(),
            None => std::mem::take(&mut self.data),
        };
        if state.truncate_on_commit {
            data.pop();
        }
        if self.mode == WriteMode::NoReplace && state.nodes.contains_key(&self.target) {
            return Err(Error::Exists(self.target.clone().into()));
        }
        state
            .nodes
            .insert(self.target.clone(), Node::File(data, SystemTime::now()));
        if let Some(key) = &self.part_key {
            state.parts.remove(key);
        }
        self.done = true;
        Ok(())
    }
}

impl Vfs for MemServer {
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        let state = self.state.lock().unwrap();
        let base = p.path().to_string_lossy().into_owned();
        let prefix = if base == "/" {
            "/".to_owned()
        } else {
            format!("{base}/")
        };
        match state.nodes.get(&base) {
            None => return Err(Error::NotFound(p.path().into())),
            Some(Node::File(..)) => return Err(Error::NotADirectory(p.path().into())),
            Some(Node::Dir) => {}
        }
        Ok(state
            .nodes
            .iter()
            .filter_map(|(path, node)| {
                let rest = path.strip_prefix(&prefix)?;
                (!rest.is_empty() && !rest.contains('/')).then(|| Self::entry(rest, node))
            })
            .collect())
    }
    fn stat(&self, p: &VPath) -> Result<Entry> {
        let state = self.state.lock().unwrap();
        let key = p.path().to_string_lossy().into_owned();
        let node = state
            .nodes
            .get(&key)
            .ok_or_else(|| Error::NotFound(p.path().into()))?;
        Ok(Self::entry(
            p.path().file_name().and_then(|n| n.to_str()).unwrap_or("/"),
            node,
        ))
    }
    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        match self
            .state
            .lock()
            .unwrap()
            .nodes
            .get(&*p.path().to_string_lossy())
        {
            Some(Node::File(data, _)) => Ok(Box::new(Cursor::new(data.clone()))),
            Some(Node::Dir) => Err(Error::IsADirectory(p.path().into())),
            None => Err(Error::NotFound(p.path().into())),
        }
    }
    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        Ok(Box::new(MemWrite {
            state: self.state.clone(),
            target: p.path().to_string_lossy().into_owned(),
            mode,
            data: Vec::new(),
            part_key: None,
            done: false,
        }))
    }
    fn create_write_resumable(
        &self,
        p: &VPath,
        expected_source: SourceIdentity,
    ) -> Result<ResumableWrite> {
        let key = format!("{}.dualfile-part", p.path().display());
        let mut state = self.state.lock().unwrap();
        let (existing, matches) = match state.parts.get(&key) {
            Some((identity, data)) if *identity == expected_source => (data.len() as u64, true),
            _ => (0, false),
        };
        if !matches {
            state
                .parts
                .insert(key.clone(), (expected_source, Vec::new()));
        }
        state.resumed_from.push(existing);
        drop(state);
        Ok(ResumableWrite {
            existing_len: existing,
            identity_matches: matches,
            writer: Box::new(MemWrite {
                state: self.state.clone(),
                target: p.path().to_string_lossy().into_owned(),
                mode: WriteMode::NoReplace,
                data: Vec::new(),
                part_key: Some(key),
                done: false,
            }),
        })
    }
    fn mkdir(&self, p: &VPath) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let key = p.path().to_string_lossy().into_owned();
        if state.nodes.contains_key(&key) {
            return Err(Error::Exists(p.path().into()));
        }
        state.nodes.insert(key, Node::Dir);
        Ok(())
    }
    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let (a, b) = (
            from.path().to_string_lossy().into_owned(),
            to.path().to_string_lossy().into_owned(),
        );
        if mode == ReplaceMode::NoReplace && state.nodes.contains_key(&b) {
            return Err(Error::Exists(to.path().into()));
        }
        let node = state
            .nodes
            .remove(&a)
            .ok_or_else(|| Error::NotFound(from.path().into()))?;
        state.nodes.insert(b, node);
        Ok(())
    }
    fn read_link(&self, _: &VPath) -> Result<String> {
        Err(Error::Unsupported("Links"))
    }
    fn create_symlink(&self, _: &str, _: &VPath) -> Result<()> {
        Err(Error::Unsupported("Links"))
    }
    fn trash(&self, _: &VPath) -> Result<()> {
        Err(Error::Unsupported("Papierkorb"))
    }
    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let key = p.path().to_string_lossy().into_owned();
        let prefix = format!("{key}/");
        if state.nodes.keys().any(|other| other.starts_with(&prefix)) {
            return Err(Error::DirectoryNotEmpty(p.path().into()));
        }
        state
            .nodes
            .remove(&key)
            .map(|_| ())
            .ok_or_else(|| Error::NotFound(p.path().into()))
    }
    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: false,
            can_rename_in_place: true,
        }
    }
    fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
        Some((self.scheme, &self.id))
    }
}

fn local_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn lp(path: &std::path::Path) -> VPath {
    VPath::local(path).unwrap()
}

struct Choose(Resolution);
impl ConflictResolver for Choose {
    fn resolve(&self, _: &Conflict) -> Resolution {
        self.0
    }
}

// ---------------------------------------------------------------------------- location format

#[test]
fn server_locations_roundtrip_through_the_ipc_format() {
    let cases = [
        (
            VPath::ftp("srv-1", "/a/b c/d.txt").unwrap(),
            "ftp:srv-1!/a/b c/d.txt",
        ),
        (VPath::sftp("x", "/").unwrap(), "sftp:x!/"),
        // `!` and `%` in names are taken literally; the first `!` ends the id
        (
            VPath::ftp("a1", "/we!rd/100%25").unwrap(),
            "ftp:a1!/we!rd/100%25",
        ),
    ];
    for (path, text) in cases {
        assert_eq!(format_ipc_path(&path), text);
        let parsed = parse_ipc_path(text).unwrap();
        assert_eq!(parsed, path);
        assert_eq!(parsed.conn_id(), path.conn_id());
        assert!(parsed.scheme().is_remote());
        assert!(parsed.outer().is_none());
    }
    assert_eq!(
        VPath::ftp("a", "//x///y/").unwrap().path().to_str(),
        Some("/x/y")
    );
}

#[test]
fn forged_server_locations_are_rejected() {
    let too_long_id = "a".repeat(33);
    let long_name = format!("ftp:a!/{}", "n".repeat(256));
    let deep = format!("ftp:a!{}", "/d".repeat(129));
    let long_path = format!("ftp:a!/{}", vec!["n".repeat(200); 25].join("/"));
    let bad = [
        "ftp:",
        "ftp:abc",
        "ftp:!/x",
        "ftp:ABC!/x",
        "ftp:a_b!/x",
        "ftp:a b!/x",
        "ftp:ab%21!/x",
        "ftp:a\0b!/x",
        "ftp:a!x",
        "ftp:a!",
        "ftp:a!/a/../b",
        "ftp:a!/./b",
        "ftp:a!/a\0b",
        "ftp:a!/a\\b",
        "ftp:a!/a\nRETR /etc/passwd",
        "ftp:a!/a\rb",
        "ftp:a!/a\x1bb",
        "ftp:a!/a\x7fb",
        "sftp:a!/../x",
        "sftp:A!/x",
        &too_long_id_path(&too_long_id),
        &long_name,
        &deep,
        &long_path,
        // an archive cannot live on a server
        "archive:ftp:a!%21/x.zip!/inner",
    ];
    for text in bad {
        assert!(parse_ipc_path(text).is_err(), "must be rejected: {text:?}");
    }
    assert!(VPath::ftp(&"a".repeat(32), "/x").is_ok());
    assert!(VPath::ftp("", "/x").is_err());
    assert!(VPath::archive(VPath::ftp("a", "/x.zip").unwrap(), "/y").is_err());
    // joining a name with a separator or control character fails, too
    let base = VPath::ftp("a", "/").unwrap();
    assert!(base.join("a/b").is_err());
    assert!(base.join("a\nb").is_err());
    assert!(base.join("..").is_err());
    assert_eq!(base.join("ok").unwrap().conn_id(), Some("a"));
    assert_eq!(base.join("ok").unwrap().parent().unwrap(), base);
    assert_eq!(
        base.join("ok")
            .unwrap()
            .sibling("zz")
            .unwrap()
            .path()
            .to_str(),
        Some("/zz")
    );
}

fn too_long_id_path(id: &str) -> String {
    format!("ftp:{id}!/x")
}

#[test]
fn server_and_local_locations_never_compare_equal() {
    let remote = VPath::ftp("a", "/tmp").unwrap();
    let other = VPath::ftp("b", "/tmp").unwrap();
    let sftp = VPath::sftp("a", "/tmp").unwrap();
    let local = VPath::local("/tmp").unwrap();
    assert_ne!(remote, local);
    assert_ne!(remote, other);
    assert_ne!(remote, sftp);
}

// ---------------------------------------------------------------------------- router

#[test]
fn router_dispatches_by_scheme_and_connection_and_reports_disconnected() {
    let router = VfsRouter::new();
    let one = MemServer::new(Scheme::Ftp, "one");
    let two = MemServer::new(Scheme::Sftp, "two");
    router.register_remote("one", one.clone()).unwrap();
    router.register_remote("two", two.clone()).unwrap();
    assert_eq!(router.remote_ids(), ["one", "two"]);
    let a = router.vfs_for(&VPath::ftp("one", "/x").unwrap()).unwrap();
    assert_eq!(a.remote_endpoint(), Some((Scheme::Ftp, "one")));
    assert!(!a.is_local());
    let b = router.vfs_for(&VPath::sftp("two", "/x").unwrap()).unwrap();
    assert_eq!(b.remote_endpoint(), Some((Scheme::Sftp, "two")));
    // wrong scheme for an id, unknown id: Disconnected, never another back end
    assert!(matches!(
        router.vfs_for(&VPath::sftp("one", "/x").unwrap()),
        Err(Error::Disconnected)
    ));
    assert!(matches!(
        router.vfs_for(&VPath::ftp("nope", "/x").unwrap()),
        Err(Error::Disconnected)
    ));
    // local paths are unaffected
    assert!(router
        .vfs_for(&VPath::local("/tmp").unwrap())
        .unwrap()
        .is_local());
    // after unregistering, the location is disconnected at once
    assert!(router.unregister("one").is_some());
    assert!(router.unregister("one").is_none());
    assert!(matches!(
        router.vfs_for(&VPath::ftp("one", "/x").unwrap()),
        Err(Error::Disconnected)
    ));
    assert!(router.vfs_for(&VPath::sftp("two", "/x").unwrap()).is_ok());
}

#[test]
fn router_refuses_bad_or_duplicate_registrations() {
    let router = VfsRouter::new();
    let server = MemServer::new(Scheme::Ftp, "abc");
    assert!(router.register_remote("ABC", server.clone()).is_err());
    assert!(router.register_remote("", server.clone()).is_err());
    // the back end must serve exactly the id it is registered under
    assert!(router.register_remote("other", server.clone()).is_err());
    // the local file system is no server back end
    assert!(router
        .register_remote("abc", Arc::new(crate::vfs::LocalVfs))
        .is_err());
    router.register_remote("abc", server.clone()).unwrap();
    assert!(router
        .register_remote("abc", MemServer::new(Scheme::Ftp, "abc"))
        .is_err());
    // the original is still in place
    let vfs = router.vfs_for(&VPath::ftp("abc", "/").unwrap()).unwrap();
    assert!(Arc::ptr_eq(&vfs, &(server as Arc<dyn Vfs>)));
}

#[test]
fn open_path_without_registered_connections_is_disconnected() {
    assert!(matches!(
        crate::open_path(&VPath::ftp("never-registered", "/").unwrap()),
        Err(Error::Disconnected)
    ));
}

// ---------------------------------------------------------------------------- copy / resume

fn copy_one(
    sv: &dyn Vfs,
    src: &VPath,
    dv: &dyn Vfs,
    dst_dir: &VPath,
    resolver: Option<&dyn ConflictResolver>,
) -> ops::OpReport {
    ops::copy(
        sv,
        std::slice::from_ref(src),
        dv,
        dst_dir,
        resolver,
        &JobHandle::silent(),
    )
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[test]
fn upload_and_download_roundtrip_with_size_check() {
    let tmp = local_dir();
    let data = payload(3_000_000);
    std::fs::write(tmp.path().join("f.bin"), &data).unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.dir("/in");
    let report = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&tmp.path().join("f.bin")),
        &*server,
        &server.vp("/in"),
        None,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/in/f.bin").unwrap(), data);
    assert!(server.state.lock().unwrap().parts.is_empty());
    // and back down into a new local folder
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let report = copy_one(
        &*server,
        &server.vp("/in/f.bin"),
        &crate::vfs::LocalVfs,
        &lp(&out),
        None,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(std::fs::read(out.join("f.bin")).unwrap(), data);
}

#[test]
fn aborted_upload_is_continued_from_the_partial_file() {
    let tmp = local_dir();
    let data = payload(2_500_000);
    let file = tmp.path().join("big.bin");
    std::fs::write(&file, &data).unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.dir("/in");
    server.state.lock().unwrap().fail_after = Some(1_000_000);
    let first = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &server.vp("/in"),
        None,
    );
    assert_eq!(first.failed.len(), 1, "the lost connection is reported");
    assert!(
        !server.has("/in/big.bin"),
        "no half file under the final name"
    );
    assert_eq!(
        server.state.lock().unwrap().parts.len(),
        1,
        "partial file kept"
    );
    // second attempt: the connection works again and only the rest is sent
    server.state.lock().unwrap().fail_after = None;
    let second = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &server.vp("/in"),
        None,
    );
    assert!(second.failed.is_empty(), "{:?}", second.failed);
    assert_eq!(server.get("/in/big.bin").unwrap(), data);
    assert_eq!(server.state.lock().unwrap().resumed_from, [0, 1_000_000]);
    assert!(server.state.lock().unwrap().parts.is_empty());
}

#[test]
fn cancelled_upload_keeps_the_partial_file_and_continues_later() {
    let tmp = local_dir();
    let data = payload(4_000_000);
    let file = tmp.path().join("c.bin");
    std::fs::write(&file, &data).unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    let holder: Arc<Mutex<Option<JobHandle>>> = Arc::new(Mutex::new(None));
    struct Late(Arc<Mutex<Option<JobHandle>>>, std::sync::atomic::AtomicU32);
    impl ops::ProgressSink for Late {
        fn event(&self, event: ops::ProgressEvent) {
            if matches!(event, ops::ProgressEvent::BytesCopied { .. })
                && self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
            {
                if let Some(job) = self.0.lock().unwrap().as_ref() {
                    job.cancel();
                }
            }
        }
    }
    let job = JobHandle::new(Arc::new(Late(holder.clone(), Default::default())));
    *holder.lock().unwrap() = Some(job.clone());
    let report = ops::copy(
        &crate::vfs::LocalVfs,
        &[lp(&file)],
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
        &job,
    );
    assert!(report.cancelled);
    assert!(!server.has("/c.bin"));
    let kept = server
        .state
        .lock()
        .unwrap()
        .parts
        .values()
        .next()
        .unwrap()
        .1
        .len();
    assert!(kept > 0 && kept < data.len(), "partial data kept: {kept}");
    let report = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/c.bin").unwrap(), data);
    assert_eq!(server.state.lock().unwrap().resumed_from[1], kept as u64);
}

#[test]
fn changed_source_starts_over_instead_of_continuing() {
    let tmp = local_dir();
    let file = tmp.path().join("v.bin");
    std::fs::write(&file, payload(2_200_000)).unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.state.lock().unwrap().fail_after = Some(1_100_000);
    let first = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
    );
    assert_eq!(first.failed.len(), 1);
    // the source changes (other size): a new identity
    let changed: Vec<u8> = payload(2_300_000)
        .iter()
        .map(|b| b.wrapping_add(1))
        .collect();
    std::fs::write(&file, &changed).unwrap();
    server.state.lock().unwrap().fail_after = None;
    let second = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
    );
    assert!(second.failed.is_empty(), "{:?}", second.failed);
    assert_eq!(server.get("/v.bin").unwrap(), changed);
    assert_eq!(server.state.lock().unwrap().resumed_from, [0, 0]);
}

#[test]
fn existing_target_is_never_replaced_without_a_decision() {
    let tmp = local_dir();
    let file = tmp.path().join("e.txt");
    std::fs::write(&file, b"new").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.put("/e.txt", b"precious", 5);
    let root = VPath::ftp("s", "/").unwrap();
    // no resolver: skipped
    let report = copy_one(&crate::vfs::LocalVfs, &lp(&file), &*server, &root, None);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(server.get("/e.txt").unwrap(), b"precious");
    // keep both: a new name on the server, original untouched
    let report = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &root,
        Some(&Choose(Resolution::KeepBoth)),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/e.txt").unwrap(), b"precious");
    assert_eq!(server.get("/e (2).txt").unwrap(), b"new");
    // overwrite only on explicit decision
    let report = copy_one(
        &crate::vfs::LocalVfs,
        &lp(&file),
        &*server,
        &root,
        Some(&Choose(Resolution::Overwrite)),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/e.txt").unwrap(), b"new");
}

#[test]
fn a_file_that_appears_during_the_upload_wins_over_the_new_copy() {
    // another client creates the target between the check and the final rename: the
    // no-replace commit refuses, the late file survives and nothing is overwritten
    let tmp = local_dir();
    let file = tmp.path().join("race.txt");
    std::fs::write(&file, b"mine").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.state.lock().unwrap().inject_before_commit =
        Some(("/race.txt".into(), b"theirs".to_vec()));
    let root = VPath::ftp("s", "/").unwrap();
    let report = copy_one(&crate::vfs::LocalVfs, &lp(&file), &*server, &root, None);
    assert_eq!(report.skipped.len(), 1, "{:?}", report.failed);
    assert_eq!(server.get("/race.txt").unwrap(), b"theirs");
}

#[test]
fn source_size_mismatch_is_not_committed() {
    // a server source that delivers fewer bytes than its listing promised
    struct Lying(Arc<MemServer>);
    impl Vfs for Lying {
        fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
            self.0.list(p)
        }
        fn stat(&self, p: &VPath) -> Result<Entry> {
            let mut entry = self.0.stat(p)?;
            if entry.kind == EntryKind::File {
                entry.size += 10;
            }
            Ok(entry)
        }
        fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
            self.0.open_read(p)
        }
        fn create_write(&self, p: &VPath, m: WriteMode) -> Result<Box<dyn WriteFinish>> {
            self.0.create_write(p, m)
        }
        fn mkdir(&self, p: &VPath) -> Result<()> {
            self.0.mkdir(p)
        }
        fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
            self.0.rename(a, b, m)
        }
        fn read_link(&self, p: &VPath) -> Result<String> {
            self.0.read_link(p)
        }
        fn create_symlink(&self, t: &str, p: &VPath) -> Result<()> {
            self.0.create_symlink(t, p)
        }
        fn trash(&self, p: &VPath) -> Result<()> {
            self.0.trash(p)
        }
        fn remove_permanent(&self, p: &VPath) -> Result<()> {
            self.0.remove_permanent(p)
        }
        fn capabilities(&self) -> Caps {
            self.0.capabilities()
        }
        fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
            self.0.remote_endpoint()
        }
    }
    let server = MemServer::new(Scheme::Ftp, "s");
    server.put("/f.txt", b"0123456789", 1);
    let lying = Lying(server.clone());
    let tmp = local_dir();
    let report = copy_one(
        &lying,
        &server.vp("/f.txt"),
        &crate::vfs::LocalVfs,
        &lp(tmp.path()),
        None,
    );
    assert_eq!(report.failed.len(), 1);
    assert!(matches!(report.failed[0].1, Error::SizeMismatch(_)));
    assert!(
        !tmp.path().join("f.txt").exists(),
        "nothing visible under the final name"
    );
    // and a move never removes the source in that case
    let moved = ops::move_(
        &lying,
        &[server.vp("/f.txt")],
        &crate::vfs::LocalVfs,
        &lp(tmp.path()),
        None,
        &JobHandle::silent(),
    );
    assert_eq!(moved.failed.len(), 1);
    assert!(server.has("/f.txt"));
}

#[test]
fn written_size_that_differs_after_commit_is_an_error_and_keeps_the_source() {
    let tmp = local_dir();
    let file = tmp.path().join("m.txt");
    std::fs::write(&file, b"abcdef").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.state.lock().unwrap().truncate_on_commit = true;
    let moved = ops::move_(
        &crate::vfs::LocalVfs,
        &[lp(&file)],
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
        &JobHandle::silent(),
    );
    assert_eq!(moved.failed.len(), 1);
    assert!(matches!(moved.failed[0].1, Error::SizeMismatch(_)));
    assert!(file.exists(), "source stays when the check fails");
    assert!(moved.done.is_empty());
}

// ---------------------------------------------------------------------------- move

#[test]
fn move_between_back_ends_copies_checks_and_removes_the_source() {
    let tmp = local_dir();
    let tree = tmp.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("a.txt"), b"aaa").unwrap();
    std::fs::write(tree.join("sub/b.txt"), b"bbbb").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    let report = ops::move_(
        &crate::vfs::LocalVfs,
        &[lp(&tree)],
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/tree/a.txt").unwrap(), b"aaa");
    assert_eq!(server.get("/tree/sub/b.txt").unwrap(), b"bbbb");
    assert!(!tree.exists(), "source removed after a verified copy");
}

#[test]
fn move_with_a_partial_failure_leaves_the_source_in_place() {
    let tmp = local_dir();
    let tree = tmp.path().join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("a.txt"), payload(10)).unwrap();
    std::fs::write(tree.join("big.bin"), payload(2_000_000)).unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    server.state.lock().unwrap().fail_after = Some(5);
    let report = ops::move_(
        &crate::vfs::LocalVfs,
        &[lp(&tree)],
        &*server,
        &VPath::ftp("s", "/").unwrap(),
        None,
        &JobHandle::silent(),
    );
    assert!(!report.failed.is_empty());
    assert!(
        tree.join("big.bin").exists(),
        "the failed file is still at the source"
    );
    assert!(
        tree.exists(),
        "the folder is not removed after a partial copy"
    );
}

#[test]
fn move_on_one_connection_renames_without_replacing() {
    let server = MemServer::new(Scheme::Ftp, "s");
    server.dir("/a");
    server.dir("/b");
    server.put("/a/f.txt", b"one", 1);
    server.put("/b/f.txt", b"two", 1);
    server.put("/a/g.txt", b"g", 1);
    let report = ops::move_(
        &*server,
        &[server.vp("/a/g.txt")],
        &*server,
        &server.vp("/b"),
        None,
        &JobHandle::silent(),
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(server.get("/b/g.txt").unwrap(), b"g");
    assert!(!server.has("/a/g.txt"));
    // a clash is handed to the conflict rules, nothing is overwritten
    let report = ops::move_(
        &*server,
        &[server.vp("/a/f.txt")],
        &*server,
        &server.vp("/b"),
        None,
        &JobHandle::silent(),
    );
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(server.get("/b/f.txt").unwrap(), b"two");
    assert_eq!(server.get("/a/f.txt").unwrap(), b"one");
}

#[test]
fn copying_a_folder_into_itself_on_one_connection_is_refused() {
    let server = MemServer::new(Scheme::Ftp, "s");
    server.dir("/a");
    server.dir("/a/sub");
    let report = copy_one(
        &*server,
        &server.vp("/a"),
        &*server,
        &server.vp("/a/sub"),
        None,
    );
    assert!(matches!(report.failed[0].1, Error::DestinationInsideSource));
    let report = copy_one(&*server, &server.vp("/a"), &*server, &server.vp("/"), None);
    assert!(matches!(report.failed[0].1, Error::SameFile));
}

// ---------------------------------------------------------------------------- delete plans

#[test]
fn a_delete_plan_only_runs_on_the_connection_it_was_made_for() {
    let one = MemServer::new(Scheme::Ftp, "one");
    let two = MemServer::new(Scheme::Ftp, "two");
    for server in [&one, &two] {
        server.dir("/d");
        server.put("/d/f.txt", b"x", 1);
    }
    let plan = ops::plan_delete(&*one, &[one.vp("/d")]);
    assert!(!plan.incomplete);
    assert_eq!(plan.files, 1);
    // another connection with identical paths: refused, nothing deleted anywhere
    let report = ops::delete_permanent(&*two, &plan, plan.token, &JobHandle::silent());
    assert_eq!(report.failed.len(), 1);
    assert!(report.done.is_empty());
    assert!(two.has("/d/f.txt") && one.has("/d/f.txt"));
    // the local file system refuses it as well
    let report = ops::delete_permanent(
        &crate::vfs::LocalVfs,
        &plan,
        plan.token,
        &JobHandle::silent(),
    );
    assert!(report.done.is_empty() && !report.failed.is_empty());
    // a local plan is no good for a server either
    let tmp = local_dir();
    std::fs::write(tmp.path().join("x"), b"x").unwrap();
    let local_plan = ops::plan_delete(&crate::vfs::LocalVfs, &[lp(&tmp.path().join("x"))]);
    let report = ops::delete_permanent(&*one, &local_plan, local_plan.token, &JobHandle::silent());
    assert!(report.done.is_empty() && !report.failed.is_empty());
    assert!(tmp.path().join("x").exists());
    // wrong token still fails
    let report = ops::delete_permanent(&*one, &plan, plan.token ^ 1, &JobHandle::silent());
    assert!(report.done.is_empty());
    // on its own connection it works
    let report = ops::delete_permanent(&*one, &plan, plan.token, &JobHandle::silent());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!one.has("/d") && two.has("/d/f.txt"));
}

#[test]
fn a_plan_with_items_of_another_connection_is_incomplete() {
    let one = MemServer::new(Scheme::Ftp, "one");
    let two = MemServer::new(Scheme::Ftp, "two");
    one.put("/f", b"x", 1);
    two.put("/f", b"x", 1);
    let plan = ops::plan_delete(&*one, &[two.vp("/f")]);
    assert!(plan.incomplete);
    let report = ops::delete_permanent(&*one, &plan, plan.token, &JobHandle::silent());
    assert!(report.done.is_empty());
    assert!(one.has("/f") && two.has("/f"));
}

#[test]
fn server_delete_plan_never_touches_a_local_file_with_the_same_path() {
    // the server path /<tmp>/x equals a real local path: the plan must use the server entry
    let tmp = local_dir();
    let local_file = tmp.path().join("same.txt");
    std::fs::write(&local_file, b"local").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    let server_path = local_file.to_string_lossy().into_owned();
    server.put(&server_path, b"remote", 1);
    let plan = ops::plan_delete(&*server, &[server.vp(&server_path)]);
    assert!(!plan.incomplete);
    let report = ops::delete_permanent(&*server, &plan, plan.token, &JobHandle::silent());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!server.has(&server_path));
    assert_eq!(std::fs::read(&local_file).unwrap(), b"local");
}

#[test]
fn server_entry_changed_after_the_plan_is_not_deleted() {
    let server = MemServer::new(Scheme::Ftp, "s");
    server.put("/f.txt", b"abc", 1);
    let plan = ops::plan_delete(&*server, &[server.vp("/f.txt")]);
    server.put("/f.txt", b"abcdef", 2);
    let report = ops::delete_permanent(&*server, &plan, plan.token, &JobHandle::silent());
    assert_eq!(report.failed.len(), 1);
    assert!(matches!(report.failed[0].1, Error::ChangedSincePlan(_)));
    assert!(server.has("/f.txt"));
}

#[test]
fn trash_on_a_server_is_unsupported_and_never_deletes() {
    let server = MemServer::new(Scheme::Ftp, "s");
    server.put("/f.txt", b"abc", 1);
    let report = ops::trash_items(&*server, &[server.vp("/f.txt")], &JobHandle::silent());
    assert_eq!(report.failed.len(), 1);
    assert!(matches!(report.failed[0].1, Error::Unsupported(_)));
    assert!(server.has("/f.txt"));
    assert!(ops::restore_from_trash(&[server.vp("/f.txt")]).is_err());
}

// ---------------------------------------------------------------------------- rename / mkdir

#[test]
fn rename_and_mkdir_work_on_server_locations() {
    let server = MemServer::new(Scheme::Ftp, "s");
    server.dir("/d");
    server.put("/d/a.txt", b"a", 1);
    let result = ops::rename_entry(&*server, &server.vp("/d/a.txt"), "b.txt").unwrap();
    assert_eq!(result.new, server.vp("/d/b.txt"));
    assert!(server.has("/d/b.txt") && !server.has("/d/a.txt"));
    let made = ops::make_dir(&*server, &server.vp("/d"), "sub").unwrap();
    assert_eq!(made, server.vp("/d/sub"));
    assert!(server.has("/d/sub"));
    assert!(ops::rename_entry(&*server, &server.vp("/d/b.txt"), "../x").is_err());
}

// ---------------------------------------------------------------------------- sync

#[test]
fn sync_onto_a_server_is_refused_with_a_clear_message() {
    use crate::compare::{compare_dirs, plan_sync, CompareOptions, SyncMode, SyncOptions};
    let tmp = local_dir();
    std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    let server = MemServer::new(Scheme::Ftp, "s");
    let left = lp(tmp.path());
    let right = server.vp("/");
    let result = compare_dirs(
        &crate::vfs::LocalVfs,
        &left,
        &*server,
        &right,
        &CompareOptions::default(),
        &JobHandle::silent(),
    );
    let err = plan_sync(&result, SyncMode::LeftToRight, &SyncOptions::default()).unwrap_err();
    assert!(
        matches!(err, Error::Unsupported(message) if message == crate::compare::MSG_REMOTE_TARGET),
        "{err:?}"
    );
    assert!(
        server.state.lock().unwrap().nodes.len() == 1,
        "the server is untouched"
    );
}

#[test]
fn schemes_know_whether_they_are_remote() {
    assert!(Scheme::Ftp.is_remote() && Scheme::Sftp.is_remote() && Scheme::Plugin.is_remote());
    assert!(!Scheme::Local.is_remote() && !Scheme::Archive.is_remote());
    assert!(Scheme::Ftp.is_server() && Scheme::Sftp.is_server());
    assert!(!Scheme::Plugin.is_server() && !Scheme::Local.is_server());
}

// ---------------------------------------------------------------------------- plugin locations

#[test]
fn plugin_locations_roundtrip_through_the_ipc_format() {
    let cases = [
        (
            VPath::plugin("example", "/a/b c/d.txt").unwrap(),
            "plugin:example!/a/b c/d.txt",
        ),
        (VPath::plugin("x-1", "/").unwrap(), "plugin:x-1!/"),
        (
            VPath::plugin("a1", "/we!rd/100%25").unwrap(),
            "plugin:a1!/we!rd/100%25",
        ),
    ];
    for (path, text) in cases {
        assert_eq!(format_ipc_path(&path), text);
        let parsed = parse_ipc_path(text).unwrap();
        assert_eq!(parsed, path);
        assert_eq!(parsed.scheme(), Scheme::Plugin);
        assert_eq!(parsed.conn_id(), path.conn_id());
        assert!(parsed.scheme().is_remote() && parsed.outer().is_none());
    }
    // a plugin location never equals a server location with the same id and path
    assert_ne!(
        VPath::plugin("a", "/x").unwrap(),
        VPath::ftp("a", "/x").unwrap()
    );
    assert_eq!(
        VPath::plugin("a", "//x///y/").unwrap().path().to_str(),
        Some("/x/y")
    );
}

#[test]
fn forged_plugin_locations_are_rejected() {
    let long_name = format!("plugin:a!/{}", "n".repeat(256));
    let deep = format!("plugin:a!{}", "/d".repeat(129));
    let long_id = format!("plugin:{}!/x", "a".repeat(33));
    let long_path = format!("plugin:a!/{}", vec!["n".repeat(200); 25].join("/"));
    let bad = [
        "plugin:",
        "plugin:abc",
        "plugin:!/x",
        "plugin:ABC!/x",
        "plugin:a_b!/x",
        "plugin:a b!/x",
        "plugin:ab%21!/x",
        "plugin:a\0b!/x",
        "plugin:a!x",
        "plugin:a!",
        "plugin:a!/a/../b",
        "plugin:a!/./b",
        "plugin:a!/a\0b",
        "plugin:a!/a\\b",
        "plugin:a!/a\nb",
        "plugin:a!/a\x1bb",
        &long_name,
        &deep,
        &long_id,
        &long_path,
        // an archive cannot live in a plugin
        "archive:plugin:a!%21/x.zip!/inner",
    ];
    for text in bad {
        assert!(parse_ipc_path(text).is_err(), "must be rejected: {text:?}");
    }
    assert!(VPath::plugin(&"a".repeat(32), "/x").is_ok());
    assert!(VPath::plugin("", "/x").is_err());
    assert!(VPath::archive(VPath::plugin("a", "/x.zip").unwrap(), "/y").is_err());
    let base = VPath::plugin("a", "/").unwrap();
    assert!(base.join("a/b").is_err() && base.join("..").is_err());
    assert_eq!(base.join("ok").unwrap().conn_id(), Some("a"));
    assert_eq!(base.join("ok").unwrap().parent().unwrap(), base);
}

#[test]
fn router_keeps_plugins_apart_from_server_connections() {
    let router = VfsRouter::new();
    let plugin = MemServer::new(Scheme::Plugin, "same");
    let server = MemServer::new(Scheme::Ftp, "same");
    router.register_plugin("same", plugin.clone()).unwrap();
    // the same id as a server connection does not collide
    router.register_remote("same", server.clone()).unwrap();
    let found = router
        .vfs_for(&VPath::plugin("same", "/x").unwrap())
        .unwrap();
    assert_eq!(found.remote_endpoint(), Some((Scheme::Plugin, "same")));
    let found = router.vfs_for(&VPath::ftp("same", "/x").unwrap()).unwrap();
    assert_eq!(found.remote_endpoint(), Some((Scheme::Ftp, "same")));
    assert_eq!(router.plugin_ids(), ["same"]);
    assert!(router.has_plugin("same") && !router.has_plugin("other"));
    // bad ids, a back end of another id or scheme, and duplicates are refused
    assert!(router.register_plugin("SAME", plugin.clone()).is_err());
    assert!(router.register_plugin("other", plugin.clone()).is_err());
    assert!(router
        .register_plugin("srv", MemServer::new(Scheme::Ftp, "srv"))
        .is_err());
    assert!(router
        .register_plugin("same", MemServer::new(Scheme::Plugin, "same"))
        .is_err());
    assert!(router
        .register_remote("plug", MemServer::new(Scheme::Plugin, "plug"))
        .is_err());
    assert!(router
        .register_plugin("loc", Arc::new(crate::vfs::LocalVfs))
        .is_err());
    // an unknown or removed plugin is disconnected and never another back end
    assert!(matches!(
        router.vfs_for(&VPath::plugin("nope", "/x").unwrap()),
        Err(Error::Disconnected)
    ));
    assert!(router.unregister_plugin("same").is_some());
    assert!(router.unregister_plugin("same").is_none());
    assert!(matches!(
        router.vfs_for(&VPath::plugin("same", "/x").unwrap()),
        Err(Error::Disconnected)
    ));
    assert!(router.vfs_for(&VPath::ftp("same", "/x").unwrap()).is_ok());
    assert!(matches!(
        crate::open_path(&VPath::plugin("never-registered", "/").unwrap()),
        Err(Error::Disconnected)
    ));
}

#[test]
fn plugin_sources_copy_to_local_but_never_trash_or_sync_or_pack_into_a_plugin() {
    use crate::compare::{compare_dirs, plan_sync, CompareOptions, SyncMode, SyncOptions};
    let plugin = MemServer::new(Scheme::Plugin, "p");
    plugin.put("/a.txt", b"abc", 1);
    // copy out of the plugin into a local folder works as for a server
    let tmp = local_dir();
    let report = copy_one(
        &*plugin,
        &plugin.vp("/a.txt"),
        &crate::vfs::LocalVfs,
        &lp(tmp.path()),
        None,
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(std::fs::read(tmp.path().join("a.txt")).unwrap(), b"abc");
    // trash is unsupported and nothing is deleted
    let report = ops::trash_items(&*plugin, &[plugin.vp("/a.txt")], &JobHandle::silent());
    assert!(matches!(report.failed[0].1, Error::Unsupported(_)));
    assert!(plugin.has("/a.txt"));
    assert!(ops::restore_from_trash(&[plugin.vp("/a.txt")]).is_err());
    // a plugin folder is no synchronisation target (either side)
    let result = compare_dirs(
        &crate::vfs::LocalVfs,
        &lp(tmp.path()),
        &*plugin,
        &plugin.vp("/"),
        &CompareOptions::default(),
        &JobHandle::silent(),
    );
    let err = plan_sync(&result, SyncMode::LeftToRight, &SyncOptions::default()).unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    // packing into a plugin location is refused
    let report = crate::archive::pack(
        &crate::vfs::LocalVfs,
        &[lp(tmp.path())],
        &plugin.vp("/x.zip"),
        crate::archive::Format::Zip,
        &JobHandle::silent(),
    );
    assert!(!report.failed.is_empty(), "packing into a plugin must fail");
    assert!(!plugin.has("/x.zip"));
}

#[test]
fn verified_resume_hashes_existing_prefix_and_detects_corruption() {
    for corrupt in [false, true] {
        let tmp = local_dir();
        let data = payload(2_500_000);
        let file = tmp.path().join("big.bin");
        std::fs::write(&file, &data).unwrap();
        let server = MemServer::new(Scheme::Sftp, "verify");
        server.dir("/in");
        server.state.lock().unwrap().fail_after = Some(1_000_000);
        let first = ops::copy_verified(
            &crate::vfs::LocalVfs,
            &[lp(&file)],
            &*server,
            &server.vp("/in"),
            None,
            &JobHandle::silent(),
            ops::VerifyMode::Sha256,
        );
        assert_eq!(first.failed.len(), 1);
        {
            let mut state = server.state.lock().unwrap();
            state.fail_after = None;
            if corrupt {
                state.parts.values_mut().next().unwrap().1[0] ^= 1;
            }
        }
        let second = ops::move_verified(
            &crate::vfs::LocalVfs,
            &[lp(&file)],
            &*server,
            &server.vp("/in"),
            None,
            &JobHandle::silent(),
            ops::VerifyMode::Sha256,
        );
        assert_eq!(server.state.lock().unwrap().resumed_from, [0, 1_000_000]);
        assert!(server.has("/in/big.bin"));
        if corrupt {
            assert!(matches!(second.failed[0].1, Error::VerifyFailed));
            assert_eq!(std::fs::read(file).unwrap(), data);
        } else {
            assert!(second.failed.is_empty(), "{:?}", second.failed);
            assert!(!file.exists());
            assert_eq!(server.get("/in/big.bin").unwrap(), data);
        }
    }
}
