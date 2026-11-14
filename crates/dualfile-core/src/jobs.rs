//! Auftragsverwaltung: führt `ops`-Arbeit auf eigenen Threads aus (kein Async).
//!
//! Aufträge können an kooperativen Kontrollpunkten pausieren. Pause/Fortsetzen
//! werden als `Progress` mit dem neuen Zustand gemeldet. Die Spur bleibt reserviert,
//! während der Parallelitätsplatz bei Pause freigegeben wird.
//! Ereignisse je Auftrag: `Queued`, `Started`, `Progress*`, `Finished`; ein
//! vor dem Start abgebrochener Auftrag erzeugt kein `Started`.

use crate::error::Error;
use crate::ops::{JobControl, JobHandle, OpReport, ProgressEvent, ProgressSink};
use crate::vfs::VPath;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Instant;

pub type JobId = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

/// Stand eines Auftrags. `report` ist ein `Arc`, weil `OpReport` nicht
/// `Clone` ist (`Error` enthält `io::Error`) und der Stand mit jedem
/// Fortschrittsereignis kopiert wird.
#[derive(Clone)]
pub struct JobStatus {
    pub state: JobState,
    pub files_done: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_per_second: f64,
    pub report: Option<Arc<OpReport>>,
    pub eta_seconds: Option<u64>,
    /// One-based global waiting order, including paused queued jobs.
    pub queue_position: Option<usize>,
    pub lane: Option<String>,
}

#[derive(Clone)]
pub enum JobEvent {
    Queued(JobId),
    Started(JobId),
    Progress(JobId, JobStatus),
    Finished(JobId, JobState),
}

pub trait JobSink: Send + Sync {
    fn event(&self, event: JobEvent);
}

struct NullSink;
impl JobSink for NullSink {
    fn event(&self, _: JobEvent) {}
}

type JobWork = Box<dyn FnOnce(&JobHandle) -> OpReport + Send>;

struct Slot {
    status: JobStatus,
    work: Option<JobWork>,
    handle: Option<JobHandle>,
    /// Erst wahr, nachdem `Finished` ausgeliefert wurde; `wait` wartet darauf,
    /// damit Beobachter nach `wait` die vollständige Ereignisfolge sehen.
    finished: bool,
    paused: bool,
    active: bool,
    started: bool,
}

struct Inner {
    cancelled: AtomicBool,
    slot: Mutex<Slot>,
    changed: Condvar,
}

struct Sched {
    queue: VecDeque<(JobId, Arc<Inner>)>,
    active: usize,
    lanes: HashMap<String, JobId>,
}

struct State {
    jobs: Mutex<HashMap<JobId, Arc<Inner>>>,
    sched: Mutex<Sched>,
    limit: usize,
    next: AtomicU64,
    sink: Arc<dyn JobSink>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

pub struct JobManager {
    state: Arc<State>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new(2, Arc::new(NullSink))
    }
}

