//! Shared helpers: installs the example plugin (normal or test double) into a temporary
//! plugin folder with a correct manifest.

#![allow(dead_code)]

use dualfile_plugin::{PinnedState, PluginClient, PluginRuntime, RuntimeConfig};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

pub const NORMAL: &str = env!("DUALFILE_EXAMPLE_PLUGIN");
pub const TESTBIN: &str = env!("DUALFILE_EXAMPLE_PLUGIN_TESTBIN");

pub fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Clone)]
pub struct Spec {
    pub id: String,
    pub binary: PathBuf,
    pub kinds: Vec<&'static str>,
    pub write: bool,
    pub extra_files: Vec<(String, Vec<u8>)>,
}

impl Spec {
    pub fn testbin(id: &str) -> Self {
        Self {
            id: id.into(),
            binary: TESTBIN.into(),
            kinds: vec!["filesystem", "packer", "column", "preview"],
            write: false,
            extra_files: Vec::new(),
        }
    }

    pub fn normal(id: &str) -> Self {
        Self {
            binary: NORMAL.into(),
            ..Self::testbin(id)
        }
    }

    pub fn writable(mut self) -> Self {
        self.write = true;
        self
    }

    pub fn mode(mut self, mode: &str) -> Self {
        self.extra_files
            .push(("mode".into(), mode.as_bytes().to_vec()));
        self
    }
}

pub fn manifest_json(spec: &Spec, exe_bytes: &[u8]) -> Value {
    let mut files: Vec<(String, String)> = vec![("example-plugin".into(), sha(exe_bytes))];
    for (name, data) in &spec.extra_files {
        files.push((name.clone(), sha(data)));
    }
    files.sort();
    let mut manifest = json!({
        "id": spec.id, "name": "Beispiel", "version": "1.0.0", "api": 1,
        "exec": "example-plugin", "kinds": spec.kinds, "write": spec.write,
        "files": files.iter().map(|(p, h)| json!({"path": p, "sha256": h})).collect::<Vec<_>>(),
    });
    let object = manifest.as_object_mut().unwrap();
    if spec.kinds.contains(&"filesystem") {
        object.insert("filesystem".into(), json!({"scheme": "example"}));
    }
    if spec.kinds.contains(&"packer") {
        object.insert("packer".into(), json!({"extensions": ["xyz"]}));
    }
    if spec.kinds.contains(&"column") {
        object.insert(
            "columns".into(),
            json!([{"id": "len", "title": "Länge"}, {"id": "upper", "title": "Gross"}]),
        );
    }
    if spec.kinds.contains(&"preview") {
        object.insert(
            "preview".into(),
            json!({"extensions": ["txt", "png"], "mime": ["text/plain"]}),
        );
    }
    manifest
}

pub fn write_file(path: &Path, data: &[u8], mode: u32) {
    std::fs::write(path, data).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Installs the plugin below `base`; returns the plugin folder.
pub fn install(base: &Path, spec: &Spec) -> PathBuf {
    let dir = base.join(&spec.id);
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let exe = std::fs::read(&spec.binary).unwrap();
    write_file(&dir.join("example-plugin"), &exe, 0o755);
    for (name, data) in &spec.extra_files {
        write_file(&dir.join(name), data, 0o644);
    }
    let manifest = serde_json::to_vec_pretty(&manifest_json(spec, &exe)).unwrap();
    write_file(&dir.join("plugin.json"), &manifest, 0o644);
    dir
}

pub struct Env {
    pub base_dir: TempDir,
    pub log: Arc<Mutex<Vec<String>>>,
}

impl Env {
    pub fn new() -> Self {
        let base_dir = tempfile::Builder::new()
            .prefix("dualfile-plugin-test-")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(base_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            base_dir,
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn base(&self) -> &Path {
        self.base_dir.path()
    }

    pub fn config(&self) -> RuntimeConfig {
        let mut config = RuntimeConfig::new(self.base());
        config.request_timeout = std::time::Duration::from_secs(5);
        config.backoff = std::time::Duration::ZERO;
        let log = self.log.clone();
        config.log = Some(Arc::new(move |line| {
            log.lock().unwrap().push(line.to_owned())
        }));
        config
    }

    pub fn runtime(&self) -> Arc<PluginRuntime> {
        PluginRuntime::new(self.config())
    }

    /// Installs and opens a client with the state the user would have confirmed.
    pub fn client(&self, spec: &Spec) -> Arc<PluginClient> {
        install(self.base(), spec);
        self.client_for(spec, self.runtime())
    }

    pub fn client_for(&self, spec: &Spec, runtime: Arc<PluginRuntime>) -> Arc<PluginClient> {
        let package = dualfile_plugin::verify_package(self.base(), &spec.id).unwrap();
        PluginClient::open(runtime, PinnedState::for_package(&package, false)).unwrap()
    }
}
