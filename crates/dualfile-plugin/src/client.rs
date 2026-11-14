//! One plugin as the program uses it: starts the process on demand (always from a freshly
//! verified package that must match what the user confirmed), keeps one session, serializes
//! the requests, and switches the plugin off when it keeps failing.

use crate::error::{PluginError, PluginResult};
use crate::host::Session;
use crate::manifest::{Kind, Manifest};
use crate::package::{verify_package, PinnedState};
use crate::protocol::{self, FsCapabilities, PackerEntry, PluginEntry, Preview};
use crate::runtime::PluginRuntime;
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Item of a `column.values` request.
#[derive(Clone, Debug)]
pub struct ColumnItem {
    pub name: String,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

/// How a plugin is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Health {
    /// Failures in a row (crash, time-out, protocol error, failed start).
    pub failures: u32,
    /// Switched off after too many failures; needs `reset_health`.
    pub disabled: bool,
    /// A process is running right now.
    pub running: bool,
}

/// Why a plugin was switched off by itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisabledCause {
    /// Too many failures in a row (crash, time-out, protocol error).
    Failures,
    /// The package no longer matches what the user confirmed.
    Locked,
}

/// Called (with the id) when the plugin switches itself off after repeated failures or turns
/// out to be locked. It runs while the client's internal lock is held: it must only note the
/// fact and never call back into the client.
pub type DisabledHook = Arc<dyn Fn(&str, DisabledCause) + Send + Sync>;

#[derive(Default)]
struct State {
    session: Option<Session>,
    generation: u64,
    failures: u32,
    last_failure: Option<Instant>,
    disabled: bool,
    caps: Option<FsCapabilities>,
    on_disabled: Option<DisabledHook>,
}

pub struct PluginClient {
    runtime: Arc<PluginRuntime>,
    pinned: PinnedState,
    manifest: Manifest,
    state: Mutex<State>,
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

impl PluginClient {
    /// Verifies the installed package and checks it against `pinned` (`Locked` otherwise).
    /// Nothing runs yet.
    pub fn open(runtime: Arc<PluginRuntime>, pinned: PinnedState) -> PluginResult<Arc<Self>> {
        let package = verify_package(&runtime.config().base_dir, &pinned.id)?;
        pinned.check(&package)?;
        Ok(Arc::new(Self {
            runtime,
            manifest: package.manifest.clone(),
            pinned,
            state: Mutex::new(State::default()),
        }))
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn pinned(&self) -> &PinnedState {
        &self.pinned
    }

    pub fn health(&self) -> Health {
        let mut state = self.state.lock().unwrap();
        let running = state.session.as_mut().is_some_and(Session::is_alive);
        Health {
            failures: state.failures,
            disabled: state.disabled,
            running,
        }
    }

    /// Sets the function that is told when the plugin switches itself off (see `DisabledHook`).
    pub fn set_disabled_hook(&self, hook: DisabledHook) {
        self.state.lock().unwrap().on_disabled = Some(hook);
    }

    /// Starts the process now (verified package, sandbox, first exchange) if it is not running;
    /// a failure counts like any other. Used when the user activates a plugin, so a plugin that
    /// cannot start is reported right away.
    pub fn start(&self) -> PluginResult<()> {
        self.with_session(None, |_, _| Ok(()))
    }

    /// Re-enables a plugin that was switched off (the user's decision).
    pub fn reset_health(&self) {
        let mut state = self.state.lock().unwrap();
        state.failures = 0;
        state.last_failure = None;
        state.disabled = false;
    }

    /// Ends the process (if any); the next request starts a new one.
    pub fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        if let Some(session) = state.session.take() {
            session.close();
        }
    }

    /// Number of the current session; changes whenever the process was replaced (open write
    /// handles of an older session are gone).
    pub fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }

    /// The cleaned standard-error text of the running process, for a diagnosis view.
    pub fn diagnostic(&self) -> String {
        let state = self.state.lock().unwrap();
        state
            .session
            .as_ref()
            .map(|session| session.diagnostic().as_str().to_owned())
            .unwrap_or_default()
    }

