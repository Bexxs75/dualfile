//! Similar-image DTOs and selection checks. No filesystem mutations here.
use crate::{
    convert::UiError,
    space::{gone, invalid, KeepChoice, ScanProgress, SkippedDto, Slot},
};
use dualfile_core::similar::{suggest_keep_similar, SimilarEntry, SimilarKeepRule, SimilarResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Condvar, Mutex},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarOptionsDto {
    pub min_size: u64,
    pub max_distance: f64,
}
impl SimilarOptionsDto {
    pub fn validate(&self) -> Result<(), UiError> {
        if !self.max_distance.is_finite()
            || !(0.0..=16.0).contains(&self.max_distance)
            || self.max_distance.fract() != 0.0
        {
            Err(invalid())
        } else {
            Ok(())
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SimilarRuleDto {
    LargestResolution,
    LargestFile,
    Oldest,
    Newest,
    ShortestPath,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashChoice {
    pub group_index: usize,
    pub trash_paths: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    path: String,
    size: u64,
    modified: Option<i64>,
    width: u32,
    height: u32,
    hash: String,
    distance_to_reference: u32,
    protected: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    group_index: usize,
    reclaimable_bytes: u64,
    entries: Vec<EntryDto>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarPageDto {
    ready: bool,
    progress: ScanProgress,
    total: usize,
    images: usize,
    reclaimable_bytes: u64,
    groups: Vec<GroupDto>,
    incomplete: bool,
    cancelled: bool,
    issues: Vec<(String, String)>,
}
pub fn page(
    slot: &Slot<SimilarResult>,
    offset: usize,
    limit: usize,
) -> Result<SimilarPageDto, UiError> {
    if limit == 0 {
        return Err(invalid());
    }
    if let Some(e) = &slot.error {
        return Err(e.clone());
    }
    let mut p = SimilarPageDto {
        ready: false,
        progress: slot.progress.clone(),
        total: 0,
        images: 0,
        reclaimable_bytes: 0,
        groups: vec![],
        incomplete: false,
        cancelled: false,
        issues: vec![],
    };
    if let Some(r) = &slot.result {
        p.ready = true;
        p.total = r.groups.len();
        p.images = r.groups.iter().map(|g| g.entries.len()).sum();
        p.reclaimable_bytes = r
            .groups
            .iter()
            .fold(0u64, |n, g| n.saturating_add(g.reclaimable_bytes));
        p.incomplete = r.incomplete;
        p.cancelled = r.cancelled;
        p.issues = r
            .skipped
            .iter()
            .map(|(p, e)| (p.clone(), crate::convert::similar_issue(*e).into()))
            .collect();
        p.groups = r
            .groups
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit.min(100))
            .map(|(i, g)| GroupDto {
                group_index: i,
                reclaimable_bytes: g.reclaimable_bytes,
                entries: g
                    .entries
                    .iter()
                    .map(|e| EntryDto {
                        path: e.path.to_string_lossy().into_owned(),
                        size: e.size,
                        modified: e.modified.map(crate::convert::millis),
                        width: e.width,
                        height: e.height,
                        hash: format!("{:016x}", e.hash),
                        distance_to_reference: e.distance_to_reference,
                        protected: e.hard_links > 1 || !e.linked_paths.is_empty(),
                    })
                    .collect(),
            })
            .collect();
    }
    Ok(p)
}
pub fn suggest(r: &SimilarResult, rule: SimilarRuleDto) -> Vec<KeepChoice> {
    let rule = match rule {
        SimilarRuleDto::LargestResolution => SimilarKeepRule::LargestResolution,
        SimilarRuleDto::LargestFile => SimilarKeepRule::LargestFile,
        SimilarRuleDto::Oldest => SimilarKeepRule::Oldest,
        SimilarRuleDto::Newest => SimilarKeepRule::Newest,
        SimilarRuleDto::ShortestPath => SimilarKeepRule::ShortestPath,
    };
    r.groups
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            suggest_keep_similar(g, rule).map(|s| KeepChoice {
                group_index: i,
                keep_path: s.keep.path.to_string_lossy().into_owned(),
            })
        })
        .collect()
}
pub fn changed(e: &SimilarEntry) -> bool {
    !crate::space::unchanged(&e.path, e.size, e.modified)
}
pub fn reason(e: &SimilarEntry) -> Option<&'static str> {
    if e.hard_links > 1 || !e.linked_paths.is_empty() || crate::space::current_links(&e.path) > 1 {
        Some("hardlink")
    } else if changed(e) {
        Some("changed")
    } else {
        None
    }
}
pub fn candidates(
    r: &SimilarResult,
    choices: &[TrashChoice],
) -> Result<(Vec<SimilarEntry>, Vec<SkippedDto>), UiError> {
    let mut groups = HashSet::new();
    let mut paths = HashSet::new();
    let mut selected = vec![];
    for c in choices {
        if !groups.insert(c.group_index) {
            return Err(invalid());
        }
        let g = r.groups.get(c.group_index).ok_or_else(invalid)?;
        if c.trash_paths.len() >= g.entries.len() {
            return Err(invalid());
        }
        for path in &c.trash_paths {
            if !paths.insert(path) {
                return Err(invalid());
            }
            let e = g
                .entries
                .iter()
                .find(|e| e.path == Path::new(path))
                .ok_or_else(invalid)?;
            if e.hard_links > 1 || !e.linked_paths.is_empty() {
                return Err(invalid());
            }
            selected.push(e.clone());
        }
    }
    let mut kept = vec![];
    let mut skipped = vec![];
    for e in selected {
        if let Some(reason) = reason(&e) {
            skipped.push(SkippedDto {
                path: e.path.to_string_lossy().into_owned(),
                reason: reason.into(),
            });
        } else {
            kept.push(e);
        }
    }
    Ok((kept, skipped))
}
/// One gate across all sessions. Waiting and running readers observe session cancellation.
#[derive(Default)]
pub struct ThumbnailGate {
    active: Mutex<usize>,
    wake: Condvar,
}
pub struct Permit<'a>(&'a ThumbnailGate);
impl ThumbnailGate {
    pub fn acquire(&self, job: &dualfile_core::ops::JobHandle) -> Result<Permit<'_>, UiError> {
        let mut active = self.active.lock().unwrap();
        loop {
            if job.is_cancelled() {
                return Err(gone());
            }
            if *active < 2 {
                *active += 1;
                return Ok(Permit(self));
            }
            active = self
                .wake
                .wait_timeout(active, std::time::Duration::from_millis(25))
                .unwrap()
                .0;
        }
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap() -= 1;
        self.0.wake.notify_one();
    }
}
#[derive(Serialize)]
pub struct ThumbnailDto {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::similar::SimilarGroup;
    pub fn fixture() -> (tempfile::TempDir, SimilarResult) {
        let d = tempfile::tempdir().unwrap();
        let entries = ["a", "b", "c"]
            .iter()
            .map(|n| {
                let path = d.path().join(n);
                std::fs::write(&path, b"image").unwrap();
                SimilarEntry {
                    modified: std::fs::metadata(&path).unwrap().modified().ok(),
                    path,
                    size: 5,
                    hard_links: 1,
                    linked_paths: vec![],
                    width: 2,
                    height: 2,
                    hash: 0,
                    hash_hex: "0000000000000000".into(),
                    distance_to_reference: 0,
                }
            })
            .collect();
        (
            d,
            SimilarResult {
                groups: vec![SimilarGroup {
                    entries,
                    reclaimable_bytes: 10,
                }],
                ..Default::default()
            },
        )
    }
    fn choice(r: &SimilarResult, indices: &[usize]) -> TrashChoice {
        TrashChoice {
            group_index: 0,
            trash_paths: indices
                .iter()
                .map(|&i| r.groups[0].entries[i].path.to_string_lossy().into_owned())
                .collect(),
        }
    }
    #[test]
    fn selection_is_explicit_and_rejects_foreign_full_duplicate_and_protected_choices() {
        let (_d, mut r) = fixture();
        assert!(candidates(&r, &[]).unwrap().0.is_empty());
        assert_eq!(candidates(&r, &[choice(&r, &[0, 1])]).unwrap().0.len(), 2);
        for c in [
            vec![choice(&r, &[0, 1, 2])],
            vec![choice(&r, &[0, 0])],
            vec![choice(&r, &[0]), choice(&r, &[1])],
            vec![TrashChoice {
                group_index: 0,
                trash_paths: vec!["/foreign".into()],
            }],
            vec![TrashChoice {
                group_index: 99,
                trash_paths: vec![],
            }],
        ] {
            assert_eq!(candidates(&r, &c).unwrap_err().kind, "invalidInput");
        }
        r.groups[0].entries[0].hard_links = 2;
        assert!(candidates(&r, &[choice(&r, &[0])]).is_err());
        r.groups[0].entries[0].hard_links = 1;
        r.groups[0].entries[0].linked_paths.push("/alias".into());
        assert!(candidates(&r, &[choice(&r, &[0])]).is_err());
    }
    #[test]
    fn changed_missing_and_new_hardlinks_are_skipped() {
        let (d, r) = fixture();
        let c = choice(&r, &[0, 1]);
        std::fs::write(&r.groups[0].entries[0].path, b"changed length").unwrap();
        std::fs::remove_file(&r.groups[0].entries[1].path).unwrap();
        let (items, skipped) = candidates(&r, &[c]).unwrap();
        assert!(items.is_empty());
        assert_eq!(skipped.len(), 2);
        assert!(r.groups[0].entries[2].path.exists());
        std::fs::hard_link(&r.groups[0].entries[2].path, d.path().join("alias")).unwrap();
        assert_eq!(reason(&r.groups[0].entries[2]), Some("hardlink"));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_leaf_and_ancestor_are_refused() {
        let (d, r) = fixture();
        let e = &r.groups[0].entries[0];
        std::fs::remove_file(&e.path).unwrap();
        std::os::unix::fs::symlink(&r.groups[0].entries[1].path, &e.path).unwrap();
        assert!(changed(e));
        let alias = d.path().join("dir");
        std::os::unix::fs::symlink(d.path(), &alias).unwrap();
        let mut e = r.groups[0].entries[1].clone();
        e.path = alias.join("b");
        assert!(changed(&e));
    }
    #[test]
    fn pages_distance_hex_and_suggestions() {
        let (_d, mut r) = fixture();
        r.groups = vec![r.groups[0].clone(); 105];
        let slot = Slot {
            result: Some(r),
            ..Default::default()
        };
        let p = page(&slot, 0, 500).unwrap();
        assert_eq!(p.groups.len(), 100);
        assert_eq!(p.images, 315);
        assert_eq!(p.groups[0].entries[0].hash, "0000000000000000");
        assert_eq!(page(&slot, 100, 50).unwrap().groups.len(), 5);
        assert!(page(&slot, usize::MAX, 50).unwrap().groups.is_empty());
        assert!(page(&slot, 0, 0).is_err());
        for n in [0.0, 6.0, 16.0] {
            assert!(SimilarOptionsDto {
                min_size: 0,
                max_distance: n
            }
            .validate()
            .is_ok());
        }
        for max_distance in [-1.0, 17.0, 1.5, f64::NAN] {
            assert!(SimilarOptionsDto {
                min_size: 0,
                max_distance
            }
            .validate()
            .is_err());
        }
        assert_eq!(
            suggest(
                slot.result.as_ref().unwrap(),
                SimilarRuleDto::LargestResolution
            )
            .len(),
            105
        );
    }
    #[test]
    fn gate_limits_two_and_cancelled_waiter_leaves() {
        let gate = ThumbnailGate::default();
        let job = dualfile_core::ops::JobHandle::silent();
        let a = gate.acquire(&job).unwrap();
        let b = gate.acquire(&job).unwrap();
        assert_eq!(*gate.active.lock().unwrap(), 2);
        std::thread::scope(|scope| {
            let j = job.clone();
            let gate = &gate;
            let waiter = scope.spawn(move || gate.acquire(&j).is_err());
            job.cancel();
            assert!(waiter.join().unwrap());
        });
        drop(a);
        drop(b);
        assert_eq!(*gate.active.lock().unwrap(), 0);
    }
    #[test]
    fn held_limit_counts_failures_and_close_cancels_late_results() {
        let store = crate::space::ScanStore::<SimilarResult>::default();
        let a = store.start(|_| Ok(1)).unwrap();
        let slot = store.get(a).unwrap();
        slot.lock().unwrap().error = Some(invalid());
        store.start(|_| Ok(2)).unwrap();
        assert!(store.start(|_| Ok(3)).is_err());
        store.close(a).unwrap();
        assert!(slot.lock().unwrap().thumbnail_cancel.is_cancelled());
        slot.lock().unwrap().result = Some(SimilarResult::default());
        assert!(store.get(a).is_err());
        assert!(store.start(|_| Ok(3)).is_ok());
    }
    #[test]
    fn snapshot_is_consumed_once_under_parallel_selection() {
        let (_d, r) = fixture();
        let c = choice(&r, &[0]);
        let slot = Mutex::new(Slot {
            result: Some(r),
            ..Default::default()
        });
        std::thread::scope(|s| {
            let run = || {
                let mut held = slot.lock().unwrap();
                let Some(r) = held.result.as_ref() else {
                    return false;
                };
                candidates(r, std::slice::from_ref(&c)).unwrap();
                held.result.take();
                true
            };
            let a = s.spawn(run);
            let b = s.spawn(run);
            assert_ne!(a.join().unwrap(), b.join().unwrap());
        });
    }
}
