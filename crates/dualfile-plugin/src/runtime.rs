//! Settings shared by all plugin processes of the program: limits, time limits, the cap on
//! concurrently running plugin processes and the policy for plugins that keep failing.

use crate::error::{PluginError, PluginResult};
use crate::host::ResourceLimits;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct RuntimeConfig {
    /// The plugin folder (`~/.local/share/dualfile/plugins`); a parameter so tests use their own.
    pub base_dir: PathBuf,
    pub limits: ResourceLimits,
    /// Most plugin processes alive at the same time.
    pub max_processes: usize,
    /// Time limit for one request.
    pub request_timeout: Duration,
    /// Time limit for the first exchange (`hello`).
    pub hello_timeout: Duration,
    /// Longest answer line accepted from a plugin, in bytes.
    pub max_line_bytes: usize,
    /// Longest request line sent to a plugin, in bytes.
    pub max_request_bytes: usize,
    /// Most standard-error bytes kept per plugin process.
    pub stderr_cap: usize,
    /// After this many failures in a row (crash, timeout, protocol error) the plugin is
    /// deactivated until `PluginClient::reset_health`.
    pub max_failures: u32,
    /// Pause after the first failure; doubles with every further failure in a row.
    pub backoff: Duration,
    /// Test hook: pretend the sandbox cannot be built (the plugin must then not start).
    #[doc(hidden)]
    pub simulate_sandbox_unavailable: bool,
    /// Receives log lines (default: standard error of the program).
    pub log: Option<LogSink>,
}

impl RuntimeConfig {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            limits: ResourceLimits::default(),
            max_processes: 4,
            request_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(5),
            max_line_bytes: 4 * 1024 * 1024,
            max_request_bytes: 8 * 1024 * 1024,
            stderr_cap: 4096,
            max_failures: 3,
            backoff: Duration::from_millis(250),
            simulate_sandbox_unavailable: false,
            log: None,
        }
    }
}

pub struct PluginRuntime {
    config: RuntimeConfig,
    live: Arc<AtomicUsize>,
    /// A long-lived thread that starts the plugin processes. The kernel's parent-death signal
    /// (`PR_SET_PDEATHSIG`) fires when the *thread* that started a process ends, so a plugin
    /// must never be started from a short-lived job thread. The thread ends with the runtime.
    spawner: Mutex<Option<Sender<Job>>>,
}

/// Counts one running plugin process; released when dropped.
pub(crate) struct ProcessPermit {
    live: Arc<AtomicUsize>,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PluginRuntime {
    pub fn new(config: RuntimeConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            live: Arc::new(AtomicUsize::new(0)),
            spawner: Mutex::new(None),
        })
    }

    /// Runs `job` on the spawner thread and returns its result.
    pub(crate) fn run_on_spawner<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
    ) -> PluginResult<R> {
        let (reply, answer) = channel();
        let work: Job = Box::new(move || {
            let _ = reply.send(job());
        });
        {
            let mut spawner = self.spawner.lock().unwrap();
            if spawner.is_none() {
                let (sender, receiver) = channel::<Job>();
                std::thread::Builder::new()
                    .name("dualfile-plugin-spawner".into())
                    .spawn(move || {
                        while let Ok(job) = receiver.recv() {
                            job();
                        }
                    })
                    .map_err(|_| PluginError::Spawn)?;
                *spawner = Some(sender);
            }
            spawner
                .as_ref()
                .expect("spawner thread present")
                .send(work)
                .map_err(|_| PluginError::Spawn)?;
        }
        answer.recv().map_err(|_| PluginError::Spawn)
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Plugin processes alive right now.
    pub fn live_processes(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    pub(crate) fn acquire(&self) -> PluginResult<ProcessPermit> {
        let mut current = self.live.load(Ordering::SeqCst);
        loop {
            if current >= self.config.max_processes {
                return Err(PluginError::TooManyProcesses);
            }
            match self.live.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Ok(ProcessPermit {
                        live: self.live.clone(),
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }

    pub fn log(&self, line: &str) {
        match &self.config.log {
            Some(sink) => sink(line),
            None => log::warn!("{line}"),
        }
    }
}
