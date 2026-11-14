//! Bridge between the core's blocking `ConflictResolver` and the UI's asynchronous dialog.
//!
//! The resolver runs on the job's own thread (never on a Tauri command thread): it sends a
//! `conflict` event and blocks until `answer_conflict` delivers the decision. Cancelling a
//! job, or shutting the app down, answers every waiting resolver with `Cancel`.

use crate::convert::{conflict_dto, CoreEventDto};
use crate::state::EventSink;
use dualfile_core::ops::{Conflict, ConflictResolver, Resolution};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Job number, known only after `JobManager::submit` returned; 0 = not yet known.
#[derive(Default)]
pub struct JobIdCell(AtomicU64);

impl JobIdCell {
    pub fn set(&self, id: u64) {
        self.0.store(id, Ordering::SeqCst);
    }
    pub fn get(&self) -> Option<u64> {
        match self.0.load(Ordering::SeqCst) {
            0 => None,
            id => Some(id),
        }
    }
}

#[derive(Default)]
struct Waiting {
    pending: HashMap<u64, Sender<Resolution>>,
    /// Jobs cancelled while no question was open: their next question is answered `Cancel`.
    cancelled: HashSet<u64>,
}

#[derive(Default)]
pub struct ConflictBridge {
    waiting: Mutex<Waiting>,
}