impl JobManager {
    pub fn new(limit: usize, sink: Arc<dyn JobSink>) -> Self {
        Self {
            state: Arc::new(State {
                jobs: Mutex::new(HashMap::new()),
                sched: Mutex::new(Sched {
                    queue: VecDeque::new(),
                    active: 0,
                    lanes: HashMap::new(),
                }),
                limit: limit.max(1),
                next: AtomicU64::new(1),
                sink,
                threads: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn submit(
        &self,
        name: impl Into<String>,
        work: Box<dyn FnOnce(&JobHandle) -> OpReport + Send>,
    ) -> JobId {
        self.submit_with_lane(name, None, work)
    }

    pub fn submit_with_lane(
        &self,
        _name: impl Into<String>,
        lane: Option<String>,
        work: Box<dyn FnOnce(&JobHandle) -> OpReport + Send>,
    ) -> JobId {
        let id = self.state.next.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::new(Inner {
            cancelled: AtomicBool::new(false),
            slot: Mutex::new(Slot {
                status: JobStatus {
                    state: JobState::Queued,
                    files_done: 0,
                    bytes_done: 0,
                    bytes_total: 0,
                    bytes_per_second: 0.0,
                    report: None,
                    eta_seconds: None,
                    queue_position: None,
                    lane,
                },
                work: Some(work),
                handle: None,
                finished: false,
                paused: false,
                active: false,
                started: false,
            }),
            changed: Condvar::new(),
        });
        self.state.jobs.lock().unwrap().insert(id, inner.clone());
        // `Queued` muss vor jedem möglichen `Started` ausgeliefert sein.
        self.state.sink.event(JobEvent::Queued(id));
        {
            let mut sched = self.state.sched.lock().unwrap();
            if !terminal(inner.slot.lock().unwrap().status.state) {
                sched.queue.push_back((id, inner));
            }
            positions(&sched);
        }

        schedule(&self.state);
        id
    }

    pub fn status(&self, id: JobId) -> Option<JobStatus> {
        let inner = self.state.jobs.lock().unwrap().get(&id).cloned()?;
        let status = inner.slot.lock().unwrap().status.clone();
        Some(status)
    }

    /// Alle Aufträge, nach Nummer sortiert.
    pub fn list(&self) -> Vec<(JobId, JobStatus)> {
        let jobs: Vec<_> = self
            .state
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(|(id, j)| (*id, j.clone()))
            .collect();
        let mut out: Vec<_> = jobs
            .into_iter()
            .map(|(id, j)| (id, j.slot.lock().unwrap().status.clone()))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// Bricht ab. Wartende Aufträge werden ohne Start `Cancelled`, laufende
    /// bekommen das Abbruch-Token. `false`: unbekannte Nummer.
    pub fn cancel(&self, id: JobId) -> bool {
        let Some(inner) = self.state.jobs.lock().unwrap().get(&id).cloned() else {
            return false;
        };
        cancel_inner(&self.state, id, &inner);
        true
    }

    pub fn pause(&self, id: JobId) -> bool {
        self.set_pause(id, true)
    }
    pub fn resume(&self, id: JobId) -> bool {
        self.set_pause(id, false)
    }
    fn set_pause(&self, id: JobId, paused: bool) -> bool {
        let Some(inner) = self.state.jobs.lock().unwrap().get(&id).cloned() else {
            return false;
        };
        change_pause(&self.state, id, &inner, paused);
        true
    }

    /// Blockiert, bis der Auftrag beendet und `Finished` ausgeliefert ist.
    pub fn wait(&self, id: JobId) -> Option<JobStatus> {
        let inner = self.state.jobs.lock().unwrap().get(&id).cloned()?;
        let mut slot = inner.slot.lock().unwrap();
        while !slot.finished {
            slot = inner.changed.wait(slot).unwrap();
        }
        Some(slot.status.clone())
    }
}

fn terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Done | JobState::Failed | JobState::Cancelled
    )
}

fn positions(sched: &Sched) {
    for (index, (_, inner)) in sched.queue.iter().enumerate() {
        inner.slot.lock().unwrap().status.queue_position = Some(index + 1);
    }
}

fn cancel_inner(state: &Arc<State>, id: JobId, inner: &Arc<Inner>) {
    let cancelled_queued = {
        let mut sched = state.sched.lock().unwrap();
        let mut slot = inner.slot.lock().unwrap();
        if terminal(slot.status.state) {
            return;
        }
        inner.cancelled.store(true, Ordering::SeqCst);
        if !slot.started {
            slot.status.state = JobState::Cancelled;
            slot.status.queue_position = None;
            slot.work = None;
            sched.queue.retain(|(i, _)| *i != id);
            drop(slot);
            positions(&sched);
            true
        } else {
            false
        }
    };
    inner.changed.notify_all();
    if cancelled_queued {
        state
            .sink
            .event(JobEvent::Finished(id, JobState::Cancelled));
        mark_finished(inner);
    }
    schedule(state);
}

fn change_pause(state: &Arc<State>, id: JobId, inner: &Arc<Inner>, paused: bool) {
    let snapshot = {
        let mut sched = state.sched.lock().unwrap();
        let mut slot = inner.slot.lock().unwrap();
        if terminal(slot.status.state) || slot.paused == paused {
            return;
        }
        slot.paused = paused;
        // A running job only relinquishes its permit at its next checkpoint.
        if !slot.active {
            slot.status.state = if paused {
                JobState::Paused
            } else {
                JobState::Queued
            };
            if slot.started {
                sched.queue.retain(|(i, _)| *i != id);
                slot.status.queue_position = None;
                if !paused {
                    sched.queue.push_back((id, inner.clone()));
                }
            }
        }
        slot.status.eta_seconds = None;
        drop(slot);
        positions(&sched);
        inner.slot.lock().unwrap().status.clone()
    };
    state.sink.event(JobEvent::Progress(id, snapshot));
    schedule(state);
    inner.changed.notify_all();
}

struct ManagedControl {
    state: Weak<State>,
    inner: Arc<Inner>,
    id: JobId,
}
impl JobControl for ManagedControl {
    fn cancel(&self) {
        if let Some(state) = self.state.upgrade() {
            cancel_inner(&state, self.id, &self.inner);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }
    fn pause(&self) {
        if let Some(state) = self.state.upgrade() {
            change_pause(&state, self.id, &self.inner, true);
        }
    }
    fn resume(&self) {
        if let Some(state) = self.state.upgrade() {
            change_pause(&state, self.id, &self.inner, false);
        }
    }
    fn is_paused(&self) -> bool {
        self.inner.slot.lock().unwrap().paused
    }
    fn checkpoint(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return true;
        };
        loop {
            let snapshot = {
                let mut sched = state.sched.lock().unwrap();
                let mut slot = self.inner.slot.lock().unwrap();
                if slot.paused && slot.active && !self.is_cancelled() {
                    slot.active = false;
                    slot.status.state = JobState::Paused;
                    slot.status.eta_seconds = None;
                    sched.active -= 1;
                    Some(slot.status.clone())
                } else {
                    None
                }
            };
            if let Some(snapshot) = snapshot {
                state.sink.event(JobEvent::Progress(self.id, snapshot));
                schedule(&state);
            }
            let mut slot = self.inner.slot.lock().unwrap();
            while !slot.active && !self.is_cancelled() && !terminal(slot.status.state) {
                slot = self.inner.changed.wait(slot).unwrap();
            }
            if self.is_cancelled() || terminal(slot.status.state) {
                return self.is_cancelled();
            }
            // Resume may have been followed by another pause before this thread woke.
            if !slot.paused {
                return false;
            }
        }
    }
}

fn mark_finished(inner: &Inner) {
    inner.slot.lock().unwrap().finished = true;
    inner.changed.notify_all();
}

impl Drop for JobManager {
    fn drop(&mut self) {
        // Nichts im Hintergrund weiterlaufen lassen: erst alles abbrechen,
        // dann auf die Threads warten.
        let all: Vec<_> = self
            .state
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(|(id, j)| (*id, j.clone()))
            .collect();
        for (id, inner) in all {
            cancel_inner(&self.state, id, &inner);
        }
        loop {
            let next = self.state.threads.lock().unwrap().pop();
            match next {
                Some(t) => {
                    let _ = t.join();
                }
                None => break,
            }
        }
    }
}

/// Übersetzt Kopier-Ereignisse (Bytes je Datei) in kumulierten Auftragsstand.
struct Bridge {
    id: JobId,
    inner: Arc<Inner>,
    sink: Arc<dyn JobSink>,
    started: Instant,
    /// Datei, deren Bytes zuletzt gemeldet wurden, und ihr letzter Stand.
    current: Mutex<Option<(VPath, u64)>>,
    rate: Mutex<Rate>,
}

impl ProgressSink for Bridge {
    fn event(&self, e: ProgressEvent) {
        let snapshot = {
            let mut slot = self.inner.slot.lock().unwrap();
            let s = &mut slot.status;
            match e {
                ProgressEvent::ScanProgress { entries, bytes } => {
                    s.files_done = entries;
                    s.bytes_done = bytes;
                    s.bytes_total = 0;
                    s.eta_seconds = None;
                }
                ProgressEvent::BytesCopied {
                    path,
                    bytes,
                    total_bytes,
                } => {
                    let mut cur = self.current.lock().unwrap();
                    match cur.as_mut() {
                        Some((p, last)) if *p == path && bytes >= *last => {
                            s.bytes_done += bytes - *last;
                            *last = bytes;
                        }
                        _ => {
                            // Neue Datei (oder Neustart nach Konflikt-Rennen).
                            s.bytes_done += bytes;
                            s.bytes_total += total_bytes;
                            *cur = Some((path, bytes));
                        }
                    }
                    let elapsed = self.started.elapsed().as_secs_f64();
                    s.bytes_per_second = s.bytes_done as f64 / elapsed.max(0.001);
                    s.eta_seconds =
                        self.rate
                            .lock()
                            .unwrap()
                            .update(elapsed, s.bytes_done, s.bytes_total);
                }
                ProgressEvent::ItemDone(_) => s.files_done += 1,
                _ => {}
            }
            s.clone()
        };
        // Außerhalb der Sperre, damit eine Senke `status()` aufrufen darf.
        self.sink.event(JobEvent::Progress(self.id, snapshot));
    }
}

/// EMA of per-sample throughput, alpha = 0.25. Times are injected for deterministic tests.
#[derive(Default)]
struct Rate {
    time: f64,
    bytes: u64,
    smoothed: Option<f64>,
}
impl Rate {
    fn update(&mut self, elapsed: f64, bytes: u64, total: u64) -> Option<u64> {
        let dt = elapsed - self.time;
        if dt > 0.0 {
            let sample = bytes.saturating_sub(self.bytes) as f64 / dt;
            self.smoothed = Some(
                self.smoothed
                    .map_or(sample, |old| 0.25 * sample + 0.75 * old),
            );
            self.time = elapsed;
            self.bytes = bytes;
            if sample == 0.0 {
                return None;
            }
        }
        let rate = self.smoothed?;
        (elapsed >= 2.0 && rate > 0.0)
            .then(|| (total.saturating_sub(bytes) as f64 / rate).ceil() as u64)
    }
}

fn schedule(state: &Arc<State>) {
    loop {
        let picked = {
            let mut sched = state.sched.lock().unwrap();
            if sched.active >= state.limit {
                return;
            }
            // The first queued job in a lane owns its ordering, even when paused.
            let mut seen = std::collections::HashSet::new();
            let index = sched.queue.iter().position(|(id, inner)| {
                let slot = inner.slot.lock().unwrap();
                if let Some(lane) = &slot.status.lane {
                    if sched.lanes.get(lane).is_some_and(|owner| owner != id) {
                        return false;
                    }
                    if !seen.insert(lane.clone()) {
                        return false;
                    }
                }
                !slot.paused
                    && !inner.cancelled.load(Ordering::SeqCst)
                    && slot.status.state == JobState::Queued
            });
            let Some(index) = index else {
                return;
            };
            let (id, inner) = sched.queue.remove(index).unwrap();
            let mut slot = inner.slot.lock().unwrap();
            sched.active += 1;
            if let Some(lane) = &slot.status.lane {
                sched.lanes.insert(lane.clone(), id);
            }
            slot.active = true;
            slot.status.state = JobState::Running;
            slot.status.queue_position = None;
            if slot.started {
                let snapshot = slot.status.clone();
                drop(slot);
                positions(&sched);
                drop(sched);
                state.sink.event(JobEvent::Progress(id, snapshot));
                inner.changed.notify_all();
                continue;
            }
            slot.started = true;
            let bridge = Arc::new(Bridge {
                id,
                inner: inner.clone(),
                sink: state.sink.clone(),
                started: Instant::now(),
                current: Mutex::new(None),
                rate: Mutex::new(Rate::default()),
            });
            let handle = JobHandle::controlled(
                bridge,
                Arc::new(ManagedControl {
                    state: Arc::downgrade(state),
                    inner: inner.clone(),
                    id,
                }),
            );
            slot.handle = Some(handle.clone());
            let work = slot.work.take().expect("Queued job has work");
            drop(slot);
            positions(&sched);
            (id, inner, handle, work)
        };
        let (id, inner, handle, work) = picked;
        let s = state.clone();
        let t = std::thread::spawn(move || run_job(s, id, inner, handle, work));
        state.threads.lock().unwrap().push(t);
    }
}

fn run_job(state: Arc<State>, id: JobId, inner: Arc<Inner>, handle: JobHandle, work: JobWork) {
    state.sink.event(JobEvent::Started(id));
    let report = catch_unwind(AssertUnwindSafe(|| {
        if handle.checkpoint() {
            OpReport {
                cancelled: true,
                ..OpReport::default()
            }
        } else {
            work(&handle)
        }
    }))
    .unwrap_or_else(|_| {
        // Ein abgestürzter Auftrag darf `wait` nicht ewig blockieren.
        let mut r = OpReport::default();
        let root = VPath::local("/").expect("root path is valid");
        r.failed.push((
            root,
            Error::Io {
                op: "Auftrag ausführen",
                path: "/".into(),
                source: std::io::Error::other("Auftrag unerwartet abgebrochen"),
            },
        ));
        r
    });
    let final_state = if handle.is_cancelled() || report.cancelled {
        JobState::Cancelled
    } else if !report.failed.is_empty() && report.done.is_empty() {
        JobState::Failed
    } else {
        JobState::Done
    };
    {
        let mut sched = state.sched.lock().unwrap();
        let mut slot = inner.slot.lock().unwrap();
        slot.status.state = final_state;
        slot.status.eta_seconds = None;
        slot.status.queue_position = None;
        slot.status.report = Some(Arc::new(report));
        slot.handle = None;
        if slot.active {
            sched.active -= 1;
            slot.active = false;
        }
        if let Some(lane) = &slot.status.lane {
            sched.lanes.remove(lane);
        }
        sched.queue.retain(|(i, _)| *i != id);
        drop(slot);
        positions(&sched);
    }
    state.sink.event(JobEvent::Finished(id, final_state));
    mark_finished(&inner);
    schedule(&state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::copy;
    use crate::vfs::LocalVfs;
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn scan_counters_use_managed_job_progress() {
        let manager = JobManager::default();
        let id = manager.submit(
            "scan",
            Box::new(|job| {
                job.scan_progress(12, 4096);
                OpReport::default()
            }),
        );
        let status = manager.wait(id).unwrap();
        assert_eq!(status.files_done, 12);
        assert_eq!(status.bytes_done, 4096);
    }

    #[derive(Default)]
    struct Record(Mutex<Vec<JobEvent>>);
    impl JobSink for Record {
        fn event(&self, e: JobEvent) {
            self.0.lock().unwrap().push(e);
        }
    }
    impl Record {
        fn tags(&self, id: JobId) -> Vec<&'static str> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    JobEvent::Queued(i) if *i == id => Some("Q"),
                    JobEvent::Started(i) if *i == id => Some("S"),
                    JobEvent::Progress(i, _) if *i == id => Some("P"),
                    JobEvent::Finished(i, _) if *i == id => Some("F"),
                    _ => None,
                })
                .collect()
        }
    }

    fn vp(p: &std::path::Path) -> VPath {
        VPath::local(p).unwrap()
    }

    fn copy_work(src: VPath, dst: VPath) -> JobWork {
        Box::new(move |h| copy(&LocalVfs, &[src], &LocalVfs, &dst, None, h))
    }

    fn done_report(path: &std::path::Path) -> OpReport {
        let mut r = OpReport::default();
        r.done.push(vp(path));
        r
    }

    #[test]
    fn events_arrive_in_order_queued_started_progress_finished() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.bin");
        fs::write(&src, vec![7u8; 300_000]).unwrap();
        let dst = dir.path().join("out");
        fs::create_dir(&dst).unwrap();
        let rec = Arc::new(Record::default());
        let m = JobManager::new(2, rec.clone());
        let id = m.submit("copy", copy_work(vp(&src), vp(&dst)));
        let st = m.wait(id).unwrap();
        assert_eq!(st.state, JobState::Done);
        let tags = rec.tags(id);
        assert_eq!(tags.first(), Some(&"Q"));
        assert_eq!(tags.get(1), Some(&"S"));
        assert_eq!(tags.last(), Some(&"F"));
        assert_eq!(tags.iter().filter(|t| **t == "Q").count(), 1);
        assert_eq!(tags.iter().filter(|t| **t == "S").count(), 1);
        assert_eq!(tags.iter().filter(|t| **t == "F").count(), 1);
        assert!(tags[2..tags.len() - 1].iter().all(|t| *t == "P"));
        assert!(tags.len() > 3, "expected progress events: {tags:?}");
    }

