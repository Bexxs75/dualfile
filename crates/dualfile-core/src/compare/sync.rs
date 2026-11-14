use super::dirs::{CompareResult, CompareState, SideInfo};
use crate::archive::MSG_READ_ONLY;

/// Synchronizing onto a server or into a plugin is not part of this version (a server cannot replace atomically).
pub const MSG_REMOTE_TARGET: &str =
    "Auf Servern und in Plugins wird in dieser Version nicht synchronisiert";
use crate::error::{from_io, Error, Result};
use crate::ops::{
    self, Conflict, ConflictResolver, JobHandle, OpReport, ProgressEvent, Resolution,
};
use crate::vfs::{EntryKind, Scheme, VPath, Vfs};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncMode {
    /// Left is the source, right the target.
    LeftToRight,
    /// Right is the source, left the target.
    RightToLeft,
    /// Like `LeftToRight`, and entries only on the right go to the trash. With
    /// `replace_newer` differing files are overwritten in both "directions" of newness,
    /// so the right side ends up equal to the left.
    Mirror,
}

#[derive(Clone, Debug)]
pub struct SyncOptions {
    /// `false` (default): only add what is missing. `true`: also replace files where the
    /// source is newer (Mirror: also where the target is newer or differs).
    pub replace_newer: bool,
    /// Most bytes hashed while planning overwrites; a larger target is not planned.
    pub max_hash_bytes: u64,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            replace_newer: false,
            max_hash_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

/// What a target entry looked like when it was planned; read again before every
/// overwrite or removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetVersion {
    pub kind: EntryKind,
    /// Device and inode (lstat).
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i128,
    /// SHA-256 of the content (planned overwrites of files).
    pub sha256: Option<[u8; 32]>,
    pub link_target: Option<String>,
    /// Folders: names of the entries when planned (all of them are planned for removal
    /// as well; the folder is only removed once it is empty).
    pub children: Vec<String>,
    /// Device and inode of the folder the entry sits in.
    pub parent: Option<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub enum SyncAction {
    /// Creates a missing folder in the target (permissions and time are not carried over).
    MakeDir { path: VPath },
    /// Copies a file or link. `overwrite == false`: the target must still not exist.
    /// `overwrite == true`: the target must still look like `expected_target`.
    Copy {
        from: VPath,
        to: VPath,
        source_kind: EntryKind,
        size: u64,
        overwrite: bool,
        expected_target: Option<TargetVersion>,
    },
    /// Moves a target file or link to the trash; an emptied folder is removed with
    /// `rmdir` instead (see `remove_empty_dir`).
    Trash {
        path: VPath,
        expected: TargetVersion,
    },
}

impl SyncAction {
    /// The location this action changes.
    pub fn target_path(&self) -> &VPath {
        match self {
            SyncAction::MakeDir { path } | SyncAction::Trash { path, .. } => path,
            SyncAction::Copy { to, .. } => to,
        }
    }
}

/// An entry the plan leaves alone, with the reason.
#[derive(Clone, Debug)]
pub struct PlanNote {
    pub rel: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct SyncPlan {
    pub token: u128,
    pub mode: SyncMode,
    pub source: VPath,
    pub target: VPath,
    /// Order: folders and copies top-down, then removals bottom-up.
    pub actions: Vec<SyncAction>,
    pub make_dirs: usize,
    pub copies: usize,
    pub replaces: usize,
    pub trashes: usize,
    pub bytes: u64,
    pub notes: Vec<PlanNote>,
    /// The compare result was incomplete: no removals are planned and nothing under
    /// unknown parts.
    pub incomplete: bool,
}

impl SyncPlan {
    fn recount(&mut self) {
        self.make_dirs = 0;
        self.copies = 0;
        self.replaces = 0;
        self.trashes = 0;
        self.bytes = 0;
        for action in &self.actions {
            match action {
                SyncAction::MakeDir { .. } => self.make_dirs += 1,
                SyncAction::Copy {
                    overwrite, size, ..
                } => {
                    self.bytes += size;
                    if *overwrite {
                        self.replaces += 1;
                    } else {
                        self.copies += 1;
                    }
                }
                SyncAction::Trash { .. } => self.trashes += 1,
            }
        }
    }

