//! In-memory undo history. Record only successfully completed operations, immediately
//! afterwards. Callers must supply whether ANY destination was replaced; that fact
//! cannot be reconstructed from the resulting filesystem. Integration belongs to 9b.
//!
//! Fingerprints are deliberately not content hashes: directories compare only kind.
//! VFS checks and mutations are separate calls, not an atomic filesystem transaction.
use crate::ops::{restore_from_trash, JobHandle};
use crate::vfs::{EntryKind, LocalVfs, ReplaceMode, Scheme, VPath, Vfs};
use crate::Error;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

pub type UndoId = u64;
type Result<T> = std::result::Result<T, UndoError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UndoError {
    #[error("Rückgängig ist nur für lokale Pfade möglich")]
    NotLocal,
    #[error("Der Eintrag hat sich inzwischen geändert")]
    Changed,
    #[error("Der Ursprungspfad ist bereits belegt oder der Eintrag ist mehrdeutig")]
    Conflict,
    #[error("Der Vorgang wurde bereits zurückgenommen")]
    AlreadyUndone,
    #[error("Der Eintrag wurde nicht gefunden")]
    NotFound,
    #[error("Rückgängig ist an einem Dateisystemfehler gescheitert")]
    Io,
    #[error("Diese Rücknahme wird nicht unterstützt")]
    Unsupported,
}
impl From<Error> for UndoError {
    fn from(error: Error) -> Self {
        match error {
            Error::NotFound(_) => Self::NotFound,
            Error::Exists(_) => Self::Conflict,
            Error::ChangedSincePlan(_) | Error::DirectoryNotEmpty(_) => Self::Changed,
            Error::Unsupported(_) | Error::NoReplaceUnsupported => Self::Unsupported,
            _ => Self::Io,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UndoKind {
    Move,
    Rename,
    Trash,
    Copy,
    MakeDir,
}
#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub id: UndoId,
    pub time: SystemTime,
    pub label: String,
    pub first: String,
    pub more: usize,
    pub kind: UndoKind,
    pub undone: bool,
}
#[derive(Debug, Default)]
pub struct UndoReport {
    /// Successfully reverted items during this attempt.
    pub done: Vec<VPath>,
    pub failed: Vec<(VPath, UndoError)>,
    /// All items still pending, including failures and cancellation.
    pub remaining: Vec<VPath>,
    pub cancelled: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fingerprint {
    kind: EntryKind,
    stamp: Option<(u64, SystemTime)>,
}
impl Fingerprint {
    fn read(vfs: &dyn Vfs, path: &VPath) -> Result<Self> {
        let e = vfs.stat(path)?;
        Ok(Self {
            kind: e.kind,
            stamp: if e.kind == EntryKind::Dir {
                None
            } else {
                Some((e.size, e.modified.ok_or(UndoError::Io)?))
            },
        })
    }
}
struct Item {
    target: VPath,
    original: Option<VPath>,
    fingerprint: Fingerprint,
    done: bool,
}
struct Record {
    entry: UndoEntry,
    items: Vec<Item>,
}
fn present_record(record: &Record) -> UndoEntry {
    let mut entry = record.entry.clone();
    entry.first = record
        .items
        .first()
        .and_then(|item| {
            item.original
                .as_ref()
                .unwrap_or(&item.target)
                .path()
                .file_name()
        })
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    entry.more = record.items.len().saturating_sub(1);
    entry
}

#[derive(Default)]
struct State {
    next: UndoId,
    records: VecDeque<Record>,
}
#[derive(Default)]
pub struct UndoJournal {
    state: Mutex<State>,
}

impl UndoJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// `pairs` are (original, resulting destination). `replaced` covers the whole batch.
    pub fn record_move(&self, pairs: &[(VPath, VPath)], replaced: bool) -> Result<Option<UndoId>> {
        self.record_pairs(UndoKind::Move, pairs, replaced)
    }
    pub fn record_rename(
        &self,
        original: &VPath,
        target: &VPath,
        replaced: bool,
    ) -> Result<Option<UndoId>> {
        self.record_pairs(
            UndoKind::Rename,
            &[(original.clone(), target.clone())],
            replaced,
        )
    }
    pub fn record_renames(&self, pairs: &[(VPath, VPath)]) -> Result<Option<UndoId>> {
        // Chains and cycles need the multirename engine, not independent inverses.
        if pairs.iter().any(|(a, _)| pairs.iter().any(|(_, b)| a == b)) {
            return Ok(None);
        }
        self.record_pairs(UndoKind::Rename, pairs, false)
    }
    fn record_pairs(
        &self,
        kind: UndoKind,
        pairs: &[(VPath, VPath)],
        replaced: bool,
    ) -> Result<Option<UndoId>> {
        if replaced
            || pairs
                .iter()
                .any(|(a, b)| a.scheme() != Scheme::Local || b.scheme() != Scheme::Local)
        {
            return Ok(None);
        }
        let items = pairs
            .iter()
            .map(|(a, b)| {
                Ok(Item {
                    target: b.clone(),
                    original: Some(a.clone()),
                    fingerprint: Fingerprint::read(&LocalVfs, b)?,
                    done: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.insert(kind, items)
    }
    pub fn record_copy_created(&self, paths: &[VPath], replaced: bool) -> Result<Option<UndoId>> {
        self.record_created(UndoKind::Copy, paths, replaced)
    }
    pub fn record_make_dir(&self, path: &VPath) -> Result<Option<UndoId>> {
        self.record_created(UndoKind::MakeDir, std::slice::from_ref(path), false)
    }
    fn record_created(
        &self,
        kind: UndoKind,
        paths: &[VPath],
        replaced: bool,
    ) -> Result<Option<UndoId>> {
        if replaced || paths.iter().any(|p| p.scheme() != Scheme::Local) {
            return Ok(None);
        }
        let items = paths
            .iter()
            .map(|p| {
                Ok(Item {
                    target: p.clone(),
                    original: None,
                    fingerprint: Fingerprint::read(&LocalVfs, p)?,
                    done: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if kind == UndoKind::MakeDir && items.iter().any(|i| i.fingerprint.kind != EntryKind::Dir) {
            return Err(UndoError::Changed);
        }
        self.insert(kind, items)
    }
    /// Called after trashing. Resolves each original to exactly one trash entry.
    /// Linux trash discovery necessarily reads the system trash, outside input paths.
    pub fn record_trash(&self, originals: &[VPath]) -> Result<Option<UndoId>> {
        if originals.iter().any(|p| p.scheme() != Scheme::Local) {
            return Ok(None);
        }
        let items = originals
            .iter()
            .map(|original| {
                let target = trash_target(original)?;
                Ok(Item {
                    fingerprint: Fingerprint::read(&LocalVfs, &target)?,
                    target,
                    original: Some(original.clone()),
                    done: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.insert(UndoKind::Trash, items)
    }
    fn insert(&self, kind: UndoKind, items: Vec<Item>) -> Result<Option<UndoId>> {
        if items.is_empty() {
            return Ok(None);
        }
        // Overlapping roots could be moved/trashed twice. Require disjoint targets.
        for (n, item) in items.iter().enumerate() {
            if items[..n].iter().any(|other| {
                item.target.path().starts_with(other.target.path())
                    || other.target.path().starts_with(item.target.path())
            }) {
                return Err(UndoError::Conflict);
            }
        }
        let mut state = self.state.lock().map_err(|_| UndoError::Io)?;
        state.next = state.next.checked_add(1).ok_or(UndoError::Io)?;
        let id = state.next;
        let label = match kind {
            UndoKind::Move => "Verschieben",
            UndoKind::Rename => "Umbenennen",
            UndoKind::Trash => "Papierkorb",
            UndoKind::Copy => "Kopieren",
            UndoKind::MakeDir => "Ordner anlegen",
        }
        .to_owned();
        state.records.push_back(Record {
            entry: UndoEntry {
                id,
                time: SystemTime::now(),
                label,
                first: String::new(),
                more: 0,
                kind,
                undone: false,
            },
            items,
        });
        if state.records.len() > 200 {
            state.records.pop_front();
        }
        Ok(Some(id))
    }
    pub fn list(&self) -> Vec<UndoEntry> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .records
            .iter()
            .rev()
            .map(present_record)
            .collect()
    }
    pub fn last_undoable(&self) -> Option<UndoEntry> {
        self.list().into_iter().find(|e| !e.undone)
    }

    /// Serializes attempts, preserves successful items on failure, and retries only
    /// pending items. Each item is checked immediately before its VFS mutation.
    pub fn undo(&self, id: UndoId, vfs: &dyn Vfs, job: &JobHandle) -> Result<UndoReport> {
        if !vfs.is_local() {
            return Err(UndoError::NotLocal);
        }
        let mut state = self.state.lock().map_err(|_| UndoError::Io)?;
        let record = state
            .records
            .iter_mut()
            .find(|r| r.entry.id == id)
            .ok_or(UndoError::NotFound)?;
        if record.entry.undone {
            return Err(UndoError::AlreadyUndone);
        }
        let mut report = UndoReport::default();
        for item in record.items.iter_mut().filter(|i| !i.done) {
            if job.is_cancelled() {
                report.cancelled = true;
                break;
            }
            match revert(record.entry.kind, item, vfs) {
                Ok(()) => {
                    item.done = true;
                    report.done.push(item.target.clone());
                }
                Err(e) => report.failed.push((item.target.clone(), e)),
            }
        }
        report.remaining = record
            .items
            .iter()
            .filter(|i| !i.done)
            .map(|i| i.target.clone())
            .collect();
        record.entry.undone = report.remaining.is_empty();
        Ok(report)
    }
}
fn vacant(vfs: &dyn Vfs, path: &VPath) -> Result<()> {
    match vfs.stat(path) {
        Err(Error::NotFound(_)) => Ok(()),
        Ok(_) => Err(UndoError::Conflict),
        Err(e) => Err(e.into()),
    }
}
fn revert(kind: UndoKind, item: &Item, vfs: &dyn Vfs) -> Result<()> {
    if Fingerprint::read(vfs, &item.target).map_err(|e| {
        if e == UndoError::NotFound {
            UndoError::Changed
        } else {
            e
        }
    })? != item.fingerprint
    {
        return Err(UndoError::Changed);
    }
    match kind {
        UndoKind::Move | UndoKind::Rename => {
            let original = item.original.as_ref().ok_or(UndoError::Io)?;
            vacant(vfs, original)?;
            vfs.rename(&item.target, original, ReplaceMode::NoReplace)?;
        }
        UndoKind::Trash => {
            let original = item.original.as_ref().ok_or(UndoError::Io)?;
            vacant(vfs, original)?;
            if trash_target(original)? != item.target {
                return Err(UndoError::Changed);
            }
            let report = restore_from_trash(std::slice::from_ref(original))?;
            if !report.done.contains(original) {
                return Err(UndoError::NotFound);
            }
        }
        UndoKind::Copy | UndoKind::MakeDir => {
            if kind == UndoKind::MakeDir && !vfs.list(&item.target)?.is_empty() {
                return Err(UndoError::Changed);
            }
            vfs.trash(&item.target)?;
        }
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn trash_target(original: &VPath) -> Result<VPath> {
    let entries = trash::os_limited::list().map_err(|_| UndoError::Io)?;
    let mut matches = entries
        .iter()
        .filter(|e| e.original_path() == original.path());
    let item = matches.next().ok_or(UndoError::NotFound)?;
    if matches.next().is_some() {
        return Err(UndoError::Conflict);
    }
    // trash 5 documents Linux IDs as absolute .trashinfo paths. Freedesktop
    // stores the payload under ../files/<stem>, including collision suffixes.
    let info = std::path::Path::new(&item.id);
    let root = info
        .parent()
        .and_then(|p| p.parent())
        .ok_or(UndoError::Io)?;
    VPath::local(
        root.join("files")
            .join(info.file_stem().ok_or(UndoError::Io)?),
    )
    .map_err(Into::into)
}
#[cfg(not(target_os = "linux"))]
fn trash_target(_: &VPath) -> Result<VPath> {
    Err(UndoError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    fn p(path: impl AsRef<Path>) -> VPath {
        VPath::local(path).unwrap()
    }
    #[test]
    fn presentation_uses_original_name_and_root_count() {
        let item = |name: &str| Item {
            target: p(format!("/trash/{name}-internal")),
            original: Some(p(format!("/source/{name}"))),
            fingerprint: Fingerprint {
                kind: EntryKind::File,
                stamp: None,
            },
            done: false,
        };
        let record = Record {
            entry: UndoEntry {
                id: 1,
                time: SystemTime::UNIX_EPOCH,
                label: String::new(),
                first: String::new(),
                more: 0,
                kind: UndoKind::Trash,
                undone: false,
            },
            items: vec![item("first.txt"), item("second.txt")],
        };
        let shown = present_record(&record);
        assert_eq!(shown.first, "first.txt");
        assert_eq!(shown.more, 1);
        assert!(!shown.undone);
    }

    fn moved(dir: &Path, name: &str) -> (VPath, VPath) {
        let a = p(dir.join(name));
        let b = p(dir.join(format!("{name}-new")));
        fs::write(a.path(), b"abc").unwrap();
        LocalVfs.rename(&a, &b, ReplaceMode::NoReplace).unwrap();
        (a, b)
    }
    #[test]
    fn transfer_report_tracks_actual_targets_and_successful_replacements() {
        use crate::ops::{self, Conflict, ConflictResolver, Resolution};
        struct Choose(Resolution);
        impl ConflictResolver for Choose {
            fn resolve(&self, _: &Conflict) -> Resolution {
                self.0
            }
        }
        let d = tempfile::tempdir().unwrap();
        let source = p(d.path().join("a"));
        let dest = p(d.path().join("dest"));
        fs::write(source.path(), "new").unwrap();
        fs::create_dir(dest.path()).unwrap();
        fs::write(dest.path().join("a"), "old").unwrap();
        let report = ops::copy(
            &LocalVfs,
            std::slice::from_ref(&source),
            &LocalVfs,
            &dest,
            Some(&Choose(Resolution::KeepBoth)),
            &JobHandle::silent(),
        );
        assert!(!report.replaced);
        assert_eq!(
            report.created,
            vec![(source.clone(), p(dest.path().join("a (2)")))]
        );
        let report = ops::copy(
            &LocalVfs,
            std::slice::from_ref(&source),
            &LocalVfs,
            &dest,
            Some(&Choose(Resolution::Overwrite)),
            &JobHandle::silent(),
        );
        assert!(report.replaced);
        assert!(report.created.is_empty());
        let target = p(d.path().join("moved"));
        fs::create_dir(target.path()).unwrap();
        let report = ops::move_with_strategy(
            &LocalVfs,
            std::slice::from_ref(&source),
            &LocalVfs,
            &target,
            None,
            &JobHandle::silent(),
            ops::MoveStrategy::ForceCopy,
        );
        assert_eq!(
            report.created,
            vec![(source.clone(), p(target.path().join("a")))]
        );
        let journal = UndoJournal::new();
        let id = journal
            .record_move(&report.created, report.replaced)
            .unwrap()
            .unwrap();
        assert!(journal
            .undo(id, &LocalVfs, &JobHandle::silent())
            .unwrap()
            .remaining
            .is_empty());
        assert_eq!(fs::read_to_string(source.path()).unwrap(), "new");
    }

    #[test]
    fn move_back_and_double_undo() {
        let d = tempfile::tempdir().unwrap();
        let pair = moved(d.path(), "a");
        let j = UndoJournal::new();
        let id = j
            .record_move(std::slice::from_ref(&pair), false)
            .unwrap()
            .unwrap();
        let r = j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap();
        assert_eq!(r.done, vec![pair.1.clone()]);
        assert!(r.remaining.is_empty());
        assert_eq!(fs::read(pair.0.path()).unwrap(), b"abc");
        assert!(!pair.1.path().exists());
        assert_eq!(
            j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap_err(),
            UndoError::AlreadyUndone
        );
        assert!(j.last_undoable().is_none());
    }
    #[test]
    fn rename_back() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = moved(d.path(), "a");
        let j = UndoJournal::new();
        let id = j.record_rename(&a, &b, false).unwrap().unwrap();
        assert!(j
            .undo(id, &LocalVfs, &JobHandle::silent())
            .unwrap()
            .failed
            .is_empty());
        assert!(a.path().exists());
    }
    #[test]
    fn changed_size_and_nanosecond_mtime_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = moved(d.path(), "a");
        let j = UndoJournal::new();
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::open(b.path()).unwrap().set_modified(t).unwrap();
        let id = j.record_rename(&a, &b, false).unwrap().unwrap();
        fs::File::open(b.path())
            .unwrap()
            .set_modified(t + std::time::Duration::from_nanos(1))
            .unwrap();
        let r = j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap();
        assert_eq!(r.failed[0].1, UndoError::Changed);
        fs::write(b.path(), b"longer").unwrap();
        let r = j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap();
        assert_eq!(r.failed[0].1, UndoError::Changed);
        assert!(!a.path().exists());
    }
    #[test]
    fn conflict_partial_failure_and_retry() {
        let d = tempfile::tempdir().unwrap();
        let a = moved(d.path(), "a");
        let b = moved(d.path(), "b");
        let j = UndoJournal::new();
        let id = j
            .record_move(&[a.clone(), b.clone()], false)
            .unwrap()
            .unwrap();
        fs::write(b.0.path(), b"keep").unwrap();
        let r = j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap();
        assert_eq!(r.done, vec![a.1.clone()]);
        assert_eq!(r.failed[0].1, UndoError::Conflict);
        assert_eq!(r.remaining, vec![b.1.clone()]);
        assert!(!j.list()[0].undone);
        assert_eq!(fs::read(b.0.path()).unwrap(), b"keep");
        fs::remove_file(b.0.path()).unwrap();
        let r = j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap();
        assert_eq!(r.done, vec![b.1]);
        assert!(j.list()[0].undone);
    }
    #[test]
    fn nonlocal_and_replacement_skip_entire_batch_before_io() {
        let j = UndoJournal::new();
        let local = p("/does-not-exist");
        let remote = VPath::sftp("test", "/a").unwrap();
        assert_eq!(
            j.record_move(
                &[
                    (local.clone(), local.clone()),
                    (remote.clone(), local.clone())
                ],
                false
            )
            .unwrap(),
            None
        );
        assert_eq!(
            j.record_move(&[(local.clone(), local.clone())], true)
                .unwrap(),
            None
        );
        assert_eq!(j.record_rename(&local, &local, true).unwrap(), None);
        assert_eq!(
            j.record_copy_created(&[local.clone(), remote.clone()], false)
                .unwrap(),
            None
        );
        assert_eq!(j.record_copy_created(&[local], true).unwrap(), None);
        assert_eq!(j.record_make_dir(&remote).unwrap(), None);
        assert_eq!(j.record_trash(&[remote]).unwrap(), None);
        assert!(j.list().is_empty());
    }
    #[test]
    fn bounded_newest_first_and_missing_id() {
        let d = tempfile::tempdir().unwrap();
        let path = p(d.path());
        let j = UndoJournal::new();
        for _ in 0..201 {
            j.record_make_dir(&path).unwrap();
        }
        let entries = j.list();
        assert_eq!(entries.len(), 200);
        assert_eq!(entries[0].id, 201);
        assert_eq!(entries[199].id, 2);
        assert_eq!(j.last_undoable().unwrap().id, 201);
        assert_eq!(
            j.undo(1, &LocalVfs, &JobHandle::silent()).unwrap_err(),
            UndoError::NotFound
        );
    }
    #[test]
    fn cancellation_preserves_pending_items() {
        let d = tempfile::tempdir().unwrap();
        let pair = moved(d.path(), "a");
        let j = UndoJournal::new();
        let id = j
            .record_move(std::slice::from_ref(&pair), false)
            .unwrap()
            .unwrap();
        let job = JobHandle::silent();
        job.cancel();
        let r = j.undo(id, &LocalVfs, &job).unwrap();
        assert!(r.cancelled);
        assert_eq!(r.remaining, vec![pair.1]);
        assert!(r.done.is_empty());
        assert!(!j.list()[0].undone);
    }
    #[test]
    fn make_dir_rejects_nonempty_directory() {
        let d = tempfile::tempdir().unwrap();
        let j = UndoJournal::new();
        let path = p(d.path());
        let id = j.record_make_dir(&path).unwrap().unwrap();
        fs::write(d.path().join("new"), b"keep").unwrap();
        assert_eq!(
            j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap().failed[0].1,
            UndoError::Changed
        );
    }
    #[cfg(target_os = "linux")]
    struct Env(Vec<(&'static str, Option<std::ffi::OsString>)>);
    #[cfg(target_os = "linux")]
    impl Drop for Env {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    fn trash_env(dir: &Path) -> Env {
        let mut old = Vec::new();
        for (key, sub) in [("HOME", "home"), ("XDG_DATA_HOME", "data")] {
            old.push((key, std::env::var_os(key)));
            let path = dir.join(sub);
            fs::create_dir(&path).unwrap();
            std::env::set_var(key, path);
        }
        Env(old)
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn copy_to_trash_then_restore_and_empty_mkdir_to_trash() {
        let _lock = crate::vfs::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let _env = trash_env(d.path());
        let j = UndoJournal::new();
        let file = p(d.path().join("copy"));
        fs::write(file.path(), b"copy").unwrap();
        let id = j
            .record_copy_created(std::slice::from_ref(&file), false)
            .unwrap()
            .unwrap();
        assert!(j
            .undo(id, &LocalVfs, &JobHandle::silent())
            .unwrap()
            .remaining
            .is_empty());
        assert!(!file.path().exists());
        assert!(trash::os_limited::list()
            .unwrap()
            .iter()
            .any(|i| i.original_path() == file.path()));
        let id = j
            .record_trash(std::slice::from_ref(&file))
            .unwrap()
            .unwrap();
        assert!(j
            .undo(id, &LocalVfs, &JobHandle::silent())
            .unwrap()
            .remaining
            .is_empty());
        assert_eq!(fs::read(file.path()).unwrap(), b"copy");
        let folder = p(d.path().join("folder"));
        fs::create_dir(folder.path()).unwrap();
        let id = j.record_make_dir(&folder).unwrap().unwrap();
        assert!(j
            .undo(id, &LocalVfs, &JobHandle::silent())
            .unwrap()
            .remaining
            .is_empty());
        assert!(!folder.path().exists());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn trash_conflict_and_changed_payload_are_rejected() {
        let _lock = crate::vfs::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let _env = trash_env(d.path());
        let j = UndoJournal::new();
        let file = p(d.path().join("trash"));
        fs::write(file.path(), b"old").unwrap();
        LocalVfs.trash(&file).unwrap();
        let id = j
            .record_trash(std::slice::from_ref(&file))
            .unwrap()
            .unwrap();
        fs::write(file.path(), b"keep").unwrap();
        assert_eq!(
            j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap().failed[0].1,
            UndoError::Conflict
        );
        fs::remove_file(file.path()).unwrap();
        let payload = trash_target(&file).unwrap();
        fs::write(payload.path(), b"changed").unwrap();
        assert_eq!(
            j.undo(id, &LocalVfs, &JobHandle::silent()).unwrap().failed[0].1,
            UndoError::Changed
        );
        fs::write(file.path(), b"second").unwrap();
        LocalVfs.trash(&file).unwrap();
        assert_eq!(j.record_trash(&[file]).unwrap_err(), UndoError::Conflict);
    }
}
