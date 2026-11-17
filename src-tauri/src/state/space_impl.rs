use super::*;
use crate::space::*;
use dualfile_core::{dupes, usage};

impl AppState {
    pub(super) fn scan_root(&self, raw: &str) -> Result<PathBuf, UiError> {
        let path = crate::validate::scan_path(raw, &self.home)?;
        let path = std::fs::canonicalize(&path).map_err(|e| io_error("Ordner lesen", &path, &e))?;
        if !path.is_dir() {
            return Err(UiError::new("notDirectory", "Der Pfad ist kein Ordner"));
        }
        Ok(path)
    }
    pub fn usage_start(&self, path: &str, options: UsageOptionsDto) -> Result<u64, UiError> {
        let path = self.scan_root(path)?;
        self.usage.start(|slot| {
            self.submit_job_titled(
                "Platzbelegung".into(),
                JobTitleDto::new("space", "", 0),
                JobPaths::read(vec![path.to_string_lossy().into_owned()]),
                move |_| {
                    Box::new(move |job| {
                        let result = usage::measure_usage(
                            &path,
                            &usage::UsageOptions {
                                one_file_system: options.one_file_system,
                                ..Default::default()
                            },
                            job,
                            &mut |p| {
                                job.scan_progress(p.entries_seen, p.bytes);
                                slot.lock().unwrap().progress = ScanProgress {
                                    stage: "size".into(),
                                    entries: p.entries_seen,
                                    bytes: p.bytes,
                                    total: None,
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
                                    VPath::local(path).unwrap(),
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
    pub fn usage_node(&self, id: u64, node_path: Option<&str>) -> Result<UsagePageDto, UiError> {
        let path = node_path
            .map(|p| crate::validate::scan_path(p, &self.home))
            .transpose()?;
        usage_page(&self.usage.get(id)?.lock().unwrap(), path.as_deref())
    }
    pub fn usage_close(&self, id: u64) -> Result<(), UiError> {
        self.usage.close(id)?;
        self.jobs.cancel(id);
        Ok(())
    }
    pub fn dupes_start(&self, roots: &[String], options: DupesOptionsDto) -> Result<u64, UiError> {
        if roots.is_empty() || roots.len() > 64 {
            return Err(invalid());
        }
        let roots = roots
            .iter()
            .map(|r| self.scan_root(r))
            .collect::<Result<Vec<_>, _>>()?;
        self.dupes.start(|slot| {
            self.submit_job_titled(
                "Duplikate finden".into(),
                JobTitleDto::new("dupes", "", 0),
                JobPaths::read(
                    roots
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                ),
                move |_| {
                    Box::new(move |job| {
                        let result = dupes::find_duplicates(
                            &roots,
                            &dupes::DupeOptions {
                                min_size: options.min_size,
                                ..Default::default()
                            },
                            job,
                            &mut |p| {
                                job.scan_progress(p.done, p.bytes_read);
                                slot.lock().unwrap().progress = ScanProgress {
                                    stage: match p.stage {
                                        dupes::DupeStage::Size => "size",
                                        dupes::DupeStage::PartialHash => "partialHash",
                                        dupes::DupeStage::FullHash => "fullHash",
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
    pub fn dupes_page(
        &self,
        id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<DupesPageDto, UiError> {
        dupes_page(&self.dupes.get(id)?.lock().unwrap(), offset, limit)
    }
    pub fn dupes_close(&self, id: u64) -> Result<(), UiError> {
        self.dupes.close(id)?;
        self.jobs.cancel(id);
        Ok(())
    }
    pub fn dupes_suggest(
        &self,
        id: u64,
        rule: KeepRuleDto,
        folder: &str,
    ) -> Result<Vec<KeepSuggestionDto>, UiError> {
        let folder = crate::validate::scan_path(folder, &self.home)?;
        let slot = self.dupes.get(id)?;
        let slot = slot.lock().unwrap();
        Ok(suggestions(
            slot.result.as_ref().ok_or_else(gone)?,
            rule,
            folder,
        ))
    }
    pub fn dupes_trash(
        &self,
        id: u64,
        choices: Vec<KeepChoice>,
    ) -> Result<TrashResultDto, UiError> {
        let slot = self.dupes.get(id)?;
        let mut held = slot.lock().unwrap();
        if held.result.as_ref().ok_or_else(gone)?.cancelled {
            return Err(crate::space::invalid());
        }
        let (items, skipped) = trash_candidates(held.result.as_ref().ok_or_else(gone)?, &choices)?;
        let result = held.result.take().ok_or_else(gone)?;
        drop(held);
        let items = items
            .iter()
            .map(|p| VPath::local(p).map_err(UiError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let mut checks = HashMap::new();
        for choice in &choices {
            let group = &result.groups[choice.group_index];
            let keep = group
                .entries
                .iter()
                .position(|e| e.path.to_string_lossy() == choice.keep_path)
                .ok_or_else(invalid)?;
            for (entry, value) in group.entries.iter().enumerate() {
                checks.insert(value.path.clone(), (choice.group_index, entry, keep));
            }
        }
        let dirs = parent_dirs(&items);
        let recorder = self.undo_recorder(dirs.clone());
        // One existing trash operation and one journal record, even across multiple groups.
        let job_id = self.submit_job_titled(
            "Papierkorb: Duplikate".into(),
            JobTitleDto::new("dupesTrash", "", 0),
            dirs,
            move |_| {
                Box::new(move |job| {
                    let mut report =
                        ops::trash_items_checked(&LocalVfs, &items, job, &mut |path| {
                            let Some(&(group, entry, keep)) = checks.get(path.path()) else {
                                return Some("changed".into());
                            };
                            let group = &result.groups[group];
                            candidate_reason(
                                group.size,
                                &group.entries[entry],
                                &group.entries[keep],
                            )
                            .map(str::to_owned)
                        });
                    recorder.record(recorder.journal.record_trash(&report.done));
                    // Consume the snapshot after use. UI rescans after the job completes.
                    slot.lock().unwrap().result = None;
                    report.cancelled |= job.is_cancelled();
                    report
                })
            },
        )?;
        Ok(TrashResultDto { job_id, skipped })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEventDto) {}
    }
    #[test]
    fn duplicates_use_one_trash_job_and_one_undo_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        std::fs::create_dir(&root).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(root.join(name), b"same").unwrap();
        }
        let state = AppState::new(Arc::new(Sink), dir.path().into(), dir.path().join("config"));
        let id = state
            .dupes_start(
                &[root.to_string_lossy().into_owned()],
                DupesOptionsDto { min_size: 1 },
            )
            .unwrap();
        state.jobs.wait(id).unwrap();
        let before = state.jobs.list().len();
        let keep = root.join("a");
        let slot = state.dupes.get(id).unwrap();
        slot.lock().unwrap().result.as_mut().unwrap().cancelled = true;
        assert_eq!(
            state
                .dupes_trash(
                    id,
                    vec![KeepChoice {
                        group_index: 0,
                        keep_path: keep.to_string_lossy().into_owned()
                    }]
                )
                .err()
                .unwrap()
                .kind,
            "invalidInput"
        );
        assert_eq!(state.jobs.list().len(), before);
        slot.lock().unwrap().result.as_mut().unwrap().cancelled = false;
        let result = state
            .dupes_trash(
                id,
                vec![KeepChoice {
                    group_index: 0,
                    keep_path: keep.to_string_lossy().into_owned(),
                }],
            )
            .unwrap();
        let status = state.jobs.wait(result.job_id).unwrap();
        assert_eq!(status.state, JobState::Done);
        assert_eq!(state.jobs.list().len(), before + 1);
        assert!(keep.exists());
        assert!(!root.join("b").exists());
        assert!(!root.join("c").exists());
        let history = state.undo_list();
        assert_eq!(history.len(), 1);
        state.undo_run(history[0].id);
        assert!(root.join("b").exists());
        assert!(root.join("c").exists());
    }
}
