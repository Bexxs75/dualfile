//! Bounded scan sessions. Closing a running session cannot resurrect its result.
use crate::convert::UiError;
use dualfile_core::{
    dupes::{suggest_keep, DupeResult, KeepRule},
    usage::{UsageNode, UsageResult, UsageTotals},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

pub fn invalid() -> UiError {
    UiError::new("invalidInput", "Ungültige Scan-Auswahl")
}
pub fn gone() -> UiError {
    UiError::new("notFound", "Das Messergebnis ist nicht (mehr) vorhanden")
}
#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub stage: String,
    pub entries: u64,
    pub bytes: u64,
    pub total: Option<u64>,
}
pub struct Slot<T> {
    pub result: Option<T>,
    pub thumbnail_cancel: dualfile_core::ops::JobHandle,
    pub error: Option<UiError>,
    pub progress: ScanProgress,
    pub started: Instant,
    pub seconds: f64,
}
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            result: None,
            thumbnail_cancel: dualfile_core::ops::JobHandle::silent(),
            error: None,
            progress: ScanProgress::default(),
            started: Instant::now(),
            seconds: 0.0,
        }
    }
}
pub struct ScanStore<T> {
    slots: Mutex<HashMap<u64, Arc<Mutex<Slot<T>>>>>,
}
impl<T> Default for ScanStore<T> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }
}
impl<T> ScanStore<T> {
    // Submission holds this lock until the job id is installed, including queued jobs.
    pub fn start(
        &self,
        submit: impl FnOnce(Arc<Mutex<Slot<T>>>) -> Result<u64, UiError>,
    ) -> Result<u64, UiError> {
        let mut slots = self.slots.lock().unwrap();
        if slots.len() >= 2 {
            return Err(UiError::new(
                "invalidInput",
                "Es sind schon zwei Messungen offen",
            ));
        }
        let slot = Arc::new(Mutex::new(Slot::default()));
        let id = submit(slot.clone())?;
        slots.insert(id, slot);
        Ok(id)
    }
    pub fn get(&self, id: u64) -> Result<Arc<Mutex<Slot<T>>>, UiError> {
        self.slots
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(gone)
    }
    pub fn close(&self, id: u64) -> Result<(), UiError> {
        self.slots
            .lock()
            .unwrap()
            .remove(&id)
            .map(|slot| slot.lock().unwrap().thumbnail_cancel.cancel())
            .ok_or_else(gone)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOptionsDto {
    pub one_file_system: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesOptionsDto {
    pub min_size: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalsDto {
    pub size: u64,
    pub allocated: u64,
    pub files: u64,
    pub directories: u64,
}
impl From<UsageTotals> for TotalsDto {
    fn from(t: UsageTotals) -> Self {
        Self {
            size: t.size,
            allocated: t.allocated,
            files: t.files,
            directories: t.directories,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntryDto {
    pub path: String,
    pub totals: TotalsDto,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePageDto {
    pub ready: bool,
    pub progress: ScanProgress,
    pub seconds: f64,
    pub path: String,
    pub totals: Option<TotalsDto>,
    pub children: Vec<UsageEntryDto>,
    pub other: Option<TotalsDto>,
    pub issues: Vec<(String, String)>,
    pub skipped_count: u64,
    pub incomplete: bool,
}
fn find_node<'a>(node: &'a UsageNode, path: &Path) -> Option<&'a UsageNode> {
    if node.path == path {
        Some(node)
    } else {
        node.children.iter().find_map(|n| find_node(n, path))
    }
}
pub fn usage_page(slot: &Slot<UsageResult>, path: Option<&Path>) -> Result<UsagePageDto, UiError> {
    if let Some(e) = &slot.error {
        return Err(e.clone());
    }
    let mut page = UsagePageDto {
        ready: false,
        progress: slot.progress.clone(),
        seconds: slot.seconds,
        path: String::new(),
        totals: None,
        children: vec![],
        other: None,
        issues: vec![],
        skipped_count: 0,
        incomplete: false,
    };
    if let Some(r) = &slot.result {
        let n = match path {
            Some(p) => find_node(&r.root, p).ok_or_else(invalid)?,
            None => &r.root,
        };
        page.ready = true;
        page.path = n.path.to_string_lossy().into_owned();
        page.totals = Some(n.totals.into());
        page.other = n.other.map(Into::into);
        page.children = n
            .children
            .iter()
            .map(|n| UsageEntryDto {
                path: n.path.to_string_lossy().into_owned(),
                totals: n.totals.into(),
            })
            .collect();
        page.children.sort_by(|a, b| {
            b.totals
                .allocated
                .cmp(&a.totals.allocated)
                .then(a.path.cmp(&b.path))
        });
        page.issues = r
            .skipped
            .iter()
            .map(|(p, e)| (p.clone(), crate::convert::scan_issue(*e).into()))
            .collect();
        page.skipped_count = r.skipped_count;
        page.incomplete = r.incomplete || r.cancelled;
    }
    Ok(page)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepChoice {
    pub group_index: usize,
    pub keep_path: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepRuleDto {
    Oldest,
    Newest,
    ShortestPath,
    InFolder,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupeEntryDto {
    pub path: String,
    pub modified: Option<i64>,
    pub protected: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupeGroupDto {
    pub group_index: usize,
    pub size: u64,
    pub reclaimable_bytes: u64,
    pub entries: Vec<DupeEntryDto>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesPageDto {
    pub ready: bool,
    pub progress: ScanProgress,
    pub total: usize,
    pub reclaimable_bytes: u64,
    pub groups: Vec<DupeGroupDto>,
    pub incomplete: bool,
    pub cancelled: bool,
    pub issues: Vec<(String, String)>,
}
pub fn dupes_page(
    slot: &Slot<DupeResult>,
    offset: usize,
    limit: usize,
) -> Result<DupesPageDto, UiError> {
    if let Some(e) = &slot.error {
        return Err(e.clone());
    }
    let mut page = DupesPageDto {
        ready: false,
        progress: slot.progress.clone(),
        total: 0,
        reclaimable_bytes: 0,
        groups: vec![],
        incomplete: false,
        cancelled: false,
        issues: vec![],
    };
    if let Some(r) = &slot.result {
        page.ready = true;
        page.total = r.groups.len();
        page.reclaimable_bytes = r.groups.iter().map(|g| g.reclaimable_bytes).sum();
        page.incomplete = r.incomplete;
        page.cancelled = r.cancelled;
        page.issues = r
            .skipped
            .iter()
            .map(|(p, e)| (p.clone(), crate::convert::scan_issue(*e).into()))
            .collect();
        page.groups = r
            .groups
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit.clamp(1, 100))
            .map(|(i, g)| DupeGroupDto {
                group_index: i,
                size: g.size,
                reclaimable_bytes: g.reclaimable_bytes,
                entries: g
                    .entries
                    .iter()
                    .map(|e| DupeEntryDto {
                        path: e.path.to_string_lossy().into_owned(),
                        modified: e.modified.map(crate::convert::millis),
                        protected: e.hard_links > 1 || !e.linked_paths.is_empty(),
                    })
                    .collect(),
            })
            .collect();
    }
    Ok(page)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepSuggestionDto {
    #[serde(flatten)]
    pub choice: KeepChoice,
    pub redundant_count: usize,
    pub redundant_bytes: u64,
}
pub fn suggestions(
    result: &DupeResult,
    rule: KeepRuleDto,
    folder: PathBuf,
) -> Vec<KeepSuggestionDto> {
    result
        .groups
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            suggest_keep(
                g,
                match rule {
                    KeepRuleDto::Oldest => KeepRule::Oldest,
                    KeepRuleDto::Newest => KeepRule::Newest,
                    KeepRuleDto::ShortestPath => KeepRule::ShortestPath,
                    KeepRuleDto::InFolder => KeepRule::InFolder(folder.clone()),
                },
            )
            .map(|s| {
                let count = g
                    .entries
                    .iter()
                    .filter(|e| {
                        e.path != s.keep.path && e.hard_links <= 1 && e.linked_paths.is_empty()
                    })
                    .count();
                KeepSuggestionDto {
                    choice: KeepChoice {
                        group_index: i,
                        keep_path: s.keep.path.to_string_lossy().into_owned(),
                    },
                    redundant_count: count,
                    redundant_bytes: (count as u64).saturating_mul(g.size),
                }
            })
        })
        .collect()
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedDto {
    pub path: String,
    pub reason: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashResultDto {
    pub job_id: u64,
    pub skipped: Vec<SkippedDto>,
}
pub(crate) fn unchanged(path: &Path, size: u64, modified: Option<std::time::SystemTime>) -> bool {
    // Reject symlink components as well as leaf links; a replaced ancestor must not redirect trash.
    if path
        .ancestors()
        .any(|p| std::fs::symlink_metadata(p).map_or(true, |m| m.file_type().is_symlink()))
    {
        return false;
    }
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_file() && m.len() == size && modified.is_some() && m.modified().ok() == modified
    })
}
/// Recheck the keeper as well as the candidate immediately before each trash call.
pub fn candidate_reason(
    size: u64,
    entry: &dualfile_core::dupes::DupeEntry,
    keep: &dualfile_core::dupes::DupeEntry,
) -> Option<&'static str> {
    if entry.path == keep.path {
        return Some("keeperChanged");
    }
    if !unchanged(&keep.path, size, keep.modified) {
        return Some("keeperChanged");
    }
    if entry.hard_links > 1 || !entry.linked_paths.is_empty() || current_links(&entry.path) > 1 {
        return Some("hardlink");
    }
    if !unchanged(&entry.path, size, entry.modified) {
        return Some("changed");
    }
    None
}

/// Fail closed for a changed/missing keeper. Each group is accepted at most once.
pub fn trash_candidates(
    result: &DupeResult,
    choices: &[KeepChoice],
) -> Result<(Vec<String>, Vec<SkippedDto>), UiError> {
    let mut groups = HashSet::new();
    let mut paths = HashSet::new();
    let mut skipped = vec![];
    for c in choices {
        if !groups.insert(c.group_index) {
            return Err(invalid());
        }
        let g = result.groups.get(c.group_index).ok_or_else(invalid)?;
        let keep = g
            .entries
            .iter()
            .find(|e| e.path == Path::new(&c.keep_path))
            .ok_or_else(invalid)?;
        for e in &g.entries {
            if e.path == keep.path {
                continue;
            }
            let reason = candidate_reason(g.size, e, keep);
            let path = e.path.to_string_lossy().into_owned();
            if let Some(reason) = reason {
                skipped.push(SkippedDto {
                    path,
                    reason: reason.into(),
                });
            } else {
                paths.insert(path);
            }
        }
    }
    let keepers: HashSet<&str> = choices.iter().map(|c| c.keep_path.as_str()).collect();
    let mut paths: Vec<_> = paths
        .into_iter()
        .filter(|p| !keepers.contains(p.as_str()))
        .collect();
    paths.sort();
    Ok((paths, skipped))
}
pub(crate) fn current_links(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).map_or(0, |m| m.nlink())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::dupes::{DupeEntry, DupeGroup};
    fn fixture() -> (tempfile::TempDir, DupeResult) {
        let dir = tempfile::tempdir().unwrap();
        let entries = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, b"same").unwrap();
                DupeEntry {
                    modified: std::fs::metadata(&path).unwrap().modified().ok(),
                    path,
                    linked_paths: vec![],
                    hard_links: 1,
                }
            })
            .collect();
        (
            dir,
            DupeResult {
                groups: vec![DupeGroup {
                    hash: "hash".into(),
                    size: 4,
                    entries,
                    reclaimable_bytes: 8,
                }],
                ..Default::default()
            },
        )
    }
    fn choice(r: &DupeResult) -> KeepChoice {
        KeepChoice {
            group_index: 0,
            keep_path: r.groups[0].entries[0].path.to_string_lossy().into_owned(),
        }
    }
    #[test]
    fn held_limit_and_close_running_cannot_resurrect() {
        let store = ScanStore::<u8>::default();
        let mut worker = None;
        store
            .start(|slot| {
                worker = Some(slot);
                Ok(1)
            })
            .unwrap();
        store.start(|_| Ok(2)).unwrap();
        assert_eq!(store.start(|_| Ok(3)).unwrap_err().kind, "invalidInput");
        store.close(1).unwrap();
        worker.unwrap().lock().unwrap().result = Some(7);
        assert!(store.get(1).is_err());
        store.start(|_| Ok(3)).unwrap();
    }
    #[test]
    fn untrusted_keeper_and_repeated_group_rejected() {
        let (_d, r) = fixture();
        let mut c = choice(&r);
        c.keep_path = "/outside".into();
        assert_eq!(trash_candidates(&r, &[c]).unwrap_err().kind, "invalidInput");
        assert!(trash_candidates(&r, &[choice(&r), choice(&r)]).is_err());
    }
    #[test]
    fn one_kept_and_changed_files_skipped() {
        let (_d, r) = fixture();
        let c = choice(&r);
        let (paths, skipped) = trash_candidates(&r, std::slice::from_ref(&c)).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(!paths.contains(&c.keep_path));
        assert!(skipped.is_empty());
        std::fs::write(&r.groups[0].entries[1].path, b"changed").unwrap();
        let (paths, skipped) = trash_candidates(&r, &[c]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(skipped[0].reason, "changed");
    }
    #[test]
    fn missing_keeper_protects_whole_group() {
        let (_d, r) = fixture();
        std::fs::remove_file(&r.groups[0].entries[0].path).unwrap();
        let (paths, skipped) = trash_candidates(&r, &[choice(&r)]).unwrap();
        assert!(paths.is_empty());
        assert_eq!(skipped.len(), 2);
    }
    #[test]
    fn changed_timestamp_is_skipped_even_with_same_size() {
        let (_d, r) = fixture();
        let file = std::fs::File::options()
            .write(true)
            .open(&r.groups[0].entries[1].path)
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        let (paths, skipped) = trash_candidates(&r, &[choice(&r)]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(skipped[0].reason, "changed");
    }
    #[test]
    fn recorded_aliases_are_never_redundant() {
        let (_d, mut r) = fixture();
        r.groups[0].entries[1]
            .linked_paths
            .push(PathBuf::from("/alias"));
        let (paths, skipped) = trash_candidates(&r, &[choice(&r)]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(skipped[0].reason, "hardlink");
    }
    #[cfg(unix)]
    #[test]
    fn newly_created_hardlink_sibling_is_never_redundant() {
        let (_d, r) = fixture();
        let a = &r.groups[0].entries[0].path;
        let b = &r.groups[0].entries[1].path;
        std::fs::remove_file(b).unwrap();
        std::fs::hard_link(a, b).unwrap();
        let (paths, skipped) = trash_candidates(&r, &[choice(&r)]).unwrap();
        assert!(!paths.contains(&b.to_string_lossy().into_owned()));
        assert_eq!(skipped[0].reason, "hardlink");
    }
    #[cfg(unix)]
    #[test]
    fn symlink_replacement_is_skipped() {
        let (_d, r) = fixture();
        let a = &r.groups[0].entries[0].path;
        let b = &r.groups[0].entries[1].path;
        std::fs::remove_file(b).unwrap();
        std::os::unix::fs::symlink(a, b).unwrap();
        let (paths, skipped) = trash_candidates(&r, &[choice(&r)]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(skipped[0].reason, "changed");
    }
    #[test]
    fn backend_keep_rules_and_ties_delegate_to_core() {
        let (_d, mut r) = fixture();
        for e in &mut r.groups[0].entries {
            e.modified = Some(std::time::UNIX_EPOCH);
        }
        for rule in [
            KeepRuleDto::Oldest,
            KeepRuleDto::Newest,
            KeepRuleDto::ShortestPath,
            KeepRuleDto::InFolder,
        ] {
            let choices = suggestions(&r, rule, PathBuf::from("/unrelated"));
            assert_eq!(choices[0].choice.keep_path, choice(&r).keep_path);
            assert_eq!(choices[0].redundant_count, 2);
        }
    }
    #[test]
    fn pagination_and_membership_are_bounded() {
        let (_d, r) = fixture();
        let slot = Slot {
            result: Some(r),
            ..Default::default()
        };
        assert_eq!(dupes_page(&slot, 0, 1000).unwrap().groups.len(), 1);
        assert!(dupes_page(&slot, usize::MAX, 1000)
            .unwrap()
            .groups
            .is_empty());
        let usage = Slot {
            result: Some(UsageResult {
                root: UsageNode {
                    path: "/chosen".into(),
                    totals: UsageTotals::default(),
                    children: vec![],
                    other: None,
                },
                skipped: vec![],
                skipped_count: 0,
                incomplete: false,
                cancelled: false,
            }),
            ..Default::default()
        };
        assert!(usage_page(&usage, Some(Path::new("/outside"))).is_err());
    }
}
