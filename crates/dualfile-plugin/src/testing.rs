//! Helpers for the tests of this crate and of the program that embeds it: installs the
//! example plugin (normal or test double) into a plugin folder with a correct manifest.
//! Not part of the plugin interface.

use crate::example_binaries::{EXAMPLE_PLUGIN, EXAMPLE_PLUGIN_TESTBIN};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What to install.
#[derive(Clone, Debug)]
pub struct InstallSpec {
    pub id: String,
    /// The test double (writable tree, misbehaving modes) instead of the normal example.
    pub testbin: bool,
    /// The manifest asks for the right to change files.
    pub write: bool,
    /// Content of the `mode` file next to the program (test double only).
    pub mode: Option<String>,
}

impl InstallSpec {
    pub fn normal(id: &str) -> Self {
        Self {
            id: id.into(),
            testbin: false,
            write: false,
            mode: None,
        }
    }

    pub fn testbin(id: &str) -> Self {
        Self {
            testbin: true,
            ..Self::normal(id)
        }
    }

    pub fn writable(mut self) -> Self {
        self.write = true;
        self
    }

    pub fn mode(mut self, mode: &str) -> Self {
        self.mode = Some(mode.into());
        self
    }
}

fn write_file(path: &Path, data: &[u8], mode: u32) {
    std::fs::write(path, data).expect("write test file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

/// Installs the plugin as `<base>/<id>` (base folder made private) and returns its folder.
pub fn install_example(base: &Path, spec: &InstallSpec) -> PathBuf {
    std::fs::create_dir_all(base).expect("plugin base folder");
    std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let dir = base.join(&spec.id);
    std::fs::create_dir(&dir).expect("plugin folder");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let binary = if spec.testbin {
        EXAMPLE_PLUGIN_TESTBIN
    } else {
        EXAMPLE_PLUGIN
    };
    let exe = std::fs::read(binary).expect("example plugin binary");
    write_file(&dir.join("example-plugin"), &exe, 0o755);
    let mut files = vec![("example-plugin".to_owned(), sha_hex(&exe))];
    if let Some(mode) = &spec.mode {
        write_file(&dir.join("mode"), mode.as_bytes(), 0o644);
        files.push(("mode".to_owned(), sha_hex(mode.as_bytes())));
    }
    files.sort();
    let manifest: Value = json!({
        "id": spec.id, "name": "Beispiel", "version": "1.0.0", "api": 1,
        "exec": "example-plugin",
        "kinds": ["filesystem", "packer", "column", "preview"],
        "write": spec.write,
        "filesystem": {"scheme": "example"},
        "packer": {"extensions": ["xyz"]},
        "columns": [{"id": "len", "title": "Länge"}, {"id": "upper", "title": "Gross"}],
        "preview": {"extensions": ["txt", "png"], "mime": ["text/plain"]},
        "files": files.iter().map(|(p, h)| json!({"path": p, "sha256": h})).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec_pretty(&manifest).expect("manifest");
    write_file(&dir.join("plugin.json"), &bytes, 0o644);
    dir
}

/// Changes the manifest of an installed plugin (new version text): a valid package whose
/// manifest differs from what was confirmed.
pub fn change_manifest(dir: &Path) {
    let path = dir.join("plugin.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    manifest["version"] = json!("2.0.0");
    write_file(
        &path,
        &serde_json::to_vec_pretty(&manifest).expect("manifest"),
        0o644,
    );
}
