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
    deadlines: Mutex<std::collections::HashMap<String, Instant>>,
    available: Condvar,
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
    fn run_keyed_policy<T: Send + 'static>(
        &self,
        key: String,
        deadline: Duration,
        wait: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let expires = Instant::now() + deadline;
        let mut active = self
            .active
            .deadlines
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while let Some(busy_until) = active.get(&key).copied() {
            let now = Instant::now();
            if !wait || now >= busy_until || now >= expires {
                return None;
            }
            active = self
                .active
                .available
                .wait_timeout(active, busy_until.min(expires) - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if Instant::now() >= expires {
            return None;
        }
        active.insert(key.clone(), expires);
        drop(active);
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
        let key = Key(self.active.clone(), key);
        self.run(
            expires.saturating_duration_since(Instant::now()),
            move || {
                let _key = key;
                work()
            },
        )
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
