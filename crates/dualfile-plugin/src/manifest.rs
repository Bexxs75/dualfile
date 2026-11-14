//! `plugin.json`: what a plugin package declares. Everything is validated strictly: unknown
//! fields are refused, lengths and counts are bounded, paths are relative and clean, and the
//! list of package files (with SHA-256) is sorted and unique. The manifest is trusted only as
//! far as the user confirmed this exact file (see `PinnedState`).

use crate::error::{PluginError, PluginResult};
use dualfile_core::vfs::is_valid_connection_id;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The only protocol version this host speaks.
pub const API_VERSION: u32 = 1;
/// Name of the manifest file inside the plugin folder.
pub const MANIFEST_FILE: &str = "plugin.json";
/// Largest accepted manifest, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_FILES: usize = 64;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_VERSION_BYTES: usize = 32;
pub const MAX_FILE_PATH_BYTES: usize = 128;
pub const MAX_FILE_PATH_DEPTH: usize = 4;
pub const MAX_LIST_ITEMS: usize = 16;

/// Schemes and extensions a plugin must not claim: the built-in ones.
const RESERVED_SCHEMES: [&str; 9] = [
    "file", "local", "archive", "ftp", "ftps", "sftp", "plugin", "http", "https",
];
const RESERVED_EXTENSIONS: [&str; 12] = [
    "zip", "7z", "tar", "gz", "bz2", "xz", "tgz", "tbz2", "txz", "rar", "jar", "apk",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Filesystem,
    Packer,
    Column,
    Preview,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FsDetails {
    /// Name of the file system, like `ftp` for FTP (`[a-z][a-z0-9-]{0,15}`).
    pub scheme: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackerDetails {
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColumnDef {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreviewDetails {
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub mime: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: u32,
    /// The executable, relative to the plugin folder; must be listed in `files`.
    pub exec: String,
    pub kinds: Vec<Kind>,
    /// The plugin may serve changing operations of its file system (default off).
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub filesystem: Option<FsDetails>,
    #[serde(default)]
    pub packer: Option<PackerDetails>,
    #[serde(default)]
    pub columns: Option<Vec<ColumnDef>>,
    #[serde(default)]
    pub preview: Option<PreviewDetails>,
    /// Every file of the package except `plugin.json`, sorted by path, with its SHA-256.
    pub files: Vec<FileEntry>,
}

fn bad(why: &'static str) -> PluginError {
    PluginError::Manifest(why)
}

/// `[a-z0-9-]{1,32}`, same as connection ids.
pub fn is_valid_plugin_id(id: &str) -> bool {
    is_valid_connection_id(id)
}

fn valid_text(text: &str, max_chars: usize) -> bool {
    let chars = text.chars().count();
    chars >= 1
        && chars <= max_chars
        && !text.trim().is_empty()
        && !text.chars().any(char::is_control)
}

fn valid_ext(ext: &str) -> bool {
    !ext.is_empty()
        && ext.len() <= 8
        && ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A file path inside the plugin folder: relative, clean components of `[A-Za-z0-9._-]`, no
/// `..`, bounded length and depth.
pub fn valid_package_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_FILE_PATH_BYTES || path.starts_with('/') {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() <= MAX_FILE_PATH_DEPTH
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        })
}

fn valid_sha256(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_mime(mime: &str) -> bool {
    let Some((kind, subtype)) = mime.split_once('/') else {
        return false;
    };
    let ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 40
            && part.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.' || b == b'+'
            })
    };
    ok(kind) && ok(subtype)
}

fn unique<T: std::hash::Hash + Eq>(items: impl Iterator<Item = T>) -> bool {
    let mut seen = HashSet::new();
    items.into_iter().all(|item| seen.insert(item))
}

impl Manifest {
    /// Parses and validates the bytes of a `plugin.json`.
    pub fn parse(bytes: &[u8]) -> PluginResult<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(bad("Die Datei ist zu groß"));
        }
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|_| bad("Aufbau oder Felder stimmen nicht"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> PluginResult<()> {
        if !is_valid_plugin_id(&self.id) {
            return Err(bad("Die Kennung ist ungültig"));
        }
        if !valid_text(&self.name, MAX_NAME_CHARS) {
            return Err(bad("Der Name ist ungültig oder zu lang"));
        }
        if self.version.is_empty()
            || self.version.len() > MAX_VERSION_BYTES
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+')
        {
            return Err(bad("Die Version ist ungültig"));
        }
        if self.api != API_VERSION {
            return Err(bad("Die Protokollversion wird nicht unterstützt"));
        }
        if !valid_package_path(&self.exec) {
            return Err(bad("Der Pfad der ausführbaren Datei ist ungültig"));
        }
        if self.kinds.is_empty() || self.kinds.len() > 4 || !unique(self.kinds.iter()) {
            return Err(bad("Die Arten sind leer oder doppelt"));
        }
        let has = |kind| self.kinds.contains(&kind);
        if has(Kind::Filesystem) != self.filesystem.is_some()
            || has(Kind::Packer) != self.packer.is_some()
            || has(Kind::Column) != self.columns.is_some()
            || has(Kind::Preview) != self.preview.is_some()
        {
            return Err(bad("Angaben und Arten passen nicht zusammen"));
        }
        if self.write && !has(Kind::Filesystem) {
            return Err(bad("Schreiben gibt es nur bei einem Dateisystem"));
        }
        if let Some(fs) = &self.filesystem {
            let ok = !fs.scheme.is_empty()
                && fs.scheme.len() <= 16
                && fs.scheme.as_bytes()[0].is_ascii_lowercase()
                && fs
                    .scheme
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !ok || RESERVED_SCHEMES.contains(&fs.scheme.as_str()) {
                return Err(bad("Der Dateisystem-Name ist ungültig oder vergeben"));
            }
        }
        if let Some(packer) = &self.packer {
            let ok = !packer.extensions.is_empty()
                && packer.extensions.len() <= MAX_LIST_ITEMS
                && packer.extensions.iter().all(|e| valid_ext(e))
                && unique(packer.extensions.iter());
            if !ok {
                return Err(bad("Die Archiv-Endungen sind ungültig"));
            }
            if packer
                .extensions
                .iter()
                .any(|e| RESERVED_EXTENSIONS.contains(&e.as_str()))
            {
                return Err(bad("Eine Archiv-Endung gehört einem eingebauten Format"));
            }
        }
        if let Some(columns) = &self.columns {
            let ok = !columns.is_empty()
                && columns.len() <= MAX_LIST_ITEMS
                && columns
                    .iter()
                    .all(|c| is_valid_plugin_id(&c.id) && valid_text(&c.title, 40))
                && unique(columns.iter().map(|c| c.id.as_str()));
            if !ok {
                return Err(bad("Die Spalten sind ungültig"));
            }
        }
        if let Some(preview) = &self.preview {
            let ok = (!preview.extensions.is_empty() || !preview.mime.is_empty())
                && preview.extensions.len() <= MAX_LIST_ITEMS
                && preview.mime.len() <= MAX_LIST_ITEMS
                && preview.extensions.iter().all(|e| valid_ext(e))
                && preview.mime.iter().all(|m| valid_mime(m))
                && unique(preview.extensions.iter())
                && unique(preview.mime.iter());
            if !ok {
                return Err(bad("Die Vorschau-Angaben sind ungültig"));
            }
        }
        if self.files.is_empty() || self.files.len() > MAX_FILES {
            return Err(bad("Die Dateiliste ist leer oder zu lang"));
        }
        if self.files.iter().any(|f| {
            !valid_package_path(&f.path) || f.path == MANIFEST_FILE || !valid_sha256(&f.sha256)
        }) {
            return Err(bad("Die Dateiliste enthält einen ungültigen Eintrag"));
        }
        if self
            .files
            .windows(2)
            .any(|pair| pair[0].path >= pair[1].path)
        {
            return Err(bad("Die Dateiliste ist nicht sortiert oder hat Doppelte"));
        }
        // a path must not be both a file and a folder of another entry
        for pair in self.files.windows(2) {
            if pair[1].path.starts_with(&format!("{}/", pair[0].path)) {
                return Err(bad(
                    "Die Dateiliste hat einen Eintrag als Datei und als Ordner",
                ));
            }
        }
        if !self.files.iter().any(|f| f.path == self.exec) {
            return Err(bad("Die ausführbare Datei fehlt in der Dateiliste"));
        }
        Ok(())
    }

    pub fn has_kind(&self, kind: Kind) -> bool {
        self.kinds.contains(&kind)
    }
}

/// Refuses a set of manifests that claim one id twice.
pub fn check_unique_ids(manifests: &[Manifest]) -> PluginResult<()> {
    if unique(manifests.iter().map(|m| m.id.as_str())) {
        Ok(())
    } else {
        Err(bad("Eine Plugin-Kennung kommt doppelt vor"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    pub(crate) fn good() -> Value {
        json!({
            "id": "example", "name": "Beispiel", "version": "1.0.0", "api": 1,
            "exec": "example-plugin", "kinds": ["filesystem", "column", "preview", "packer"],
            "write": false,
            "filesystem": {"scheme": "example"},
            "packer": {"extensions": ["xyz"]},
            "columns": [{"id": "len", "title": "Länge"}],
            "preview": {"extensions": ["txt"], "mime": ["text/plain"]},
            "files": [{"path": "example-plugin", "sha256": "a".repeat(64)}]
        })
    }

    fn parse(value: &Value) -> PluginResult<Manifest> {
        Manifest::parse(value.to_string().as_bytes())
    }

    fn with(edit: impl FnOnce(&mut Value)) -> PluginResult<Manifest> {
        let mut value = good();
        edit(&mut value);
        parse(&value)
    }

    #[test]
    fn a_good_manifest_parses() {
        let manifest = parse(&good()).unwrap();
        assert_eq!(manifest.id, "example");
        assert!(manifest.has_kind(Kind::Packer) && !manifest.write);
    }

    #[test]
    fn evil_ids_are_refused() {
        for id in ["", "A", "a b", "a_b", "../x", "a/b", "ä", &"a".repeat(33)] {
            assert!(with(|v| v["id"] = json!(id)).is_err(), "{id:?}");
        }
        assert!(with(|v| v["id"] = json!("a".repeat(32))).is_ok());
    }

    #[test]
    fn evil_paths_are_refused() {
        let evil = [
            "../x",
            "/bin/sh",
            "a/../b",
            "./x",
            "a//b",
            "a\\b",
            "a\nb",
            "",
            ".",
            "a b",
            "a/b/c/d/e",
            "ü",
        ];
        for path in evil {
            let exec_bad = with(|v| {
                v["exec"] = json!(path);
                v["files"][0]["path"] = json!(path);
            });
            assert!(exec_bad.is_err(), "exec {path:?}");
            assert!(
                with(|v| {
                    v["files"] = json!([
                        {"path": "example-plugin", "sha256": "a".repeat(64)},
                        {"path": path, "sha256": "b".repeat(64)}
                    ])
                })
                .is_err(),
                "file {path:?}"
            );
        }
        // the manifest cannot list itself
        assert!(with(|v| v["files"][0]["path"] = json!("plugin.json")).is_err());
        let long = "n".repeat(MAX_FILE_PATH_BYTES + 1);
        assert!(with(|v| {
            v["exec"] = json!(long);
            v["files"][0]["path"] = json!(long)
        })
        .is_err());
    }

    #[test]
    fn unknown_fields_and_wrong_versions_are_refused() {
        assert!(with(|v| v["rights"] = json!(["net"])).is_err());
        assert!(with(|v| v["net"] = json!("x:1")).is_err());
        assert!(with(|v| v["files"][0]["mode"] = json!(1)).is_err());
        assert!(with(|v| v["filesystem"]["extra"] = json!(1)).is_err());
        assert!(with(|v| v["api"] = json!(2)).is_err());
        assert!(with(|v| v["api"] = json!(0)).is_err());
        assert!(with(|v| v["api"] = json!("1")).is_err());
        assert!(with(|v| {
            v.as_object_mut().unwrap().remove("api");
        })
        .is_err());
        // duplicate keys are not silently merged
        let dup = br#"{"id":"a","id":"b"}"#;
        assert!(Manifest::parse(dup).is_err());
        assert!(Manifest::parse(b"not json").is_err());
        assert!(Manifest::parse(b"[]").is_err());
        assert!(Manifest::parse(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_err());
    }

    #[test]
    fn overlong_and_wrong_fields_are_refused() {
        assert!(with(|v| v["name"] = json!("n".repeat(MAX_NAME_CHARS + 1))).is_err());
        assert!(with(|v| v["name"] = json!("")).is_err());
        assert!(with(|v| v["name"] = json!("a\nb")).is_err());
        assert!(with(|v| v["name"] = json!("   ")).is_err());
        assert!(with(|v| v["version"] = json!("1".repeat(33))).is_err());
        assert!(with(|v| v["version"] = json!("1 2")).is_err());
        assert!(with(|v| v["kinds"] = json!([])).is_err());
        assert!(with(|v| v["kinds"] = json!(["filesystem", "filesystem"])).is_err());
        assert!(with(|v| v["kinds"] = json!(["exec"])).is_err());
        // details must match the kinds
        assert!(with(|v| v["kinds"] = json!(["filesystem"])).is_err());
        assert!(with(|v| {
            v.as_object_mut().unwrap().remove("preview");
        })
        .is_err());
        assert!(with(|v| v["write"] = json!(true)).is_ok());
        assert!(with(|v| {
            v["kinds"] = json!(["column"]);
            v["write"] = json!(true);
            for key in ["filesystem", "packer", "preview"] {
                v.as_object_mut().unwrap().remove(key);
            }
        })
        .is_err());
        for scheme in [
            "",
            "A",
            "1a",
            "a_b",
            "ftp",
            "sftp",
            "archive",
            "plugin",
            &"a".repeat(17),
        ] {
            assert!(
                with(|v| v["filesystem"]["scheme"] = json!(scheme)).is_err(),
                "{scheme}"
            );
        }
        for ext in [
            json!([]),
            json!(["ZIP"]),
            json!(["zip"]),
            json!(["7z"]),
            json!(["a.b"]),
            json!(["abcdefghi"]),
            json!(["x", "x"]),
        ] {
            assert!(
                with(|v| v["packer"]["extensions"] = ext.clone()).is_err(),
                "{ext}"
            );
        }
        assert!(with(|v| v["columns"] = json!([])).is_err());
        assert!(with(|v| v["columns"] = json!([{"id": "A", "title": "x"}])).is_err());
        assert!(with(|v| v["columns"] = json!([{"id": "a", "title": ""}])).is_err());
        assert!(with(
            |v| v["columns"] = json!([{"id": "a", "title": "x"}, {"id": "a", "title": "y"}])
        )
        .is_err());
        assert!(with(|v| v["preview"] = json!({})).is_err());
        assert!(with(|v| v["preview"] = json!({"mime": ["text"]})).is_err());
        assert!(with(|v| v["preview"] = json!({"mime": ["Text/Plain"]})).is_err());
        assert!(with(|v| v["preview"] = json!({"mime": ["text/html"]})).is_ok());
    }

    #[test]
    fn the_file_list_is_strict() {
        let h = "a".repeat(64);
        let file = |p: &str| json!({"path": p, "sha256": h});
        assert!(with(|v| v["files"] = json!([])).is_err());
        // unsorted and duplicate
        assert!(with(|v| v["files"] = json!([file("z"), file("example-plugin")])).is_err());
        assert!(
            with(|v| v["files"] = json!([file("example-plugin"), file("example-plugin")])).is_err()
        );
        // a path that is both file and folder
        assert!(
            with(|v| v["files"] = json!([file("a"), file("a/b"), file("example-plugin")])).is_err()
        );
        // the executable must be listed
        assert!(with(|v| v["files"] = json!([file("other")])).is_err());
        // hash format
        for sha in [
            "",
            "A".repeat(64).as_str(),
            &"g".repeat(64),
            &"a".repeat(63),
        ] {
            assert!(
                with(|v| v["files"][0]["sha256"] = json!(sha)).is_err(),
                "{sha}"
            );
        }
        let many: Vec<Value> = (0..=MAX_FILES).map(|i| file(&format!("f{i:03}"))).collect();
        assert!(with(|v| v["files"] = json!(many)).is_err());
    }

    #[test]
    fn duplicate_ids_across_manifests_are_refused() {
        let a = parse(&good()).unwrap();
        let mut b = a.clone();
        assert!(check_unique_ids(&[a.clone(), b.clone()]).is_err());
        b.id = "other".into();
        assert!(check_unique_ids(&[a, b]).is_ok());
    }
}
