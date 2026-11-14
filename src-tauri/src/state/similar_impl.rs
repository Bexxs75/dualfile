use super::*;
use crate::{
    similar_space::*,
    space::{gone, invalid, KeepChoice, ScanProgress, TrashResultDto},
};
use dualfile_core::similar;
impl AppState {
    pub fn similar_start(
        &self,
        roots: &[String],
        options: SimilarOptionsDto,
    ) -> Result<u64, UiError> {
        options.validate()?;
        if roots.is_empty() || roots.len() > 64 {
            return Err(invalid());
        }
        let roots = roots
            .iter()
            .map(|r| {
                let path = crate::validate::scan_path(r, &self.home)?;
                if path
                    .ancestors()
                    .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
                {
                    return Err(invalid());
                }
                self.scan_root(r)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.similar.start(|slot| {
            self.submit_job_titled(
                "Ähnliche Bilder".into(),
                JobTitleDto::new("similar", "", 0),
                vec![],
                move |_| {
                    Box::new(move |job| {
                        let result = similar::find_similar_images(
                            &roots,
                            &similar::SimilarOptions {
                                min_size: options.min_size,
                                max_distance: options.max_distance as u32,
                                ..Default::default()
                            },
                            job,
                            &mut |p| {
                                job.scan_progress(p.done, p.bytes_read);
                                slot.lock().unwrap().progress = ScanProgress {
                                    stage: match p.stage {
                                        similar::SimilarStage::Scan => "scan",
                                        similar::SimilarStage::Hash => "hash",
                                        similar::SimilarStage::Group => "group",
                                    }
                                    .into(),
                                    entries: p.done,
                                    bytes: p.bytes_read,
                                    total: p.total,
                                };
                            },
                        );
                        let mut slot = slot.lock().unwrap();
                        slot.seconds = slot.started.elapsed().as_secs_f64();
                        let mut report = OpReport::default();
                        match result {
                            Ok(r) => {
                                report.cancelled = r.cancelled;
                                slot.result = Some(r);
                            }
                            Err(e) => {
                                slot.error = Some(UiError::from(e));
                                report.failed.push((
                                    VPath::local(&roots[0]).unwrap(),
                                    dualfile_core::Error::InvalidInput("Messung fehlgeschlagen"),
                                ));
                            }
                        }
                        report
                    })
                },
            )
        })
    }
    pub fn similar_page(
        &self,
        id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<SimilarPageDto, UiError> {
        page(&self.similar.get(id)?.lock().unwrap(), offset, limit)
    }
    pub fn similar_close(&self, id: u64) -> Result<(), UiError> {
        self.similar.close(id)?;
        self.jobs.cancel(id);
        Ok(())
    }

    pub fn similar_suggest(
        &self,
        id: u64,
        rule: SimilarRuleDto,
    ) -> Result<Vec<KeepChoice>, UiError> {
        let slot = self.similar.get(id)?;
        let slot = slot.lock().unwrap();
        Ok(suggest(slot.result.as_ref().ok_or_else(gone)?, rule))
    }
    pub fn similar_thumbnail(
        &self,
        id: u64,
        group_index: usize,
        entry_index: usize,
    ) -> Result<ThumbnailDto, UiError> {
        let slot = self.similar.get(id)?;
        let (entry, cancel) = {
            let held = slot.lock().unwrap();
            let e = held
                .result
                .as_ref()
                .ok_or_else(gone)?
                .groups
                .get(group_index)
                .and_then(|g| g.entries.get(entry_index))
                .ok_or_else(invalid)?;
            (e.clone(), held.thumbnail_cancel.clone())
        };
        let _permit = self.thumbnails.acquire(&cancel)?;
        if changed(&entry) {
            return Err(UiError::new("changed", "Datei hat sich geändert"));
        }
        let t =
            similar::render_thumbnail_checked(&entry.path, 256, &cancel).map_err(|e| match e {
                dualfile_core::Error::ChangedSincePlan(_) => {
                    UiError::new("changed", "Datei hat sich geändert")
                }
                dualfile_core::Error::Cancelled => gone(),
                _ => UiError::new("invalidImage", "Vorschaubild konnte nicht gelesen werden"),
            })?;
        if cancel.is_cancelled() {
            return Err(gone());
        }
        if changed(&entry) {
            return Err(UiError::new("changed", "Datei hat sich geändert"));
        }
        Ok(ThumbnailDto {
            bytes: t.bytes,
            width: t.width,
            height: t.height,
        })
    }
    pub fn similar_trash(
        &self,
        id: u64,
        choices: Vec<TrashChoice>,
    ) -> Result<TrashResultDto, UiError> {
        let slot = self.similar.get(id)?;
        let mut held = slot.lock().unwrap();
        if held.result.as_ref().ok_or_else(gone)?.cancelled {
            return Err(crate::space::invalid());
        }
        let (entries, skipped) = candidates(held.result.as_ref().ok_or_else(gone)?, &choices)?;
        let items = entries
            .iter()
            .map(|e| VPath::local(&e.path).map_err(UiError::from))
            .collect::<Result<Vec<_>, _>>()?;
        held.result.take().ok_or_else(gone)?;
        held.thumbnail_cancel.cancel();
        drop(held);
        let checks: HashMap<_, _> = entries.into_iter().map(|e| (e.path.clone(), e)).collect();
        let dirs = parent_dirs(&items);
        let recorder = self.undo_recorder(dirs.clone());
        let job_id = self.submit_job_titled(
            "Papierkorb: ähnliche Bilder".into(),
            JobTitleDto::new("trash", "", 0),
            dirs,
            move |_| {
                Box::new(move |job| {
                    let report = ops::trash_items_checked(&LocalVfs, &items, job, &mut |path| {
                        checks
                            .get(path.path())
                            .map_or(Some("changed".into()), |e| reason(e).map(str::to_owned))
                    });
                    recorder.record(recorder.journal.record_trash(&report.done));
                    report
                })
            },
        );
        Ok(TrashResultDto { job_id, skipped })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::similar::{SimilarEntry, SimilarGroup, SimilarResult};
    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEventDto) {}
    }
    #[test]
    fn explicit_selection_is_one_trash_job_with_undo_and_consumed_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        std::fs::create_dir(&root).unwrap();
        let entries: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|n| {
                let path = root.join(n);
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
                    hash_hex: String::new(),
                    distance_to_reference: 0,
                }
            })
            .collect();
        let state = AppState::new(Arc::new(Sink), d.path().into(), d.path().join("config"));
        let result = SimilarResult {
            groups: vec![SimilarGroup {
                entries,
                reclaimable_bytes: 10,
            }],
            ..Default::default()
        };
        let id = state
            .similar
            .start(|slot| {
                slot.lock().unwrap().result = Some(result);
                9000
            })
            .unwrap();
        let c = TrashChoice {
            group_index: 0,
            trash_paths: vec![
                root.join("a").to_string_lossy().into_owned(),
                root.join("b").to_string_lossy().into_owned(),
            ],
        };
        let slot = state.similar.get(id).unwrap();
        slot.lock().unwrap().result.as_mut().unwrap().cancelled = true;
        assert_eq!(
            state.similar_trash(id, vec![c.clone()]).err().unwrap().kind,
            "invalidInput"
        );
        assert!(state.jobs.list().is_empty());
        slot.lock().unwrap().result.as_mut().unwrap().cancelled = false;
        let r = state.similar_trash(id, vec![c.clone()]).unwrap();
        assert!(state.similar_trash(id, vec![c]).is_err());
        assert_eq!(state.jobs.wait(r.job_id).unwrap().state, JobState::Done);
        assert_eq!(state.jobs.list().len(), 1);
        assert!(!root.join("a").exists());
        assert!(!root.join("b").exists());
        assert!(root.join("c").exists());
        let history = state.undo_list();
        assert_eq!(history.len(), 1);
        state.undo_run(history[0].id);
        assert!(root.join("a").exists());
        assert!(root.join("b").exists());
    }
}