    fn record_failure(&self, state: &mut State) {
        state.failures += 1;
        state.last_failure = Some(Instant::now());
        state.generation += 1;
        if let Some(mut session) = state.session.take() {
            session.kill();
        }
        if state.failures >= self.runtime.config().max_failures && !state.disabled {
            state.disabled = true;
            self.runtime.log(&format!(
                "Plugin {} nach {} Fehlern in Folge deaktiviert",
                self.manifest.id, state.failures
            ));
            if let Some(hook) = &state.on_disabled {
                hook(&self.manifest.id, DisabledCause::Failures);
            }
        }
    }

    fn backoff_active(&self, state: &State) -> bool {
        let Some(last) = state.last_failure else {
            return false;
        };
        if state.failures == 0 {
            return false;
        }
        let steps = (state.failures - 1).min(6);
        let wait = self.runtime.config().backoff.saturating_mul(1 << steps);
        last.elapsed() < wait
    }

    fn with_session<T>(
        &self,
        needs: Option<Kind>,
        run: impl FnOnce(&mut Session, &mut State) -> PluginResult<T>,
    ) -> PluginResult<T> {
        if let Some(kind) = needs {
            if !self.manifest.has_kind(kind) {
                return Err(PluginError::Unsupported(
                    "Das Plugin bietet diese Art nicht an",
                ));
            }
        }
        let mut state = self.state.lock().unwrap();
        if state.disabled {
            return Err(PluginError::Disabled);
        }
        if state
            .session
            .as_mut()
            .is_some_and(|session| !session.is_alive())
        {
            // it died between two requests: replaced below, counted as a failure
            self.record_failure(&mut state);
            if state.disabled {
                return Err(PluginError::Disabled);
            }
        }
        if state.session.is_none() {
            if self.backoff_active(&state) {
                return Err(PluginError::Backoff);
            }
            let started = verify_package(&self.runtime.config().base_dir, &self.pinned.id)
                .and_then(|package| Session::start(&self.runtime, &package, &self.pinned));
            match started {
                Ok(session) => {
                    state.generation += 1;
                    state.caps = None;
                    state.session = Some(session);
                }
                Err(error) => {
                    if error.is_process_failure() {
                        self.record_failure(&mut state);
                    }
                    if matches!(error, PluginError::Locked) {
                        if let Some(hook) = &state.on_disabled {
                            hook(&self.manifest.id, DisabledCause::Locked);
                        }
                    }
                    return Err(error);
                }
            }
        }
        if let Some(kind) = needs {
            if !state
                .session
                .as_ref()
                .is_some_and(|s| s.kinds.contains(&kind))
            {
                return Err(PluginError::Unsupported(
                    "Das Plugin bietet diese Art nicht an",
                ));
            }
        }
        let mut session = state.session.take().expect("session present");
        let outcome = run(&mut session, &mut state);
        match &outcome {
            Err(error) if error.is_process_failure() => {
                state.session = Some(session);
                self.record_failure(&mut state);
            }
            _ => {
                state.failures = 0;
                state.last_failure = None;
                state.session = Some(session);
            }
        }
        outcome
    }

    fn call(&self, kind: Kind, op: &str, params: Value) -> PluginResult<Value> {
        let timeout = self.runtime.config().request_timeout;
        self.with_session(Some(kind), |session, _| {
            session.call(op, object(params), timeout)
        })
    }

    /// Sends any request (test and diagnosis use; no schema check beyond the envelope).
    #[doc(hidden)]
    pub fn raw_call(&self, op: &str, params: Value) -> PluginResult<Value> {
        let timeout = self.runtime.config().request_timeout;
        self.with_session(None, |session, _| session.call(op, object(params), timeout))
    }

