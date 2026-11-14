use super::*;
use crate::convert::{UndoEntryDto, UndoResultDto};
use dualfile_core::undo::{UndoError, UndoId, UndoJournal};

pub(super) fn roots(pairs: &[(VPath, VPath)]) -> Vec<(VPath, VPath)> {
    pairs
        .iter()
        .filter(|(_, target)| {
            !pairs
                .iter()
                .any(|(_, parent)| parent != target && target.path().starts_with(parent.path()))
        })
        .cloned()
        .collect()
}

pub(super) struct Recorder {
    pub journal: Arc<UndoJournal>,
    dirs: Vec<String>,
    all_dirs: Arc<Mutex<HashMap<u64, Vec<String>>>>,
    sink: Arc<dyn EventSink>,
}
impl Recorder {
    pub fn record(&self, result: Result<Option<UndoId>, UndoError>) {
        match result {
            Ok(Some(id)) => {
                let mut dirs = lock(&self.all_dirs);
                dirs.insert(id, self.dirs.clone());
                let live = self.journal.list();
                dirs.retain(|id, _| live.iter().any(|e| e.id == *id));
                drop(dirs);
                self.sink.emit(CoreEventDto::UndoChanged);
            }
            Err(_) => eprintln!("undo: recording failed"),
            Ok(None) => {}
        }
    }
}
impl AppState {
    pub(super) fn undo_recorder(&self, dirs: Vec<String>) -> Recorder {
        Recorder {
            journal: self.undo.clone(),
            dirs,
            all_dirs: self.undo_dirs.clone(),
            sink: self.shared.sink.clone(),
        }
    }
    pub fn undo_list(&self) -> Vec<UndoEntryDto> {
        self.undo.list().into_iter().map(Into::into).collect()
    }
    /// The command waits on a worker; queued/running job events expose its ID for cancellation.
    pub fn undo_run(&self, id: u64) -> UndoResultDto {
        let journal = self.undo.clone();
        let sink = self.shared.sink.clone();
        let dirs = lock(&self.undo_dirs).get(&id).cloned().unwrap_or_default();
        let result = Arc::new(Mutex::new(None));
        let output = result.clone();
        let job_id = self.submit_job("Rückgängig".into(), dirs, move |_| {
            Box::new(move |job| {
                let result = UndoResultDto::from_report(journal.undo(id, &LocalVfs, job));
                if result.done > 0 {
                    sink.emit(CoreEventDto::UndoChanged);
                }
                // Job errors contain only fixed codes and an empty path, never OS details.
                let report = OpReport {
                    done: (0..result.done)
                        .map(|_| VPath::local("/").unwrap())
                        .collect(),
                    cancelled: result.cancelled,
                    failed: result
                        .errors
                        .iter()
                        .map(|e| {
                            (
                                VPath::local("/").unwrap(),
                                dualfile_core::Error::Unsupported(e),
                            )
                        })
                        .collect(),
                    ..OpReport::default()
                };
                *lock(&output) = Some(result);
                report
            })
        });
        let status = self.jobs.wait(job_id);
        let mut answer = lock(&result).take().unwrap_or_else(|| UndoResultDto {
            ok: false,
            done: 0,
            remaining: 0,
            errors: if status
                .as_ref()
                .is_some_and(|s| s.state == JobState::Cancelled)
            {
                vec![]
            } else {
                vec!["io"]
            },
            cancelled: status.is_some_and(|s| s.state == JobState::Cancelled),
            job_id,
        });
        answer.job_id = job_id;
        answer
    }
}