    #[test]
    fn progress_is_monotonic_and_accumulates_over_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tree");
        fs::create_dir(&src).unwrap();
        for i in 0..3 {
            fs::write(src.join(format!("f{i}")), vec![1u8; 200_000]).unwrap();
        }
        let dst = dir.path().join("out");
        fs::create_dir(&dst).unwrap();
        let rec = Arc::new(Record::default());
        let m = JobManager::new(2, rec.clone());
        let id = m.submit("copy", copy_work(vp(&src), vp(&dst)));
        let st = m.wait(id).unwrap();
        assert_eq!(st.bytes_done, 600_000);
        assert_eq!(st.bytes_total, 600_000);
        assert!(st.files_done >= 3);
        let mut last = (0u64, 0u64, 0u64);
        for e in rec.0.lock().unwrap().iter() {
            if let JobEvent::Progress(i, s) = e {
                assert_eq!(*i, id);
                assert!(s.bytes_done >= last.0, "bytes_done decreased");
                assert!(s.bytes_total >= last.1, "bytes_total decreased");
                assert!(s.files_done >= last.2, "files_done decreased");
                last = (s.bytes_done, s.bytes_total, s.files_done);
            }
        }
        assert_eq!(last.0, 600_000);
    }

    #[test]
    fn cancelling_a_running_copy_leaves_no_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big.bin");
        fs::write(&src, vec![3u8; 20 * 1024 * 1024]).unwrap();
        let dst = dir.path().join("out");
        fs::create_dir(&dst).unwrap();
        let (tx, rx) = mpsc::channel::<()>();
        struct Trigger(Mutex<mpsc::Sender<()>>);
        impl JobSink for Trigger {
            fn event(&self, e: JobEvent) {
                if let JobEvent::Progress(..) = e {
                    let _ = self.0.lock().unwrap().send(());
                }
            }
        }
        let m = JobManager::new(1, Arc::new(Trigger(Mutex::new(tx))));
        let id = m.submit("copy", copy_work(vp(&src), vp(&dst)));
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(m.cancel(id));
        let st = m.wait(id).unwrap();
        assert_eq!(st.state, JobState::Cancelled);
        let names: Vec<_> = fs::read_dir(&dst)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains(".dualfile-part")),
            "left over: {names:?}"
        );
        assert!(!dst.join("big.bin").exists(), "partial target left behind");
    }

    #[test]
    fn queued_job_cancelled_never_starts() {
        let rec = Arc::new(Record::default());
        let m = JobManager::new(1, rec.clone());
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (running_tx, running_rx) = mpsc::channel::<()>();
        let blocker = m.submit(
            "blocker",
            Box::new(move |_| {
                running_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                OpReport::default()
            }),
        );
        running_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let ran = Arc::new(AtomicU64::new(0));
        let ran2 = ran.clone();
        let waiting = m.submit(
            "waiting",
            Box::new(move |_| {
                ran2.fetch_add(1, Ordering::SeqCst);
                OpReport::default()
            }),
        );
        assert_eq!(m.status(waiting).unwrap().state, JobState::Queued);
        assert!(m.cancel(waiting));
        assert_eq!(m.status(waiting).unwrap().state, JobState::Cancelled);
        release_tx.send(()).unwrap();
        m.wait(blocker).unwrap();
        let st = m.wait(waiting).unwrap();
        assert_eq!(st.state, JobState::Cancelled);
        assert!(st.report.is_none(), "never-started job has no report");
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!(rec.tags(waiting), vec!["Q", "F"]);
        assert!(!m.cancel(9999), "unknown id reports false");
    }

    #[test]
    fn concurrency_limit_one_runs_jobs_one_after_another() {
        let m = JobManager::new(1, Arc::new(NullSink));
        let current = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut ids = Vec::new();
        for n in 0..3u32 {
            let (c, p, o) = (current.clone(), peak.clone(), order.clone());
            ids.push(m.submit(
                "job",
                Box::new(move |_| {
                    let now = c.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(now, Ordering::SeqCst);
                    o.lock().unwrap().push(n);
                    std::thread::sleep(Duration::from_millis(60));
                    c.fetch_sub(1, Ordering::SeqCst);
                    OpReport::default()
                }),
            ));
        }
        for id in ids {
            assert_eq!(m.wait(id).unwrap().state, JobState::Done);
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(
            *order.lock().unwrap(),
            vec![0, 1, 2],
            "start in submit order"
        );
    }

    #[test]
    fn concurrency_limit_two_allows_two_but_not_three() {
        let m = JobManager::new(2, Arc::new(NullSink));
        let current = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let ids: Vec<_> = (0..4)
            .map(|_| {
                let (c, p) = (current.clone(), peak.clone());
                m.submit(
                    "job",
                    Box::new(move |_| {
                        let now = c.fetch_add(1, Ordering::SeqCst) + 1;
                        p.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(80));
                        c.fetch_sub(1, Ordering::SeqCst);
                        OpReport::default()
                    }),
                )
            })
            .collect();
        for id in ids {
            m.wait(id).unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_when_nothing_done_done_when_partial_failure() {
        let m = JobManager::default();
        let dir = tempfile::tempdir().unwrap();
        let fail = |r: &mut OpReport| {
            r.failed
                .push((vp(dir.path()), Error::NotFound(dir.path().join("x"))));
        };
        let p = dir.path().to_path_buf();
        let all_failed = m.submit(
            "f",
            Box::new(move |_| {
                let mut r = OpReport::default();
                r.failed.push((vp(&p), Error::NotFound(p.join("x"))));
                r
            }),
        );
        let mut partial = done_report(&dir.path().join("ok"));
        fail(&mut partial);
        let partial_id = m.submit("p", Box::new(move |_| partial));
        let clean = done_report(&dir.path().join("ok"));
        let clean_id = m.submit("c", Box::new(move |_| clean));
        let empty_id = m.submit("e", Box::new(|_| OpReport::default()));

        let st = m.wait(all_failed).unwrap();
        assert_eq!(st.state, JobState::Failed);
        assert_eq!(st.report.unwrap().failed.len(), 1);
        let st = m.wait(partial_id).unwrap();
        assert_eq!(st.state, JobState::Done);
        let rep = st.report.unwrap();
        assert_eq!((rep.done.len(), rep.failed.len()), (1, 1));
        assert_eq!(m.wait(clean_id).unwrap().state, JobState::Done);
        assert_eq!(m.wait(empty_id).unwrap().state, JobState::Done);
    }

    #[test]
    fn report_cancelled_flag_yields_cancelled_state() {
        let m = JobManager::default();
        let id = m.submit(
            "c",
            Box::new(|_| OpReport {
                cancelled: true,
                ..OpReport::default()
            }),
        );
        assert_eq!(m.wait(id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn wait_blocks_until_finished_and_is_repeatable() {
        let rec = Arc::new(Record::default());
        let m = JobManager::new(1, rec.clone());
        let id = m.submit(
            "slow",
            Box::new(|_| {
                std::thread::sleep(Duration::from_millis(150));
                OpReport::default()
            }),
        );
        let started = Instant::now();
        let st = m.wait(id).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(st.state, JobState::Done);
        // Nach `wait` ist `Finished` schon ausgeliefert.
        assert_eq!(rec.tags(id).last(), Some(&"F"));
        assert_eq!(m.wait(id).unwrap().state, JobState::Done);
        assert!(m.wait(424242).is_none());
        assert!(m.status(424242).is_none());
    }

    #[test]
    fn list_returns_all_jobs_sorted_by_id() {
        let m = JobManager::default();
        let a = m.submit("a", Box::new(|_| OpReport::default()));
        let b = m.submit("b", Box::new(|_| OpReport::default()));
        m.wait(a);
        m.wait(b);
        let ids: Vec<_> = m.list().into_iter().map(|(i, _)| i).collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn panicking_job_fails_instead_of_hanging_wait() {
        let m = JobManager::default();
        let id = m.submit("boom", Box::new(|_| panic!("test panic")));
        let st = m.wait(id).unwrap();
        assert_eq!(st.state, JobState::Failed);
        // Nachfolgender Auftrag läuft trotzdem (Platz wurde freigegeben).
        let m1 = JobManager::new(1, Arc::new(NullSink));
        let a = m1.submit("boom", Box::new(|_| panic!("test panic")));
        let b = m1.submit("ok", Box::new(|_| OpReport::default()));
        m1.wait(a);
        assert_eq!(m1.wait(b).unwrap().state, JobState::Done);
    }
    fn recv<T>(rx: &mpsc::Receiver<T>) -> T {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("worker checkpoint timed out")
    }
    struct Notify(mpsc::Sender<(JobId, JobState)>);
    impl JobSink for Notify {
        fn event(&self, e: JobEvent) {
            if let JobEvent::Progress(id, s) = e {
                let _ = self.0.send((id, s.state));
            }
        }
    }
    fn blocked_work() -> (JobWork, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started, start) = mpsc::channel();
        let (release, released) = mpsc::channel();
        (
            Box::new(move |_| {
                started.send(()).unwrap();
                recv(&released);
                OpReport::default()
            }),
            start,
            release,
        )
    }

    #[test]
    fn pause_releases_limit_resume_requeues_and_keeps_lane() {
        let (events, event_rx) = mpsc::channel();
        let m = JobManager::new(1, Arc::new(Notify(events)));
        let (handles, handle_rx) = mpsc::channel();
        let (step, steps) = mpsc::channel();
        let (bytes, byte_rx) = mpsc::channel();
        let a = m.submit_with_lane(
            "a",
            Some("disk-a".into()),
            Box::new(move |h| {
                handles.send(h.clone()).unwrap();
                for n in 1..=2 {
                    recv(&steps);
                    if h.checkpoint() {
                        break;
                    }
                    bytes.send(n).unwrap();
                }
                OpReport::default()
            }),
        );
        let h = recv(&handle_rx);
        step.send(()).unwrap();
        assert_eq!(recv(&byte_rx), 1);
        let (bwork, bstart, brelease) = blocked_work();
        let b = m.submit_with_lane("b", Some("disk-a".into()), bwork);
        let (cwork, cstart, crelease) = blocked_work();
        let c = m.submit_with_lane("c", Some("disk-c".into()), cwork);
        h.pause();
        assert!(h.is_paused());
        step.send(()).unwrap();
        loop {
            if recv(&event_rx) == (a, JobState::Paused) {
                break;
            }
        }
        recv(&cstart);
        assert!(byte_rx.try_recv().is_err());
        assert!(bstart.try_recv().is_err());
        h.resume();
        assert_eq!(m.status(a).unwrap().state, JobState::Queued);
        assert!(byte_rx.try_recv().is_err());
        crelease.send(()).unwrap();
        m.wait(c);
        assert_eq!(recv(&byte_rx), 2);
        m.wait(a);
        recv(&bstart);
        brelease.send(()).unwrap();
        m.wait(b);
    }

    #[test]
    fn cancelled_paused_worker_wakes_and_releases_lane() {
        let (events, event_rx) = mpsc::channel();
        let m = JobManager::new(1, Arc::new(Notify(events)));
        let (started, start) = mpsc::channel();
        let (step, steps) = mpsc::channel();
        let a = m.submit_with_lane(
            "a",
            Some("disk".into()),
            Box::new(move |h| {
                started.send(()).unwrap();
                recv(&steps);
                assert!(h.checkpoint());
                OpReport::default()
            }),
        );
        recv(&start);
        m.pause(a);
        step.send(()).unwrap();
        loop {
            if recv(&event_rx) == (a, JobState::Paused) {
                break;
            }
        }
        let (work, start, release) = blocked_work();
        let b = m.submit_with_lane("b", Some("disk".into()), work);
        m.cancel(a);
        recv(&start);
        assert_eq!(m.wait(a).unwrap().state, JobState::Cancelled);
        release.send(()).unwrap();
        m.wait(b);
    }

    #[test]
    fn paused_waiter_preserves_fifo_other_lanes_pass() {
        let m = JobManager::new(1, Arc::new(NullSink));
        let (work, start, release) = blocked_work();
        let blocker = m.submit("block", work);
        recv(&start);
        let (aw, ast, ar) = blocked_work();
        let a = m.submit_with_lane("a", Some("disk".into()), aw);
        let (bw, bst, br) = blocked_work();
        let b = m.submit_with_lane("b", Some("disk".into()), bw);
        m.pause(a);
        assert_eq!(m.status(a).unwrap().state, JobState::Paused);
        assert_eq!(m.status(a).unwrap().queue_position, Some(1));
        assert_eq!(m.status(b).unwrap().queue_position, Some(2));
        let (cw, cst, cr) = blocked_work();
        let c = m.submit_with_lane("c", Some("other".into()), cw);
        release.send(()).unwrap();
        m.wait(blocker);
        recv(&cst);
        assert!(ast.try_recv().is_err());
        assert!(bst.try_recv().is_err());
        m.resume(a);
        cr.send(()).unwrap();
        m.wait(c);
        recv(&ast);
        assert!(bst.try_recv().is_err());
        ar.send(()).unwrap();
        m.wait(a);
        recv(&bst);
        br.send(()).unwrap();
        m.wait(b);
        assert_eq!(m.status(b).unwrap().queue_position, None);
        assert!(!m.pause(9999));
        assert!(!m.resume(9999));
    }

    #[test]
    fn lanes_parallel_fifo_and_release_on_failure_panic_cancel() {
        for outcome in 0..3 {
            let m = JobManager::new(2, Arc::new(NullSink));
            let (ready, ready_rx) = mpsc::channel();
            let (release, release_rx) = mpsc::channel();
            let a = m.submit_with_lane(
                "a",
                Some("a".into()),
                Box::new(move |_| {
                    ready.send(()).unwrap();
                    recv(&release_rx);
                    if outcome == 0 {
                        panic!("expected test panic");
                    }
                    let mut r = OpReport::default();
                    if outcome == 1 {
                        r.failed
                            .push((VPath::local("/").unwrap(), Error::VerifyFailed));
                    }
                    r
                }),
            );
            recv(&ready_rx);
            let (bw, bst, br) = blocked_work();
            let b = m.submit_with_lane("b", Some("a".into()), bw);
            let (cw, cst, cr) = blocked_work();
            let c = m.submit_with_lane("c", Some("c".into()), cw);
            recv(&cst);
            assert!(bst.try_recv().is_err());
            if outcome == 2 {
                m.cancel(a);
            }
            release.send(()).unwrap();
            recv(&bst);
            assert_eq!(
                m.wait(a).unwrap().state,
                if outcome == 2 {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                }
            );
            br.send(()).unwrap();
            cr.send(()).unwrap();
            m.wait(b);
            m.wait(c);
        }
    }

    #[test]
    fn eta_uses_ema_and_requires_two_seconds_and_nonzero_rate() {
        let mut rate = Rate::default();
        assert_eq!(rate.update(1.0, 100, 1000), None);
        assert_eq!(rate.update(2.0, 300, 1000), Some(6)); // EMA = 125
        assert_eq!(rate.smoothed, Some(125.0));
        assert_eq!(rate.update(3.0, 300, 1000), None);
        assert_eq!(rate.smoothed, Some(93.75));
        assert_eq!(Rate::default().update(3.0, 0, 100), None);
        assert_eq!(rate.update(4.0, 1000, 1000), Some(0));
    }

    #[test]
    fn copy_byte_counter_stops_at_paused_checkpoint() {
        struct CopyGate {
            first: AtomicBool,
            reached: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
            paused: mpsc::Sender<()>,
        }
        impl JobSink for CopyGate {
            fn event(&self, e: JobEvent) {
                if let JobEvent::Progress(_, s) = e {
                    if s.state == JobState::Paused {
                        let _ = self.paused.send(());
                    }
                    if s.bytes_done > 0 && !self.first.swap(true, Ordering::SeqCst) {
                        self.reached.send(()).unwrap();
                        recv(&self.release.lock().unwrap());
                    }
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big");
        let dst = dir.path().join("out");
        fs::write(&src, vec![7; 3 * 1024 * 1024]).unwrap();
        fs::create_dir(&dst).unwrap();
        let (reached, reached_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (paused, paused_rx) = mpsc::channel();
        let m = JobManager::new(
            1,
            Arc::new(CopyGate {
                first: AtomicBool::new(false),
                reached,
                release: Mutex::new(release_rx),
                paused,
            }),
        );
        let id = m.submit("copy", copy_work(vp(&src), vp(&dst)));
        recv(&reached_rx);
        m.pause(id);
        release.send(()).unwrap();
        recv(&paused_rx);
        assert_eq!(m.status(id).unwrap().bytes_done, 1024 * 1024);
        assert!(!dst.join("big").exists());
        m.resume(id);
        let status = m.wait(id).unwrap();
        assert_eq!(status.state, JobState::Done);
        assert_eq!(status.bytes_done, 3 * 1024 * 1024);
    }

    #[test]
    fn cancel_paused_queued_job_never_starts() {
        let m = JobManager::new(1, Arc::new(NullSink));
        let (work, start, release) = blocked_work();
        let a = m.submit("a", work);
        recv(&start);
        let b = m.submit_with_lane(
            "b",
            Some("disk".into()),
            Box::new(|_| panic!("must not start")),
        );
        assert_eq!(m.status(b).unwrap().queue_position, Some(1));
        m.pause(b);
        m.cancel(b);
        assert_eq!(m.wait(b).unwrap().state, JobState::Cancelled);
        assert_eq!(m.status(b).unwrap().queue_position, None);
        release.send(()).unwrap();
        m.wait(a);
        let c = m.submit_with_lane("c", Some("disk".into()), Box::new(|_| OpReport::default()));
        assert_eq!(m.wait(c).unwrap().state, JobState::Done);
    }
    #[test]
    fn pause_again_during_resume_does_not_pass_checkpoint() {
        struct RePause {
            handle: Mutex<Option<JobHandle>>,
            armed: AtomicBool,
            paused: mpsc::Sender<()>,
        }
        impl JobSink for RePause {
            fn event(&self, e: JobEvent) {
                if let JobEvent::Progress(_, s) = e {
                    if s.state == JobState::Running && self.armed.swap(false, Ordering::SeqCst) {
                        self.handle.lock().unwrap().as_ref().unwrap().pause();
                    }
                    if s.state == JobState::Paused {
                        let _ = self.paused.send(());
                    }
                }
            }
        }
        let (paused, paused_rx) = mpsc::channel();
        let sink = Arc::new(RePause {
            handle: Mutex::new(None),
            armed: AtomicBool::new(false),
            paused,
        });
        let m = JobManager::new(1, sink.clone());
        let (started, started_rx) = mpsc::channel();
        let (step, step_rx) = mpsc::channel();
        let (done, done_rx) = mpsc::channel();
        let id = m.submit(
            "a",
            Box::new(move |h| {
                started.send(h.clone()).unwrap();
                recv(&step_rx);
                done.send(h.checkpoint()).unwrap();
                OpReport::default()
            }),
        );
        *sink.handle.lock().unwrap() = Some(recv(&started_rx));
        m.pause(id);
        step.send(()).unwrap();
        recv(&paused_rx);
        sink.armed.store(true, Ordering::SeqCst);
        m.resume(id);
        recv(&paused_rx);
        assert!(done_rx.try_recv().is_err());
        m.cancel(id);
        assert!(recv(&done_rx));
        m.wait(id);
        sink.handle.lock().unwrap().take();
    }
}