    #[doc(hidden)]
    pub fn raw_call_with_timeout(
        &self,
        op: &str,
        params: Value,
        timeout: Duration,
    ) -> PluginResult<Value> {
        self.with_session(None, |session, _| session.call(op, object(params), timeout))
    }

    /// Pid of the running process (tests: zombie and group checks).
    #[doc(hidden)]
    pub fn pid(&self) -> Option<u32> {
        self.state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(Session::pid)
    }

    #[doc(hidden)]
    pub fn is_sandboxed(&self) -> Option<bool> {
        self.state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(Session::is_sandboxed)
    }

    // ------------------------------------------------------------ filesystem

    /// What the plugin's file system can do; the plugin's claim is cut down by the manifest
    /// (`write` only if the manifest allows it).
    pub fn fs_capabilities(&self) -> PluginResult<FsCapabilities> {
        if let Some(caps) = self.state.lock().unwrap().caps {
            return Ok(caps);
        }
        let result = self.call(Kind::Filesystem, "fs.capabilities", json!({}))?;
        let mut caps = protocol::parse_capabilities(&result)?;
        caps.write &= self.manifest.write;
        self.state.lock().unwrap().caps = Some(caps);
        Ok(caps)
    }

    fn require_write(&self) -> PluginResult<()> {
        if !self.manifest.write {
            return Err(PluginError::Unsupported("Dieses Plugin darf nichts ändern"));
        }
        if !self.fs_capabilities()?.write {
            return Err(PluginError::Unsupported("Das Plugin kann nichts ändern"));
        }
        Ok(())
    }

    pub fn fs_list(&self, path: &str) -> PluginResult<Vec<PluginEntry>> {
        protocol::parse_list(&self.call(Kind::Filesystem, "fs.list", json!({"path": path}))?)
    }

    pub fn fs_stat(&self, path: &str, name: &str) -> PluginResult<PluginEntry> {
        protocol::parse_stat(
            &self.call(Kind::Filesystem, "fs.stat", json!({"path": path}))?,
            name,
        )
    }

    pub fn fs_read(&self, path: &str, offset: u64, len: usize) -> PluginResult<Vec<u8>> {
        let len = len.min(protocol::MAX_READ_CHUNK);
        let result = self.call(
            Kind::Filesystem,
            "fs.read",
            json!({"path": path, "offset": offset, "len": len}),
        )?;
        protocol::parse_data(&result, len)
    }

    pub fn fs_mkdir(&self, path: &str) -> PluginResult<()> {
        self.require_write()?;
        protocol::parse_empty(&self.call(Kind::Filesystem, "fs.mkdir", json!({"path": path}))?)
    }

    pub fn fs_rename(&self, from: &str, to: &str, replace: bool) -> PluginResult<()> {
        self.require_write()?;
        let params = json!({"from": from, "to": to, "replace": replace});
        protocol::parse_empty(&self.call(Kind::Filesystem, "fs.rename", params)?)
    }

    pub fn fs_remove(&self, path: &str) -> PluginResult<()> {
        self.require_write()?;
        protocol::parse_empty(&self.call(Kind::Filesystem, "fs.remove", json!({"path": path}))?)
    }

    /// Opens a write handle; returns it with the session generation it belongs to.
    pub fn fs_write_open(&self, path: &str, replace: bool) -> PluginResult<(u32, u64)> {
        self.require_write()?;
        let timeout = self.runtime.config().request_timeout;
        self.with_session(Some(Kind::Filesystem), |session, state| {
            let result = session.call(
                "fs.write_open",
                object(json!({"path": path, "replace": replace})),
                timeout,
            )?;
            Ok((protocol::parse_handle(&result)?, state.generation))
        })
    }

    pub fn fs_write_chunk(&self, handle: u32, generation: u64, data: &[u8]) -> PluginResult<()> {
        if data.len() > protocol::MAX_WRITE_CHUNK {
            return Err(PluginError::Protocol("Der Datenblock ist zu groß"));
        }
        let timeout = self.runtime.config().request_timeout;
        self.with_session_generation(generation, |session| {
            let params = object(json!({"handle": handle, "data": protocol::encode_bytes(data)}));
            protocol::parse_empty(&session.call("fs.write_chunk", params, timeout)?)
        })
    }

