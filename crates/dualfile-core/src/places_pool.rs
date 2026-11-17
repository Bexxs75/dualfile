//! Fixed isolated workers: a timed-out syscall keeps its slot; refreshes cannot replace it.
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
type Work = Box<dyn FnOnce() + Send>;
pub struct Pool {
    sender: mpsc::SyncSender<Work>,
    active: Arc<KeySlots>,
}
#[derive(Default)]
struct KeySlots {
    deadlines: Mutex<std::collections::HashMap<String, KeyDeadline>>,
    available: Condvar,
}
enum KeyDeadline {
    Fixed(Instant),
    Progress(ProgressWatchdog, Duration),
}
impl KeyDeadline {
    fn remaining(&self, now: Instant) -> Duration {
        match self {
            Self::Fixed(expires) => expires.saturating_duration_since(now),
            Self::Progress(watchdog, idle) => {
                let state = watchdog.0.lock().unwrap_or_else(|e| e.into_inner());
                if state.expired {
                    Duration::ZERO
                } else if state.waiting {
                    // A cooperative pause is healthy, but waiters still have their
                    // own bounded wait and periodically check cancellation.
                    Duration::from_millis(50)
                } else {
                    idle.saturating_sub(now.saturating_duration_since(state.last))
                }
            }
        }
    }
}
struct Key(Arc<KeySlots>, String);
impl Drop for Key {
    fn drop(&mut self) {
        self.0
            .deadlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
        self.0.available.notify_all();
    }
}
/// A progress deadline measures inactivity, not the total duration of an operation.
#[derive(Clone)]
pub struct ProgressWatchdog(Arc<Mutex<WatchState>>);
struct WatchState {
    last: Instant,
    waiting: bool,
    expired: bool,
}
impl Default for ProgressWatchdog {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(WatchState {
            last: Instant::now(),
            waiting: false,
            expired: false,
        })))
    }
}
impl ProgressWatchdog {
    pub fn progress(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).last = Instant::now();
    }
    /// Suspend the inactivity clock only while waiting at a cooperative job checkpoint.
    pub fn checkpoint<T>(&self, wait: impl FnOnce() -> T) -> T {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).waiting = true;
        let started = Instant::now();
        let result = wait();
        let elapsed = started.elapsed();
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting = false;
        state.last += elapsed;
        result
    }
}
impl Pool {
    pub fn new(count: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Work>(count);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..count {
            let receiver = receiver.clone();
            std::thread::spawn(move || loop {
                let work = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                match work {
                    Ok(work) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
                    }
                    Err(_) => break,
                }
            });
        }
        Self {
            sender,
            active: Arc::default(),
        }
    }
    /// One outstanding syscall per path/mount, including after the response expires.
    /// Dropping queued or panicking work releases the key, but a hung syscall retains it.
    pub fn run_keyed<T: Send + 'static>(
        &self,
        key: String,
        deadline: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        self.run_keyed_policy(key, deadline, false, work)
    }
    /// Listings may wait for a healthy mount's outstanding request. Once its deadline
    /// has elapsed, reject new work until that syscall returns instead of filling slots.
    pub fn run_keyed_wait<T: Send + 'static>(
        &self,
        key: String,
        deadline: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        self.run_keyed_policy(key, deadline, true, work)
    }
    fn acquire(
        &self,
        key: String,
        expires: Instant,
        wait: bool,
        cancelled: impl Fn() -> bool,
        deadline: KeyDeadline,
    ) -> Option<Key> {
        let mut active = self
            .active
            .deadlines
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while let Some(holder) = active.get(&key) {
            let now = Instant::now();
            let remaining = holder.remaining(now);
            if !wait || remaining.is_zero() || now >= expires || cancelled() {
                return None;
            }
            active = self
                .active
                .available
                .wait_timeout(
                    active,
                    remaining.min(expires - now).min(Duration::from_millis(50)),
                )
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if Instant::now() >= expires || cancelled() {
            return None;
        }
        if let KeyDeadline::Progress(watchdog, _) = &deadline {
            // Waiting for another holder does not consume this work's idle budget.
            watchdog.progress();
        }
        active.insert(key.clone(), deadline);
        drop(active);
        Some(Key(self.active.clone(), key))
    }
    fn run_keyed_policy<T: Send + 'static>(
        &self,
        key: String,
        deadline: Duration,
        wait: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let expires = Instant::now() + deadline;
        let key = self.acquire(key, expires, wait, || false, KeyDeadline::Fixed(expires))?;
        self.run(
            expires.saturating_duration_since(Instant::now()),
            move || {
                let _key = key;
                work()
            },
        )
    }
    /// Keep one worker/key until the actual syscall returns, but release the caller
    /// after an inactivity timeout or cancellation. Healthy progress has no total limit.
    /// Before admission, wait at most `idle` for a healthy holder of the same key.
    pub fn run_keyed_watchdog<T: Send + 'static>(
        &self,
        key: String,
        idle: Duration,
        cancelled: impl Fn() -> bool,
        work: impl FnOnce(ProgressWatchdog) -> T + Send + 'static,
    ) -> Option<T> {
        let watchdog = ProgressWatchdog::default();
        let key = self.acquire(
            key,
            Instant::now() + idle,
            true,
            &cancelled,
            KeyDeadline::Progress(watchdog.clone(), idle),
        )?;
        let worker = watchdog.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Box::new(move || {
                if !worker.0.lock().unwrap_or_else(|e| e.into_inner()).expired {
                    let result = work(worker);
                    // Completion must release the key before waking the caller,
                    // which may immediately inspect another parent on this mount.
                    drop(key);
                    let _ = tx.send(result);
                }
            }))
            .ok()?;
        loop {
            let delay = {
                let mut state = watchdog.0.lock().unwrap_or_else(|e| e.into_inner());
                let remaining = idle.saturating_sub(state.last.elapsed());
                if cancelled() || (!state.waiting && remaining.is_zero()) {
                    state.expired = true;
                    return None;
                }
                if state.waiting {
                    Duration::from_millis(50)
                } else {
                    remaining.min(Duration::from_millis(50))
                }
            };
            match rx.recv_timeout(delay) {
                Ok(result) => return Some(result),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    pub fn run<T: Send + 'static>(
        &self,
        deadline: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        let expires = Instant::now() + deadline;
        self.sender
            .try_send(Box::new(move || {
                // Queued work that has already expired must never start a filesystem access.
                if Instant::now() < expires {
                    let _ = tx.send(work());
                }
            }))
            .ok()?;
        rx.recv_timeout(expires.saturating_duration_since(Instant::now()))
            .ok()
    }
}
pub fn control() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(2))
}
pub fn metadata() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(2))
}
pub fn space() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(1))
}
pub fn navigation() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(2))
}
pub fn helpers() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(2))
}
/// Potentially remote path probes never share the control-file or local-space workers.
pub fn paths() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(4))
}
pub fn network_space() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(2))
}
pub fn mount_checks() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(4))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_waiting_apis_follow_a_holders_progress_beyond_its_initial_deadline() {
        for watchdog_waiter in [false, true] {
            let pool = Pool::new(2);
            let (started, entered) = mpsc::channel();
            std::thread::scope(|scope| {
                let first = scope.spawn(|| {
                    pool.run_keyed_watchdog(
                        "/healthy".into(),
                        Duration::from_millis(300),
                        || false,
                        move |progress| {
                            started.send(()).unwrap();
                            for _ in 0..20 {
                                std::thread::sleep(Duration::from_millis(25));
                                progress.progress();
                            }
                            1
                        },
                    )
                });
                entered.recv_timeout(Duration::from_secs(2)).unwrap();
                let second = if watchdog_waiter {
                    pool.run_keyed_watchdog(
                        "/healthy".into(),
                        Duration::from_secs(2),
                        || false,
                        |_| 2,
                    )
                } else {
                    pool.run_keyed_wait("/healthy".into(), Duration::from_secs(2), || 2)
                };
                assert_eq!(second, Some(2));
                assert_eq!(first.join().unwrap(), Some(1));
            });
        }
    }
    #[test]
    fn stalled_watchdog_holder_refuses_only_its_own_key() {
        let pool = Pool::new(2);
        let (release, blocked) = mpsc::channel();
        assert_eq!(
            pool.run_keyed_watchdog(
                "/dead".into(),
                Duration::from_millis(30),
                || false,
                move |_| blocked.recv().unwrap(),
            ),
            None
        );
        let start = Instant::now();
        let watchdog = pool.run_keyed_watchdog(
            "/dead".into(),
            Duration::from_secs(1),
            || false,
            |_| panic!("must not queue more work on the stalled mount"),
        );
        let fixed = pool.run_keyed_wait("/dead".into(), Duration::from_secs(1), || {
            panic!("must not queue a probe on the stalled mount")
        });
        let elapsed = start.elapsed();
        let healthy =
            pool.run_keyed_watchdog("/healthy".into(), Duration::from_secs(1), || false, |_| 7);
        release.send(()).unwrap();
        assert_eq!(watchdog, None::<()>);
        assert_eq!(fixed, None::<()>);
        assert!(elapsed < Duration::from_millis(200));
        assert_eq!(healthy, Some(7));
    }
    #[test]
    fn waiting_for_a_healthy_holder_is_bounded_and_cancellable_without_queueing() {
        let pool = Pool::new(2);
        let (started, entered) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                pool.run_keyed_wait("/healthy".into(), Duration::from_secs(2), move || {
                    started.send(()).unwrap();
                    blocked.recv().unwrap();
                })
            });
            entered.recv_timeout(Duration::from_secs(1)).unwrap();
            let start = Instant::now();
            let expired = pool.run_keyed_watchdog(
                "/healthy".into(),
                Duration::from_millis(30),
                || false,
                |_| panic!("expired waiter must not start"),
            );
            let cancelled = pool.run_keyed_watchdog(
                "/healthy".into(),
                Duration::from_secs(1),
                || true,
                |_| panic!("cancelled waiter must not start"),
            );
            let elapsed = start.elapsed();
            release.send(()).unwrap();
            assert_eq!(first.join().unwrap(), Some(()));
            assert_eq!(expired, None::<()>);
            assert_eq!(cancelled, None::<()>);
            assert!(elapsed < Duration::from_millis(500));
        });
    }
    #[test]
    fn completed_watchdog_work_allows_immediate_reuse_of_the_mount() {
        let pool = Pool::new(2);
        for expected in 0..128 {
            assert_eq!(
                pool.run_keyed_watchdog(
                    "/mount".into(),
                    Duration::from_secs(1),
                    || false,
                    move |_| expected,
                ),
                Some(expected)
            );
        }
    }
    #[test]
    fn cooperative_pause_does_not_trip_the_progress_watchdog() {
        let pool = Pool::new(1);
        let result = pool.run_keyed_watchdog(
            "trash".into(),
            Duration::from_millis(100),
            || false,
            |progress| {
                progress.checkpoint(|| std::thread::sleep(Duration::from_millis(250)));
                progress.progress();
                42
            },
        );
        assert_eq!(result, Some(42));
    }
    #[test]
    fn concurrent_listings_on_a_healthy_mount_wait_and_both_succeed() {
        let pool = Pool::new(2);
        let (started, entered) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                pool.run_keyed_wait("/net".into(), Duration::from_secs(1), move || {
                    started.send(()).unwrap();
                    blocked.recv().unwrap();
                    1
                })
            });
            entered.recv_timeout(Duration::from_secs(1)).unwrap();
            let (called, waiting) = mpsc::channel();
            let (done, result) = mpsc::channel();
            let pool = &pool;
            scope.spawn(move || {
                called.send(()).unwrap();
                done.send(pool.run_keyed_wait("/net".into(), Duration::from_secs(1), || 2))
                    .unwrap();
            });
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
            // An immediate rejection would send None here, rather than keep waiting.
            let early = result.recv_timeout(Duration::from_millis(30));
            release.send(()).unwrap();
            assert_eq!(early, Err(mpsc::RecvTimeoutError::Timeout));
            assert_eq!(first.join().unwrap(), Some(1));
            assert_eq!(
                result.recv_timeout(Duration::from_secs(1)).unwrap(),
                Some(2)
            );
        });
    }
    #[test]
    fn a_known_hanging_mount_does_not_queue_more_listings_or_starve_others() {
        let pool = Pool::new(2);
        let (release, blocked) = mpsc::channel();
        assert_eq!(
            pool.run_keyed_wait("/dead".into(), Duration::from_millis(20), move || blocked
                .recv()
                .unwrap()),
            None
        );
        let now = Instant::now();
        assert_eq!(
            pool.run_keyed_wait("/dead".into(), Duration::from_secs(1), || panic!(
                "known hung mount"
            )),
            None::<()>
        );
        assert!(now.elapsed() < Duration::from_millis(200));
        assert_eq!(
            pool.run_keyed_wait("/healthy".into(), Duration::from_secs(1), || 3),
            Some(3)
        );
        release.send(()).unwrap();
    }
    #[test]
    fn a_hung_mount_retains_one_slot_and_home_stays_available() {
        let pool = Pool::new(2);
        let (release, block) = mpsc::channel();
        assert_eq!(
            pool.run_keyed("/net".into(), Duration::from_millis(20), move || block
                .recv()
                .unwrap()),
            None
        );
        for _ in 0..5 {
            assert_eq!(
                pool.run_keyed("/net".into(), Duration::from_millis(20), || panic!(
                    "duplicate probe"
                )),
                None::<()>
            );
        }
        // Home normally bypasses navigation; even another mount can use the other slot.
        assert_eq!(
            pool.run_keyed("/home".into(), Duration::from_secs(1), || 7),
            Some(7)
        );
        release.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(1);
        while pool
            .run_keyed("/net".into(), Duration::from_millis(20), || 8)
            .is_none()
        {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    #[test]
    fn hung_work_is_bounded_and_not_replaced() {
        let pool = Pool::new(1);
        let (release, block) = mpsc::channel();
        let start = Instant::now();
        assert_eq!(
            pool.run(Duration::from_millis(30), move || {
                block.recv().unwrap();
                1
            }),
            None
        );
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..10 {
            let c = count.clone();
            assert_eq!(
                pool.run(Duration::from_millis(10), move || c
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)),
                None
            );
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(pool.run(Duration::from_secs(1), || 7), Some(7));
    }
}