impl ConflictBridge {
    fn state(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens a question for the job. `Err(())`: the job was already cancelled.
    fn open(&self, job: u64) -> Result<std::sync::mpsc::Receiver<Resolution>, ()> {
        let mut waiting = self.state();
        if waiting.cancelled.contains(&job) {
            return Err(());
        }
        let (tx, rx) = channel();
        waiting.pending.insert(job, tx);
        Ok(rx)
    }

    /// Delivers the user's decision. `false`: that job waits for no answer.
    pub fn answer(&self, job: u64, resolution: Resolution) -> bool {
        let sender = self.state().pending.remove(&job);
        match sender {
            Some(tx) => tx.send(resolution).is_ok(),
            None => false,
        }
    }

    /// Cancels the job's open question, or marks the job so a later question is cancelled.
    pub fn cancel(&self, job: u64) {
        let sender = {
            let mut waiting = self.state();
            let sender = waiting.pending.remove(&job);
            if sender.is_none() {
                waiting.cancelled.insert(job);
            }
            sender
        };
        if let Some(tx) = sender {
            let _ = tx.send(Resolution::Cancel);
        }
    }

    /// Shutdown: every waiting resolver gets `Cancel`, and no new question is accepted for them.
    pub fn cancel_all(&self) {
        let senders: Vec<_> = self.state().pending.drain().collect();
        for (_, tx) in senders {
            let _ = tx.send(Resolution::Cancel);
        }
    }

    /// The job is finished; drop its bookkeeping.
    pub fn forget(&self, job: u64) {
        let mut waiting = self.state();
        waiting.pending.remove(&job);
        waiting.cancelled.remove(&job);
    }

    #[cfg(test)]
    pub fn is_waiting(&self, job: u64) -> bool {
        self.state().pending.contains_key(&job)
    }
}

pub struct BridgeResolver {
    pub job: Arc<JobIdCell>,
    pub bridge: Arc<ConflictBridge>,
    pub sink: Arc<dyn EventSink>,
}

impl ConflictResolver for BridgeResolver {
    fn resolve(&self, conflict: &Conflict) -> Resolution {
        let Some(job) = self.job.get() else {
            return Resolution::Cancel;
        };
        let Ok(rx) = self.bridge.open(job) else {
            return Resolution::Cancel;
        };
        self.sink.emit(CoreEventDto::Conflict {
            conflict: conflict_dto(job, conflict),
        });
        // A dropped sender (nobody will ever answer) also means: do not overwrite anything.
        rx.recv().unwrap_or(Resolution::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::vfs::{Entry, EntryKind, VPath};
    use std::sync::mpsc;
    use std::time::Duration;

    struct ChannelSink(Mutex<mpsc::Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    fn entry() -> Entry {
        Entry {
            name: "a".into(),
            kind: EntryKind::File,
            size: 1,
            modified: None,
            mode: None,
            hidden: false,
            link_target: None,
            broken_link: false,
            error: None,
        }
    }

    fn conflict() -> Conflict {
        Conflict {
            source: entry(),
            destination: entry(),
            source_path: VPath::local("/s/a").unwrap(),
            destination_path: VPath::local("/d/a").unwrap(),
        }
    }

    struct Setup {
        resolver: Arc<BridgeResolver>,
        bridge: Arc<ConflictBridge>,
        events: mpsc::Receiver<CoreEventDto>,
    }

    fn setup(job: Option<u64>) -> Setup {
        let (tx, events) = mpsc::channel();
        let bridge = Arc::new(ConflictBridge::default());
        let cell = Arc::new(JobIdCell::default());
        if let Some(id) = job {
            cell.set(id);
        }
        let resolver = Arc::new(BridgeResolver {
            job: cell,
            bridge: bridge.clone(),
            sink: Arc::new(ChannelSink(Mutex::new(tx))),
        });
        Setup {
            resolver,
            bridge,
            events,
        }
    }

    fn ask(resolver: &Arc<BridgeResolver>) -> mpsc::Receiver<Resolution> {
        let (tx, rx) = mpsc::channel();
        let resolver = resolver.clone();
        std::thread::spawn(move || {
            let _ = tx.send(resolver.resolve(&conflict()));
        });
        rx
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn question_blocks_until_the_answer_arrives() {
        let s = setup(Some(3));
        let decision = ask(&s.resolver);
        let event = s.events.recv_timeout(WAIT).expect("conflict event");
        let CoreEventDto::Conflict { conflict } = event else {
            panic!("expected a conflict event");
        };
        assert_eq!(conflict.job_id, 3);
        assert!(decision.try_recv().is_err(), "resolver must still block");
        assert!(s.bridge.is_waiting(3));
        assert!(s.bridge.answer(3, Resolution::KeepBothAll));
        assert_eq!(
            decision.recv_timeout(WAIT).unwrap(),
            Resolution::KeepBothAll
        );
        assert!(!s.bridge.is_waiting(3));
    }

    #[test]
    fn an_answer_for_a_job_without_question_is_refused() {
        let s = setup(Some(3));
        assert!(!s.bridge.answer(3, Resolution::Overwrite));
        assert!(!s.bridge.answer(99, Resolution::Skip));
    }

    #[test]
    fn answers_only_reach_their_own_job() {
        let a = setup(Some(1));
        let decision = ask(&a.resolver);
        a.events.recv_timeout(WAIT).unwrap();
        assert!(!a.bridge.answer(2, Resolution::Overwrite));
        assert!(decision.try_recv().is_err());
        assert!(a.bridge.answer(1, Resolution::Skip));
        assert_eq!(decision.recv_timeout(WAIT).unwrap(), Resolution::Skip);
    }

    #[test]
    fn cancelling_the_job_answers_cancel() {
        let s = setup(Some(5));
        let decision = ask(&s.resolver);
        s.events.recv_timeout(WAIT).unwrap();
        s.bridge.cancel(5);
        assert_eq!(decision.recv_timeout(WAIT).unwrap(), Resolution::Cancel);
    }

    #[test]
    fn cancel_before_the_question_cancels_the_later_question_without_blocking() {
        let s = setup(Some(6));
        s.bridge.cancel(6);
        let decision = ask(&s.resolver);
        assert_eq!(decision.recv_timeout(WAIT).unwrap(), Resolution::Cancel);
        assert!(
            s.events.try_recv().is_err(),
            "no dialog for a cancelled job"
        );
        s.bridge.forget(6);
        assert!(!s.bridge.is_waiting(6));
    }

    #[test]
    fn shutdown_cancels_every_waiting_resolver() {
        let s = setup(Some(7));
        let decision = ask(&s.resolver);
        s.events.recv_timeout(WAIT).unwrap();
        s.bridge.cancel_all();
        assert_eq!(decision.recv_timeout(WAIT).unwrap(), Resolution::Cancel);
    }

    #[test]
    fn unknown_job_id_means_cancel() {
        let s = setup(None);
        let decision = ask(&s.resolver);
        assert_eq!(decision.recv_timeout(WAIT).unwrap(), Resolution::Cancel);
    }
}