    pub fn fs_write_close(&self, handle: u32, generation: u64, commit: bool) -> PluginResult<()> {
        let timeout = self.runtime.config().request_timeout;
        self.with_session_generation(generation, |session| {
            let params = object(json!({"handle": handle, "commit": commit}));
            protocol::parse_empty(&session.call("fs.write_close", params, timeout)?)
        })
    }

    /// Runs `run` only if the session is still the one the handle was opened in.
    fn with_session_generation<T>(
        &self,
        generation: u64,
        run: impl FnOnce(&mut Session) -> PluginResult<T>,
    ) -> PluginResult<T> {
        self.with_session(Some(Kind::Filesystem), |session, state| {
            if state.generation != generation {
                return Err(PluginError::Crashed);
            }
            run(session)
        })
    }

    // ------------------------------------------------------------ packer, column, preview

    pub fn packer_list(&self, name: &str, archive: &[u8]) -> PluginResult<Vec<PackerEntry>> {
        if archive.len() > protocol::MAX_PACKER_INPUT {
            return Err(PluginError::Unsupported(
                "Das Archiv ist für das Plugin zu groß",
            ));
        }
        let params = json!({"name": name, "data": protocol::encode_bytes(archive)});
        protocol::parse_packer_list(&self.call(Kind::Packer, "packer.list", params)?)
    }

    pub fn packer_read(&self, name: &str, archive: &[u8], member: &str) -> PluginResult<Vec<u8>> {
        if archive.len() > protocol::MAX_PACKER_INPUT {
            return Err(PluginError::Unsupported(
                "Das Archiv ist für das Plugin zu groß",
            ));
        }
        let params = json!({"name": name, "data": protocol::encode_bytes(archive), "path": member});
        protocol::parse_data(
            &self.call(Kind::Packer, "packer.read", params)?,
            protocol::MAX_PACKER_READ,
        )
    }

    pub fn column_values(
        &self,
        column: &str,
        items: &[ColumnItem],
    ) -> PluginResult<Vec<Option<String>>> {
        let declared = self
            .manifest
            .columns
            .as_ref()
            .is_some_and(|cols| cols.iter().any(|c| c.id == column));
        if !declared {
            return Err(PluginError::Unsupported(
                "Die Spalte gehört nicht zum Plugin",
            ));
        }
        if items.len() > protocol::MAX_COLUMN_ITEMS {
            return Err(PluginError::Unsupported(
                "Zu viele Dateien für einen Aufruf",
            ));
        }
        let list: Vec<Value> = items
            .iter()
            .map(|item| {
                let mut entry = Map::new();
                entry.insert("name".into(), Value::from(item.name.as_str()));
                if let Some(size) = item.size {
                    entry.insert("size".into(), Value::from(size));
                }
                if let Some(modified) = item.modified {
                    entry.insert("modified".into(), Value::from(secs(modified)));
                }
                Value::Object(entry)
            })
            .collect();
        let result = self.call(
            Kind::Column,
            "column.values",
            json!({"column": column, "items": list}),
        )?;
        protocol::parse_values(&result, items.len())
    }

    pub fn preview_render(
        &self,
        name: &str,
        data: &[u8],
        max_chars: usize,
    ) -> PluginResult<Preview> {
        if data.len() > protocol::MAX_PREVIEW_INPUT {
            return Err(PluginError::Unsupported(
                "Die Datei ist für die Vorschau zu groß",
            ));
        }
        let params =
            json!({"name": name, "data": protocol::encode_bytes(data), "max_chars": max_chars});
        protocol::parse_preview(&self.call(Kind::Preview, "preview.render", params)?)
    }
}

impl Drop for PluginClient {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(session) = state.session.take() {
                session.close();
            }
        }
    }
}