    /// A new plan (new token) with only the actions at `keep` (indices into `actions`);
    /// this is how single actions are deselected before the confirmation.
    pub fn select(&self, keep: &[usize]) -> SyncPlan {
        let mut plan = SyncPlan {
            token: ops::new_token(),
            actions: self
                .actions
                .iter()
                .enumerate()
                .filter(|(i, _)| keep.contains(i))
                .map(|(_, a)| a.clone())
                .collect(),
            ..self.clone()
        };
        plan.recount();
        plan
    }
}

#[cfg(unix)]
fn ids(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Reads the current version of `path` (without following a link). Without inodes
/// (non-Unix) nothing can be verified and an error is returned.
fn capture(
    path: &Path,
    hash: bool,
    hash_budget: Option<&mut u64>,
) -> std::result::Result<TargetVersion, String> {
    #[cfg(not(unix))]
    {
        let _ = (path, hash, hash_budget);
        Err("Die Zielversion kann auf diesem System nicht geprüft werden".to_string())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let text = |e: std::io::Error| e.to_string();
        let meta = std::fs::symlink_metadata(path).map_err(text)?;
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let (dev, ino) = ids(&meta);
        let parent = path
            .parent()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| ids(&m));
        let mut version = TargetVersion {
            kind,
            dev,
            ino,
            size: if kind == EntryKind::File {
                meta.len()
            } else {
                0
            },
            mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
            sha256: None,
            link_target: None,
            children: Vec::new(),
            parent,
        };
        match kind {
            EntryKind::Symlink => {
                version.link_target = Some(
                    std::fs::read_link(path)
                        .map_err(text)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            EntryKind::Dir => {
                let mut names = Vec::new();
                for item in std::fs::read_dir(path).map_err(text)? {
                    names.push(
                        item.map_err(text)?
                            .file_name()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                names.sort();
                version.children = names;
            }
            EntryKind::File if hash => {
                if let Some(budget) = hash_budget {
                    if meta.len() > *budget {
                        return Err("Die Datei ist für die Versionsprüfung zu groß".to_string());
                    }
                    *budget -= meta.len();
                }
                // Opens without blocking and refuses pipes, devices and sockets.
                let (mut file, opened) =
                    crate::vfs::open_regular(path).map_err(|e| e.to_string())?;
                if ids(&opened) != (dev, ino) {
                    return Err("Die Datei wurde während der Prüfung ausgetauscht".to_string());
                }
                let mut hasher = Sha256::new();
                let mut buf = vec![0u8; 1024 * 1024];
                let mut total = 0u64;
                loop {
                    let n = std::io::Read::read(&mut file, &mut buf).map_err(text)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    total += n as u64;
                }
                if total != meta.len() {
                    return Err("Die Datei hat sich während der Prüfung geändert".to_string());
                }
                version.sha256 = Some(hasher.finalize().into());
            }
            EntryKind::File => {}
        }
        Ok(version)
    }
}

/// Compares the current state of `path` with the planned one. `Err` carries the reason.
fn verify(path: &Path, expected: &TargetVersion) -> std::result::Result<(), String> {
    verify_in(path, expected, true)
}

/// `check_parent == false` for a file that was moved out of its folder (the entry that
/// an exchange displaced into the staging folder).
fn verify_in(
    path: &Path,
    expected: &TargetVersion,
    check_parent: bool,
) -> std::result::Result<(), String> {
    let now = capture(path, expected.sha256.is_some(), None)?;
    if now.kind != expected.kind || (now.dev, now.ino) != (expected.dev, expected.ino) {
        return Err("Der Eintrag wurde seit der Planung ausgetauscht".into());
    }
    if check_parent && now.parent != expected.parent {
        return Err("Der übergeordnete Ordner wurde seit der Planung ausgetauscht".into());
    }
    if expected.kind == EntryKind::Dir {
        // Content is removed bottom-up before the folder; whatever is left was either
        // added after planning or could not be removed. Either way the folder stays.
        if let Some(name) = now.children.first() {
            return Err(if expected.children.contains(name) {
                format!("Der Ordner enthält noch geplante Einträge ({name})")
            } else {
                format!("Im Ordner liegt ein neuer Eintrag ({name})")
            });
        }
        return Ok(());
    }
    if now.size != expected.size
        || now.mtime_ns != expected.mtime_ns
        || now.link_target != expected.link_target
        || now.sha256 != expected.sha256
    {
        return Err("Der Eintrag hat sich seit der Planung geändert".into());
    }
    Ok(())
}

fn base_path(path: &VPath) -> &Path {
    match path.outer() {
        None => path.path(),
        Some(outer) => base_path(outer),
    }
}

/// Source and target must be different places and neither may lie inside the other.
fn check_roots(source: &VPath, target: &VPath) -> Result<()> {
    if source.scheme().is_remote() || target.scheme().is_remote() {
        // A server location never names a local file; there is nothing to canonicalize.
        return Ok(());
    }
    let (sp, tp) = (base_path(source), base_path(target));
    let canon = |p: &Path| std::fs::canonicalize(p).map_err(|e| from_io("Pfad auflösen", p, e));
    let (sc, tc) = (canon(sp)?, canon(tp)?);
    #[cfg(unix)]
    if source.scheme() == Scheme::Local && target.scheme() == Scheme::Local {
        let same = match (std::fs::metadata(&sc), std::fs::metadata(&tc)) {
            (Ok(a), Ok(b)) => ids(&a) == ids(&b),
            _ => false,
        };
        if same {
            return Err(Error::SameFile);
        }
    }
    if sc == tc {
        return Err(Error::SameFile);
    }
    if sc.starts_with(&tc) || tc.starts_with(&sc) {
        return Err(Error::DestinationInsideSource);
    }
    Ok(())
}

fn join_rel(root: &VPath, rel: &str) -> Result<VPath> {
    let mut path = root.clone();
    for part in rel.split('/') {
        path = path.join_entry(part)?;
    }
    Ok(path)
}

fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Builds the sync plan for `result`. Only entries known for sure get actions: nothing
/// under an `Unknown` part, and with an incomplete result no removals at all. A target
/// inside an archive is refused; source and target must not be the same place or nested.
pub fn plan_sync(
    result: &CompareResult,
    mode: SyncMode,
    options: &SyncOptions,
) -> Result<SyncPlan> {
    plan_sync_with_job(result, mode, options, &JobHandle::silent())
}

pub fn plan_sync_with_job(
    result: &CompareResult,
    mode: SyncMode,
    options: &SyncOptions,
    job: &JobHandle,
) -> Result<SyncPlan> {
    let src_is_left = mode != SyncMode::RightToLeft;
    let (source, target) = if src_is_left {
        (&result.left, &result.right)
    } else {
        (&result.right, &result.left)
    };
    if target.scheme().is_remote() {
        return Err(Error::Unsupported(MSG_REMOTE_TARGET));
    }
    if target.scheme() != Scheme::Local {
        return Err(Error::Unsupported(MSG_READ_ONLY));
    }
    check_roots(source, target)?;

    let incomplete = result.incomplete || result.cancelled;
    let unknown: Vec<&str> = result
        .entries
        .iter()
        .filter(|e| e.state == CompareState::Unknown)
        .map(|e| e.rel.as_str())
        .collect();
    let blocked = |rel: &str| {
        unknown.iter().any(|u| {
            u.is_empty() || rel == *u || (rel.starts_with(u) && rel[u.len()..].starts_with('/'))
        })
    };
    let mut children: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for entry in &result.entries {
        let rel = entry.rel.as_str();
        if !rel.is_empty() {
            let name = rel.rsplit_once('/').map_or(rel, |(_, n)| n);
            children.entry(parent_rel(rel)).or_default().insert(name);
        }
    }

    let mut plan = SyncPlan {
        token: ops::new_token(),
        mode,
        source: source.clone(),
        target: target.clone(),
        actions: Vec::new(),
        make_dirs: 0,
        copies: 0,
        replaces: 0,
        trashes: 0,
        bytes: 0,
        notes: Vec::new(),
        incomplete,
    };
    let note = |plan: &mut SyncPlan, rel: &str, reason: &str| {
        plan.notes.push(PlanNote {
            rel: rel.to_string(),
            reason: reason.to_string(),
        });
    };
    let mut hash_budget = options.max_hash_bytes;
    let mut removals: Vec<(&str, &SideInfo, bool)> = Vec::new();

    for entry in &result.entries {
        if job.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let rel = entry.rel.as_str();
        if rel.is_empty() || blocked(rel) {
            continue;
        }
        let (src_side, dst_side) = if src_is_left {
            (&entry.left, &entry.right)
        } else {
            (&entry.right, &entry.left)
        };
        match entry.state {
            CompareState::Unknown | CompareState::Same => {}
            CompareState::TypeMismatch => note(
                &mut plan,
                rel,
                "Datei, Ordner und Verknüpfung passen nicht zusammen; nicht automatisch angeglichen",
            ),
            CompareState::OnlyLeft | CompareState::OnlyRight => match (src_side, dst_side) {
                (Some(info), None) => {
                    let (from, to) = (join_rel(source, rel)?, join_rel(target, rel)?);
                    plan.actions.push(match info.kind {
                        EntryKind::Dir => SyncAction::MakeDir { path: to },
                        kind => SyncAction::Copy {
                            from,
                            to,
                            source_kind: kind,
                            size: info.size,
                            overwrite: false,
                            expected_target: None,
                        },
                    });
                }
                (None, Some(info)) if mode == SyncMode::Mirror => removals.push((rel, info, entry.listed)),
                _ => {}
            },
            CompareState::LeftNewer | CompareState::RightNewer | CompareState::Different => {
                let (Some(src), Some(dst)) = (src_side, dst_side) else {
                    continue;
                };
                if src.kind != EntryKind::File || dst.kind != EntryKind::File {
                    note(&mut plan, rel, "Verknüpfungen werden nicht ersetzt");
                    continue;
                }
                let source_newer = matches!(
                    (entry.state, src_is_left),
                    (CompareState::LeftNewer, true) | (CompareState::RightNewer, false)
                );
                let eligible = options.replace_newer && (source_newer || mode == SyncMode::Mirror);
                if !eligible {
                    let reason = if source_newer {
                        "Die Quelle ist neuer; nicht ersetzt (nur Fehlendes ergänzen)"
                    } else if entry.state == CompareState::Different {
                        "Gleiche Zeit, anderer Inhalt; nicht ersetzt"
                    } else {
                        "Das Ziel ist neuer; nicht ersetzt"
                    };
                    note(&mut plan, rel, reason);
                    continue;
                }
                let to = join_rel(target, rel)?;
                match capture(to.path(), true, Some(&mut hash_budget)) {
                    Ok(version) if version.kind == EntryKind::File => {
                        plan.actions.push(SyncAction::Copy {
                            from: join_rel(source, rel)?,
                            to,
                            source_kind: EntryKind::File,
                            size: src.size,
                            overwrite: true,
                            expected_target: Some(version),
                        })
                    }
                    Ok(_) => note(&mut plan, rel, "Das Ziel ist keine Datei mehr"),
                    Err(reason) => note(
                        &mut plan,
                        rel,
                        &format!("Zielversion nicht prüfbar, nicht ersetzt: {reason}"),
                    ),
                }
            }
        }
    }

    // Removals: one entry at a time, bottom-up (children come after their folder in the
    // sorted result, so the reversed order removes children first).
    if incomplete {
        if mode == SyncMode::Mirror {
            note(
                &mut plan,
                "",
                "Der Vergleich ist unvollständig; es wird nichts gelöscht",
            );
        }
    } else {
        for (rel, info, listed) in removals.into_iter().rev() {
            if job.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if info.kind == EntryKind::Dir && !listed {
                note(
                    &mut plan,
                    rel,
                    "Der Ordnerinhalt wurde nicht erfasst; nicht gelöscht",
                );
                continue;
            }
            let path = join_rel(target, rel)?;
            match capture(path.path(), false, None) {
                Ok(version) => {
                    let planned: Vec<&str> = children
                        .get(rel)
                        .map(|set| set.iter().copied().collect())
                        .unwrap_or_default();
                    if version.kind != info.kind {
                        note(
                            &mut plan,
                            rel,
                            "Der Eintrag hat seinen Typ geändert; nicht gelöscht",
                        );
                    } else if version.kind == EntryKind::Dir
                        && version
                            .children
                            .iter()
                            .map(String::as_str)
                            .ne(planned.iter().copied())
                    {
                        note(
                            &mut plan,
                            rel,
                            "Der Ordnerinhalt hat sich seit dem Vergleich geändert; nicht gelöscht",
                        );
                    } else {
                        plan.actions.push(SyncAction::Trash {
                            path,
                            expected: version,
                        });
                    }
                }
                Err(reason) => note(
                    &mut plan,
                    rel,
                    &format!("Eintrag nicht prüfbar, nicht gelöscht: {reason}"),
                ),
            }
        }
    }
    plan.recount();
    Ok(plan)
}

struct SkipConflicts;
impl ConflictResolver for SkipConflicts {
    fn resolve(&self, _: &Conflict) -> Resolution {
        Resolution::Skip
    }
}

/// For planned overwrites: without a resolver the confirmed plan counts as the answer
/// ("overwrite"); with one it is asked and may also skip, keep both or cancel.
struct PlannedOverwrite<'a> {
    inner: Option<&'a dyn ConflictResolver>,
    remembered: Mutex<Option<Resolution>>,
}
impl ConflictResolver for PlannedOverwrite<'_> {
    fn resolve(&self, conflict: &Conflict) -> Resolution {
        let mut remembered = self.remembered.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(choice) = *remembered {
            return choice;
        }
        let choice = self
            .inner
            .map_or(Resolution::Overwrite, |r| r.resolve(conflict));
        let (answer, keep) = match choice {
            Resolution::OverwriteAll => (Resolution::Overwrite, true),
            Resolution::SkipAll => (Resolution::Skip, true),
            Resolution::KeepBothAll => (Resolution::KeepBoth, true),
            other => (other, false),
        };
        if keep {
            *remembered = Some(answer);
        }
        answer
    }
}

struct KeepBoth;
impl ConflictResolver for KeepBoth {
    fn resolve(&self, _: &Conflict) -> Resolution {
        Resolution::KeepBoth
    }
}

/// Removes a planned folder that the child actions have emptied. Only `rmdir` is used,
/// never the trash: it can remove nothing but an empty folder (a link or file in its
/// place is refused with `ENOTDIR`), so an entry that appears between the check and the
/// removal makes it fail instead of moving unplanned content away.
fn remove_empty_dir(path: &VPath, report: &mut OpReport, job: &JobHandle) {
    #[cfg(test)]
    hooks::fire(&hooks::BEFORE_RMDIR, path.path());
    match std::fs::remove_dir(path.path()) {
        Ok(()) => {
            job.emit(ProgressEvent::ItemDone(path.clone()));
            report.done.push(path.clone());
        }
        Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => skip(
            report,
            path,
            "Im Ordner liegt ein neuer Eintrag; der Ordner bleibt",
            job,
        ),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            skip(
                report,
                path,
                "Der Ordner wurde seit der Prüfung ausgetauscht",
                job,
            )
        }
        Err(e) => {
            job.emit(ProgressEvent::ItemFailed(path.clone()));
            report
                .failed
                .push((path.clone(), from_io("Ordner entfernen", path.path(), e)));
        }
    }
}

/// A hidden folder next to the target that holds the new file and, after the exchange,
/// the displaced one. Dropping it removes what is known to be ours (never recursive),
/// unless it was disarmed because it may hold a file of the user.
struct Staging {
    dir: PathBuf,
    files: Vec<PathBuf>,
    armed: bool,
}

impl Staging {
    fn create(parent: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let dir = parent.join(format!(
                ".dualfile-part-sync-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        dir,
                        files: Vec::new(),
                        armed: true,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other(
            "Kein freier Hilfsordnername gefunden",
        ))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for file in &self.files {
            let _ = std::fs::remove_file(file);
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Swaps two paths atomically (`renameat2` with `RENAME_EXCHANGE`).
#[cfg(target_os = "linux")]
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE).map_err(std::io::Error::from)
}
#[cfg(not(target_os = "linux"))]
fn exchange(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn exchange_unsupported(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Unsupported
        || matches!(
            e.raw_os_error(),
            Some(code) if code == rustix::io::Errno::INVAL.raw_os_error()
                || code == rustix::io::Errno::NOSYS.raw_os_error()
                || code == rustix::io::Errno::NOTSUP.raw_os_error()
        )
}

const MSG_NO_EXCHANGE: &str =
    "Das Dateisystem kann Dateien nicht atomar austauschen; nicht ersetzt";

/// Replaces `to` by a copy of `from` without a gap between check and replacement:
/// (a) the copy is written into a staging folder next to the target, (b) copy and
/// target are exchanged atomically, (c) the file that the exchange moved out is checked
/// against the planned version. (d) Match: it is deleted. (e) Mismatch: the exchange is
/// undone at once, so whatever stood at the target in the meantime is still there.
/// Where the file system cannot exchange, nothing is replaced.
#[allow(clippy::too_many_arguments)]
fn replace_by_exchange(
    src_vfs: &dyn Vfs,
    dst_vfs: &dyn Vfs,
    from: &VPath,
    to: &VPath,
    parent: &VPath,
    expected: &TargetVersion,
    job: &JobHandle,
    report: &mut OpReport,
) {
    let io_failed = |report: &mut OpReport, op: &'static str, path: &Path, e: std::io::Error| {
        job.emit(ProgressEvent::ItemFailed(to.clone()));
        report.failed.push((to.clone(), from_io(op, path, e)));
    };
    let Some(name) = to.path().file_name() else {
        skip(report, to, "Ungültiges Ziel", job);
        return;
    };
    let mut staging = match Staging::create(parent.path()) {
        Ok(s) => s,
        Err(e) => return io_failed(report, "Hilfsordner anlegen", parent.path(), e),
    };
    let staged = staging.dir.join(name);
    // The staging folder must sit in the very folder that was planned.
    match std::fs::metadata(parent.path()) {
        Ok(meta) if Some(ids(&meta)) == expected.parent => {}
        _ => {
            skip(
                report,
                to,
                "Der übergeordnete Ordner wurde seit der Planung ausgetauscht",
                job,
            );
            return;
        }
    }
    // Probe with two empty files: no copying of a big file to find out that it cannot work.
    let (probe_a, probe_b) = (staging.dir.join(".probe-a"), staging.dir.join(".probe-b"));
    staging.files.extend([probe_a.clone(), probe_b.clone()]);
    let probe = std::fs::File::create_new(&probe_a)
        .and_then(|_| std::fs::File::create_new(&probe_b))
        .and_then(|_| exchange(&probe_a, &probe_b));
    if let Err(e) = probe {
        if exchange_unsupported(&e) {
            skip(report, to, MSG_NO_EXCHANGE, job);
        } else {
            io_failed(report, "Hilfsdatei anlegen", &probe_a, e);
        }
        return;
    }
    let _ = std::fs::remove_file(&probe_a);
    let _ = std::fs::remove_file(&probe_b);
    staging.files = vec![staged.clone()];

    let Ok(staging_vpath) = VPath::local(staging.dir.clone()) else {
        skip(report, to, "Ungültiges Ziel", job);
        return;
    };
    let skip_all = SkipConflicts;
    let part = ops::copy(
        src_vfs,
        std::slice::from_ref(from),
        dst_vfs,
        &staging_vpath,
        Some(&skip_all),
        job,
    );
    if part.cancelled || !part.failed.is_empty() || !part.skipped.is_empty() || part.done.len() != 1
    {
        merge(report, part);
        return;
    }

    #[cfg(test)]
    hooks::fire(&hooks::BEFORE_EXCHANGE, to.path());
    if let Err(e) = exchange(&staged, to.path()) {
        if e.kind() == std::io::ErrorKind::NotFound {
            skip(report, to, "Das Ziel ist verschwunden", job);
        } else if exchange_unsupported(&e) {
            skip(report, to, MSG_NO_EXCHANGE, job);
        } else {
            io_failed(report, "Datei austauschen", to.path(), e);
        }
        return;
    }
    #[cfg(test)]
    hooks::fire(&hooks::AFTER_EXCHANGE, &staged);
    if let Err(reason) = verify_in(&staged, expected, false) {
        match exchange(&staged, to.path()) {
            Ok(()) => skip(
                report,
                to,
                &format!("Ziel hat sich geändert: {reason}"),
                job,
            ),
            Err(e) => {
                // The displaced file may be the user's: keep the staging folder.
                staging.armed = false;
                job.emit(ProgressEvent::ItemFailed(to.clone()));
                report.failed.push((
                    to.clone(),
                    Error::Io {
                        op: "Austausch rückgängig machen (die verdrängte Datei liegt noch im Hilfsordner)",
                        path: staged.clone(),
                        source: e,
                    },
                ));
            }
        }
        return;
    }
    if let Err(e) = std::fs::remove_file(&staged) {
        report.warnings.push((
            to.clone(),
            format!("Hilfsdatei mit der alten Version konnte nicht entfernt werden: {e}"),
        ));
    }
    merge(report, part);
}

fn skip(report: &mut OpReport, path: &VPath, reason: &str, job: &JobHandle) {
    report.skipped.push(path.clone());
    report
        .warnings
        .push((path.clone(), format!("Übersprungen: {reason}")));
    job.emit(ProgressEvent::ItemSkipped(path.clone()));
}

fn merge(report: &mut OpReport, part: OpReport) {
    report.done.extend(part.done);
    report.failed.extend(part.failed);
    report.skipped.extend(part.skipped);
    report.warnings.extend(part.warnings);
    report.cancelled |= part.cancelled;
}

/// Runs a confirmed plan. Only actions of the plan are executed; each is re-checked
/// against the target as it is now and skipped (with a reason in `warnings`) when it no
/// longer matches. A wrong token does nothing. The target must be a local file system.
pub fn run_sync(
    src_vfs: &dyn Vfs,
    dst_vfs: &dyn Vfs,
    plan: &SyncPlan,
    confirmed_token: u128,
    resolver: Option<&dyn ConflictResolver>,
    job: &JobHandle,
) -> OpReport {
    let mut report = OpReport::default();
    if plan.token != confirmed_token {
        report.failed.push((
            plan.target.clone(),
            Error::InvalidPath(
                "Bestätigungstoken stimmt nicht mit dem Synchronisierungsplan überein".into(),
            ),
        ));
        return report;
    }
    if dst_vfs.is_read_only() || plan.target.scheme() != Scheme::Local {
        let message = if plan.target.scheme().is_remote() {
            MSG_REMOTE_TARGET
        } else {
            MSG_READ_ONLY
        };
        for action in &plan.actions {
            report
                .failed
                .push((action.target_path().clone(), Error::Unsupported(message)));
        }
        return report;
    }
    let skip_all = SkipConflicts;
    let overwrite = PlannedOverwrite {
        inner: resolver,
        remembered: Mutex::new(None),
    };
    for action in &plan.actions {
        if job.is_cancelled() {
            report.cancelled = true;
            break;
        }
        match action {
            SyncAction::MakeDir { path } => {
                if !dst_vfs.is_local() {
                    skip(&mut report, path, "Das Ziel ist nicht prüfbar", job);
                    continue;
                }
                match std::fs::symlink_metadata(path.path()) {
                    Ok(meta) if meta.is_dir() => report.done.push(path.clone()),
                    Ok(_) => skip(
                        &mut report,
                        path,
                        "Am Ziel liegt inzwischen etwas anderes",
                        job,
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        match dst_vfs.mkdir(path) {
                            Ok(()) => {
                                job.emit(ProgressEvent::ItemDone(path.clone()));
                                report.done.push(path.clone());
                            }
                            Err(error) => {
                                job.emit(ProgressEvent::ItemFailed(path.clone()));
                                report.failed.push((path.clone(), error));
                            }
                        }
                    }
                    Err(e) => skip(&mut report, path, &e.to_string(), job),
                }
            }
            SyncAction::Copy {
                from,
                to,
                source_kind,
                overwrite: replace,
                expected_target,
                ..
            } => {
                if !dst_vfs.is_local() {
                    skip(&mut report, to, "Das Ziel ist nicht prüfbar", job);
                    continue;
                }
                let Some(parent) = to.parent() else {
                    skip(&mut report, to, "Ungültiges Ziel", job);
                    continue;
                };
                if from.path().file_name() != to.path().file_name() {
                    skip(
                        &mut report,
                        to,
                        "Quell- und Zielname passen nicht zusammen",
                        job,
                    );
                    continue;
                }
                // Never hand a folder to the copy: it would copy a whole tree that is
                // not itemised in the plan.
                match src_vfs.stat(from) {
                    Ok(entry) if entry.kind == *source_kind && entry.kind != EntryKind::Dir => {}
                    Ok(_) => {
                        skip(&mut report, from, "Die Quelle hat ihren Typ geändert", job);
                        continue;
                    }
                    Err(e) => {
                        skip(&mut report, from, &format!("Quelle nicht lesbar: {e}"), job);
                        continue;
                    }
                }
                let part = if *replace {
                    let Some(expected) = expected_target else {
                        skip(
                            &mut report,
                            to,
                            "Kein Versionsnachweis für das Ziel im Plan",
                            job,
                        );
                        continue;
                    };
                    if let Err(reason) = verify(to.path(), expected) {
                        skip(&mut report, to, &reason, job);
                        continue;
                    }
                    let (Ok(source_entry), Ok(target_entry)) =
                        (src_vfs.stat(from), dst_vfs.stat(to))
                    else {
                        skip(&mut report, to, "Quelle oder Ziel nicht lesbar", job);
                        continue;
                    };
                    let answer = overwrite.resolve(&Conflict {
                        source: source_entry,
                        destination: target_entry,
                        source_path: from.clone(),
                        destination_path: to.clone(),
                    });
                    match answer {
                        Resolution::Skip | Resolution::SkipAll => {
                            report.skipped.push(from.clone());
                            job.emit(ProgressEvent::ItemSkipped(from.clone()));
                            continue;
                        }
                        Resolution::Cancel => {
                            report.cancelled = true;
                            break;
                        }
                        // The target is not touched: the copy picks a free name.
                        Resolution::KeepBoth | Resolution::KeepBothAll => ops::copy(
                            src_vfs,
                            std::slice::from_ref(from),
                            dst_vfs,
                            &parent,
                            Some(&KeepBoth),
                            job,
                        ),
                        Resolution::Overwrite | Resolution::OverwriteAll => {
                            replace_by_exchange(
                                src_vfs,
                                dst_vfs,
                                from,
                                to,
                                &parent,
                                expected,
                                job,
                                &mut report,
                            );
                            continue;
                        }
                    }
                } else {
                    match std::fs::symlink_metadata(to.path()) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            skip(&mut report, to, &e.to_string(), job);
                            continue;
                        }
                        Ok(_) => {
                            skip(&mut report, to, "Am Ziel liegt inzwischen etwas", job);
                            continue;
                        }
                    }
                    ops::copy(
                        src_vfs,
                        std::slice::from_ref(from),
                        dst_vfs,
                        &parent,
                        Some(&skip_all),
                        job,
                    )
                };
                merge(&mut report, part);
            }
            SyncAction::Trash { path, expected } => {
                if !dst_vfs.is_local() {
                    skip(&mut report, path, "Das Ziel ist nicht prüfbar", job);
                    continue;
                }
                if let Err(reason) = verify(path.path(), expected) {
                    skip(&mut report, path, &reason, job);
                    continue;
                }
                if expected.kind == EntryKind::Dir {
                    remove_empty_dir(path, &mut report, job);
                    continue;
                }
                match dst_vfs.trash(path) {
                    Ok(()) => {
                        job.emit(ProgressEvent::ItemDone(path.clone()));
                        report.done.push(path.clone());
                    }
                    Err(error) => {
                        job.emit(ProgressEvent::ItemFailed(path.clone()));
                        report.failed.push((path.clone(), error));
                    }
                }
            }
        }
        if report.cancelled {
            break;
        }
    }
    report
}

#[cfg(test)]
pub(super) mod hooks {
    //! Test-only callbacks that let a test change the file system at an exact point of
    //! `run_sync`, to prove that a swap in the gap between check and action loses nothing.
    use std::cell::RefCell;
    use std::path::Path;
    pub(in crate::compare) type Hook = Box<dyn Fn(&Path)>;
    thread_local! {
        /// Called with the target path right before the exchange.
        pub(in crate::compare) static BEFORE_EXCHANGE: RefCell<Option<Hook>> = const { RefCell::new(None) };
        /// Called with the displaced (old) file right after the exchange.
        pub(in crate::compare) static AFTER_EXCHANGE: RefCell<Option<Hook>> = const { RefCell::new(None) };
        /// Called with the folder right before it is removed.
        pub(in crate::compare) static BEFORE_RMDIR: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }
    pub(in crate::compare) fn fire(
        slot: &'static std::thread::LocalKey<RefCell<Option<Hook>>>,
        path: &Path,
    ) {
        slot.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook(path);
            }
        });
    }
}
