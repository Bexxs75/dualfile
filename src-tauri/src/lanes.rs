//! Destination scheduling keys stay internal; the IPC receives only process-local opaque IDs.
use dualfile_core::vfs::{Scheme, VPath};
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;

pub fn opaque(key: &str) -> String {
    static HASHER: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    let mut hash = HASHER.get_or_init(Default::default).build_hasher();
    hash.write(key.as_bytes());
    format!("{:016x}", hash.finish())
}

pub fn destination_lane(dest: &VPath) -> Option<String> {
    match dest.scheme() {
        Scheme::Local => local_lane(dest.path()),
        Scheme::Ftp | Scheme::Sftp | Scheme::Smb | Scheme::Webdav | Scheme::Nfs => {
            dest.conn_id().map(|id| format!("conn:{id}"))
        }
        Scheme::Plugin => dest.conn_id().map(|id| format!("plugin:{id}")),
        Scheme::Archive => dest
            .outer()
            .map(|outer| format!("arch:{}", opaque(&dualfile_core::format_ipc_path(outer)))),
    }
}

fn local_lane(path: &std::path::Path) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        for ancestor in path.ancestors() {
            match std::fs::metadata(ancestor) {
                Ok(meta) => {
                    let dev = meta.dev();
                    // Linux dev_t encoding (gnu_dev_major / gnu_dev_minor).
                    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
                    let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
                    return Some(format!("dev:{major}:{minor}"));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_types_and_missing_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let base = VPath::local(root.path()).unwrap();
        let child = VPath::local(root.path().join("missing/subdir")).unwrap();
        assert_eq!(destination_lane(&base), destination_lane(&child));
        assert!(destination_lane(&base).unwrap().starts_with("dev:"));
        assert_eq!(
            destination_lane(&VPath::sftp("abc", "/x").unwrap()).as_deref(),
            Some("conn:abc")
        );
        assert_eq!(
            destination_lane(&VPath::ftp("abc", "/y").unwrap()).as_deref(),
            Some("conn:abc")
        );
        assert_eq!(
            destination_lane(&VPath::plugin("abc", "/x").unwrap()).as_deref(),
            Some("plugin:abc")
        );
        let archive = dualfile_core::parse_ipc_path("archive:/secret/a.zip!/x").unwrap();
        let lane = destination_lane(&archive).unwrap();
        assert!(lane.starts_with("arch:"));
        assert!(!lane.contains("secret"));
        assert_eq!(local_lane(std::path::Path::new("")), None);
        assert_eq!(opaque("x"), opaque("x"));
        assert_ne!(opaque("x"), opaque("y"));
    }
}
