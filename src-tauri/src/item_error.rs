//! Per-job, cancellable error questions. No backend error text crosses this bridge.
use crate::conflict::JobIdCell;
use crate::convert::{CoreEventDto, ItemErrorInfoDto};
use crate::state::EventSink;
use dualfile_core::{
    error::Error,
    ops::{ErrorDecision, ErrorResolver},
    vfs::VPath,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
};

#[derive(Default)]
struct Waiting {
    pending: HashMap<u64, Sender<ErrorDecision>>,
    cancelled: HashSet<u64>,
    closed: bool,
}
#[derive(Default)]
pub struct ErrorBridge(Mutex<Waiting>);
impl ErrorBridge {
    fn open(&self, id: u64) -> Option<Receiver<ErrorDecision>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.cancelled.contains(&id) {
            return None;
        }
        let (tx, rx) = channel();
        state.pending.insert(id, tx);
        Some(rx)
    }
    pub fn answer(&self, id: u64, decision: ErrorDecision) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(&id)
            .is_some_and(|tx| tx.send(decision).is_ok())
    }
    pub fn cancel(&self, id: u64) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled.insert(id);
        if let Some(tx) = state.pending.remove(&id) {
            let _ = tx.send(ErrorDecision::Abort);
        }
    }
    pub fn close(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        for (_, tx) in state.pending.drain() {
            let _ = tx.send(ErrorDecision::Abort);
        }
    }
    pub fn forget(&self, id: u64) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.remove(&id);
        state.cancelled.remove(&id);
    }
}
pub struct BridgeErrorResolver {
    pub job: Arc<JobIdCell>,
    pub bridge: Arc<ErrorBridge>,
    pub sink: Arc<dyn EventSink>,
}
impl ErrorResolver for BridgeErrorResolver {
    fn resolve(&self, path: &VPath, error: &Error) -> ErrorDecision {
        let Some(id) = self.job.get() else {
            return ErrorDecision::Abort;
        };
        let Some(rx) = self.bridge.open(id) else {
            return ErrorDecision::Abort;
        };
        let (kind, message) = match error {
            Error::Disconnected => ("disconnected", "Die Verbindung zum Server ist abgebrochen."),
            Error::VerifyFailed => ("verifyFailed", "Die Prüfsumme stimmt nach dem Übertragen nicht überein"),
            Error::PermissionDenied(_) => ("permission", "Zugriff verweigert."),
            _ => ("io", "Die Datei konnte nicht verarbeitet werden (zum Beispiel gesperrt oder Datenträger voll)."),
        };
        self.sink.emit(CoreEventDto::ItemError {
            error: ItemErrorInfoDto {
                job_id: id,
                name: path
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                kind,
                message,
            },
        });
        rx.recv().unwrap_or(ErrorDecision::Abort)
    }
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorDecisionDto {
    Skip,
    Retry,
    Abort,
}
impl ErrorDecisionDto {
    pub fn core(self) -> ErrorDecision {
        match self {
            Self::Skip => ErrorDecision::Skip,
            Self::Retry => ErrorDecision::Retry,
            Self::Abort => ErrorDecision::Abort,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Capture(Mutex<Sender<CoreEventDto>>);
    impl EventSink for Capture {
        fn emit(&self, event: CoreEventDto) {
            self.0.lock().unwrap().send(event).unwrap();
        }
    }

    #[test]
    fn resolver_emits_only_basename_and_fixed_error_text() {
        let (tx, rx) = channel();
        let bridge = Arc::new(ErrorBridge::default());
        let job = Arc::new(JobIdCell::default());
        job.set(7);
        let resolver = BridgeErrorResolver {
            job,
            bridge: bridge.clone(),
            sink: Arc::new(Capture(Mutex::new(tx))),
        };
        let worker = std::thread::spawn(move || {
            let path = VPath::local("/private/host-secret/file.txt").unwrap();
            resolver.resolve(&path, &Error::PermissionDenied(path.path().to_path_buf()))
        });
        let CoreEventDto::ItemError { error } = rx.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected item error");
        };
        assert_eq!(error.job_id, 7);
        assert_eq!(error.name, "file.txt");
        assert_eq!(error.kind, "permission");
        assert_eq!(error.message, "Zugriff verweigert.");
        assert!(bridge.answer(7, ErrorDecision::Skip));
        assert_eq!(worker.join().unwrap(), ErrorDecision::Skip);
    }

    #[test]
    fn cancel_answers_open_and_future_questions() {
        let bridge = ErrorBridge::default();
        let rx = bridge.open(1).unwrap();
        bridge.cancel(1);
        assert_eq!(rx.recv().unwrap(), ErrorDecision::Abort);
        assert!(bridge.open(1).is_none());
    }
    #[test]
    fn decisions_are_isolated_by_job_and_shutdown_unblocks_all() {
        let bridge = ErrorBridge::default();
        let a = bridge.open(1).unwrap();
        let b = bridge.open(2).unwrap();
        assert!(bridge.answer(1, ErrorDecision::Retry));
        assert_eq!(a.recv().unwrap(), ErrorDecision::Retry);
        assert!(b.try_recv().is_err());
        bridge.close();
        assert_eq!(b.recv().unwrap(), ErrorDecision::Abort);
        assert!(bridge.open(3).is_none());
    }
}
