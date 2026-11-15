//! Chooses the `Vfs` for a location by its scheme. Archive instances are cached per archive
//! location; the cached index is dropped as soon as size or modification time of the archive
//! file (and, for nested archives, of every outer archive) change.
//!
//! Usage for copying out of an archive: `ops::copy(&*router.vfs_for(&src)?, &[src], &*router
//! .vfs_for(&dst)?, &dst_dir, ...)`. Writing into an archive is refused by the archive `Vfs`
//! (and by `ops` through `Vfs::is_read_only`).
//!
//! Plugin file systems (`Scheme::Plugin`) are registered under their plugin id
//! (`register_plugin`) in a map of their own, so a plugin id never collides with a connection
//! id.
//!
//! Server connections (`Scheme::Ftp`/`Scheme::Sftp`) are registered by the connection manager
//! under their connection id (`register_remote`) and removed again on disconnect; a location
//! on an unknown or removed connection gives `Error::Disconnected` and never falls back to
//! another back end.

use crate::archive::{ArchiveLimits, ArchiveVfs, Budget, PackerBinding, Source};
use crate::error::{from_io, Error, Result};
use crate::vfs::{is_valid_connection_id, EntryKind, LocalVfs, Scheme, VPath, Vfs};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::SystemTime;

const MAX_CACHED_ARCHIVES: usize = 16;

type Signature = Vec<(u64, Option<SystemTime>)>;

struct Cached {
    vfs: Arc<ArchiveVfs>,
    signature: Signature,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<VPath, Cached>,
    order: VecDeque<VPath>,
}

pub struct VfsRouter {
    limits: ArchiveLimits,
    local: Arc<LocalVfs>,
    cache: Mutex<Cache>,
    remote: RwLock<HashMap<String, Arc<dyn Vfs>>>,
    plugins: RwLock<HashMap<String, Arc<dyn Vfs>>>,
    packers: RwLock<Vec<PackerBinding>>,
}

impl Default for VfsRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsRouter {
    pub fn new() -> Self {
        Self::with_limits(ArchiveLimits::default())
    }

    pub fn with_limits(limits: ArchiveLimits) -> Self {
        Self {
            limits,
            local: Arc::new(LocalVfs),
            cache: Mutex::new(Cache::default()),
            remote: RwLock::new(HashMap::new()),
            plugins: RwLock::new(HashMap::new()),
            packers: RwLock::new(Vec::new()),
        }
    }

    /// Makes `vfs` the back end of the connection it names (`Vfs::remote_endpoint`). The id
    /// must be a valid connection id and not in use; an existing connection is never replaced
    /// silently.
    pub fn register_remote(&self, id: &str, vfs: Arc<dyn Vfs>) -> Result<()> {
        if !is_valid_connection_id(id) {
            return Err(Error::InvalidInput("Ungültige Verbindungskennung"));
        }
        match vfs.remote_endpoint() {
            Some((scheme, endpoint_id)) if scheme.is_server() && endpoint_id == id => {}
            _ => {
                return Err(Error::InvalidInput(
                    "Das Backend gehört nicht zu dieser Verbindung",
                ))
            }
        }
        let mut remote = self.remote.write().unwrap();
        if remote.contains_key(id) {
            return Err(Error::InvalidInput(
                "Die Verbindungskennung ist schon vergeben",
            ));
        }
        remote.insert(id.to_owned(), vfs);
        Ok(())
    }

    /// Makes `vfs` the back end of the plugin `id` (`Vfs::remote_endpoint` must name
    /// `Scheme::Plugin` and the same id). An existing registration is never replaced silently.
    pub fn register_plugin(&self, id: &str, vfs: Arc<dyn Vfs>) -> Result<()> {
        if !is_valid_connection_id(id) {
            return Err(Error::InvalidInput("Ungültige Plugin-Kennung"));
        }
        match vfs.remote_endpoint() {
            Some((Scheme::Plugin, endpoint_id)) if endpoint_id == id => {}
            _ => {
                return Err(Error::InvalidInput(
                    "Das Backend gehört nicht zu diesem Plugin",
                ))
            }
        }
        let mut plugins = self.plugins.write().unwrap();
        if plugins.contains_key(id) {
            return Err(Error::InvalidInput("Das Plugin ist schon angemeldet"));
        }
        plugins.insert(id.to_owned(), vfs);
        Ok(())
    }

    /// Makes the plugin's packer serve its archive extensions. An extension that another
    /// packer already serves is refused (the first registration stays), and so is a second
    /// registration of the same plugin. Cached archives are dropped (their format may change).
    pub fn register_packer(&self, binding: PackerBinding) -> Result<()> {
        let mut packers = self.packers.write().unwrap();
        if packers.iter().any(|other| other.id == binding.id) {
            return Err(Error::InvalidInput("Das Plugin ist schon angemeldet"));
        }
        if packers.iter().any(|other| {
            other
                .extensions
                .iter()
                .any(|ext| binding.extensions.contains(ext))
        }) {
            return Err(Error::InvalidInput(
                "Die Archiv-Endung gehört schon einem anderen Plugin",
            ));
        }
        packers.push(binding);
        drop(packers);
        self.clear_cache();
        Ok(())
    }

    /// Removes the packer of plugin `id`; archives of its formats can no longer be opened.
    pub fn unregister_packer(&self, id: &str) {
        self.packers.write().unwrap().retain(|other| other.id != id);
        self.clear_cache();
    }

    fn clear_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.entries.clear();
        cache.order.clear();
    }

    /// Removes a plugin; later accesses to its locations give `Error::Disconnected`.
    pub fn unregister_plugin(&self, id: &str) -> Option<Arc<dyn Vfs>> {
        self.plugins.write().unwrap().remove(id)
    }

    pub fn has_plugin(&self, id: &str) -> bool {
        self.plugins.read().unwrap().contains_key(id)
    }

    /// Ids of all registered plugins, sorted.
    pub fn plugin_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.plugins.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Removes a connection; later accesses to its locations give `Error::Disconnected`.
    /// Returns the removed back end.
    pub fn unregister(&self, id: &str) -> Option<Arc<dyn Vfs>> {
        self.remote.write().unwrap().remove(id)
    }

    pub fn has_remote(&self, id: &str) -> bool {
        self.remote.read().unwrap().contains_key(id)
    }

    /// Ids of all registered connections, sorted.
    pub fn remote_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.remote.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// The `Vfs` that serves `path`: the local file system, the (cached) archive the path
    /// lies in, or the registered server connection. Opening an archive builds its index within the index limits.
    pub fn vfs_for(&self, path: &VPath) -> Result<Arc<dyn Vfs>> {
        match path.scheme() {
            Scheme::Local => Ok(self.local.clone()),
            Scheme::Archive => {
                let file = path.outer().expect("archive paths have an outer location");
                let budget = ArchiveVfs::new_index_budget(&self.limits);
                Ok(self.archive_for(file, &budget)?)
            }
            Scheme::Plugin => {
                let id = path.conn_id().ok_or(Error::Disconnected)?;
                let vfs = self
                    .plugins
                    .read()
                    .unwrap()
                    .get(id)
                    .cloned()
                    .ok_or(Error::Disconnected)?;
                match vfs.remote_endpoint() {
                    Some((Scheme::Plugin, endpoint_id)) if endpoint_id == id => Ok(vfs),
                    _ => Err(Error::Disconnected),
                }
            }
            Scheme::Ftp | Scheme::Sftp | Scheme::Smb | Scheme::Nfs | Scheme::Webdav => {
                let id = path.conn_id().ok_or(Error::Disconnected)?;
                let vfs = self
                    .remote
                    .read()
                    .unwrap()
                    .get(id)
                    .cloned()
                    .ok_or(Error::Disconnected)?;
                // The registered back end must serve exactly this scheme and id.
                match vfs.remote_endpoint() {
                    Some((scheme, endpoint_id)) if scheme == path.scheme() && endpoint_id == id => {
                        Ok(vfs)
                    }
                    _ => Err(Error::Disconnected),
                }
            }
        }
    }

    /// Drops the cached archive at `file` (and everything nested in it).
    pub fn forget(&self, file: &VPath) {
        let mut cache = self.cache.lock().unwrap();
        let doomed: Vec<VPath> = cache
            .entries
            .keys()
            .filter(|key| is_inside(key, file))
            .cloned()
            .collect();
        for key in doomed {
            cache.entries.remove(&key);
            cache.order.retain(|other| other != &key);
        }
    }

    fn signature(&self, file: &VPath, budget: &Budget) -> Result<Signature> {
        match file.outer() {
            None => {
                let meta = std::fs::metadata(file.path())
                    .map_err(|e| from_io("Archiv öffnen", file.path(), e))?;
                if !meta.is_file() {
                    return Err(Error::InvalidPath("Der Pfad ist keine Archivdatei".into()));
                }
                Ok(vec![(meta.len(), meta.modified().ok())])
            }
            Some(outer) => {
                let (parent, mut signature) = self.archive_with_signature(outer, budget)?;
                let entry = parent.stat(file)?;
                if entry.kind != EntryKind::File {
                    return Err(Error::InvalidPath("Der Pfad ist keine Archivdatei".into()));
                }
                signature.push((entry.size, entry.modified));
                Ok(signature)
            }
        }
    }

    /// The archive vfs for the archive file `file` plus the signature it was built for.
    /// `budget` is shared over all nesting levels opened in one call.
    pub fn archive_for(&self, file: &VPath, budget: &Budget) -> Result<Arc<ArchiveVfs>> {
        self.archive_with_signature(file, budget)
            .map(|(vfs, _)| vfs)
    }

    fn archive_with_signature(
        &self,
        file: &VPath,
        budget: &Budget,
    ) -> Result<(Arc<ArchiveVfs>, Signature)> {
        let signature = self.signature(file, budget)?;
        if let Some(cached) = self.cache.lock().unwrap().entries.get(file) {
            if cached.signature == signature {
                return Ok((cached.vfs.clone(), signature));
            }
        }
        let source = match file.outer() {
            None => Source::File(file.path().to_path_buf()),
            Some(outer) => {
                let parent = self.archive_for(outer, budget)?;
                Source::Memory(Arc::new(self.read_nested(&parent, file, budget)?))
            }
        };
        let packers = self.packers.read().unwrap().clone();
        let vfs = Arc::new(ArchiveVfs::open_with(
            file.clone(),
            source,
            self.limits.clone(),
            budget.clone(),
            &packers,
        )?);
        let mut cache = self.cache.lock().unwrap();
        if cache.entries.remove(file).is_some() {
            cache.order.retain(|other| other != file);
        }
        while cache.entries.len() >= MAX_CACHED_ARCHIVES {
            match cache.order.pop_front() {
                Some(oldest) => {
                    cache.entries.remove(&oldest);
                }
                None => break,
            }
        }
        cache.order.push_back(file.clone());
        cache.entries.insert(
            file.clone(),
            Cached {
                vfs: vfs.clone(),
                signature: signature.clone(),
            },
        );
        Ok((vfs, signature))
    }

    /// Reads an archive stored inside another archive into memory (bounded).
    fn read_nested(&self, parent: &ArchiveVfs, file: &VPath, budget: &Budget) -> Result<Vec<u8>> {
        let limit = self.limits.nested_max_bytes;
        let mut data = Vec::new();
        parent
            .open_read(file)?
            .take(limit + 1)
            .read_to_end(&mut data)
            .map_err(|e| crate::archive::map_io(e, file.path()))?;
        if data.len() as u64 > limit {
            return Err(Error::Archive("Verschachteltes Archiv zu groß"));
        }
        budget
            .charge(data.len() as u64)
            .map_err(|e| crate::archive::map_io(e, file.path()))?;
        Ok(data)
    }
}

fn is_inside(candidate: &VPath, file: &VPath) -> bool {
    let mut current = Some(candidate);
    while let Some(path) = current {
        if path == file {
            return true;
        }
        current = path.outer();
    }
    false
}

/// The `Vfs` for `path` from a process-wide router (default limits).
pub fn open_path(path: &VPath) -> Result<Arc<dyn Vfs>> {
    static ROUTER: OnceLock<VfsRouter> = OnceLock::new();
    ROUTER.get_or_init(VfsRouter::new).vfs_for(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{self, JobHandle};
    use std::fs;
    use std::io::{Cursor, Write};

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn read(vfs: &dyn Vfs, path: &VPath) -> Vec<u8> {
        let mut data = Vec::new();
        vfs.open_read(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    #[test]
    fn dispatches_by_scheme() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        fs::write(&zip, zip_with(&[("f.txt", b"content")])).unwrap();
        let router = VfsRouter::new();
        let local = VPath::local(tmp.path()).unwrap();
        let vfs = router.vfs_for(&local).unwrap();
        assert!(vfs.is_local() && !vfs.is_read_only());
        let inside = VPath::archive(VPath::local(&zip).unwrap(), "/f.txt").unwrap();
        let vfs = router.vfs_for(&inside).unwrap();
        assert!(!vfs.is_local() && vfs.is_read_only());
        assert_eq!(read(&*vfs, &inside), b"content");
        // a file that is no archive gives a clear error, a missing one a not-found error
        let plain = tmp.path().join("plain.zip");
        fs::write(&plain, b"just text").unwrap();
        let bad = VPath::archive(VPath::local(&plain).unwrap(), "/x").unwrap();
        assert!(router.vfs_for(&bad).is_err());
        let missing =
            VPath::archive(VPath::local(tmp.path().join("gone.zip")).unwrap(), "/x").unwrap();
        assert!(matches!(router.vfs_for(&missing), Err(Error::NotFound(_))));
        // the process-wide helper uses the same dispatch
        assert!(open_path(&inside).unwrap().is_read_only());
        assert!(open_path(&local).unwrap().is_local());
    }

    #[test]
    fn archive_instances_are_cached_and_invalidated_by_size_or_time() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        fs::write(&zip, zip_with(&[("f.txt", b"first")])).unwrap();
        let router = VfsRouter::new();
        let file = VPath::local(&zip).unwrap();
        let path = VPath::archive(file.clone(), "/f.txt").unwrap();
        let budget = || ArchiveVfs::new_index_budget(&ArchiveLimits::default());
        let one = router.archive_for(&file, &budget()).unwrap();
        let again = router.archive_for(&file, &budget()).unwrap();
        assert!(
            Arc::ptr_eq(&one, &again),
            "unchanged archive: same instance"
        );
        assert_eq!(read(&*router.vfs_for(&path).unwrap(), &path), b"first");
        // different size
        fs::write(
            &zip,
            zip_with(&[("f.txt", b"second version"), ("g.txt", b"new")]),
        )
        .unwrap();
        let changed = router.archive_for(&file, &budget()).unwrap();
        assert!(!Arc::ptr_eq(&one, &changed));
        assert_eq!(
            read(&*router.vfs_for(&path).unwrap(), &path),
            b"second version"
        );
        // same size, different modification time
        let same_size = zip_with(&[("f.txt", b"SECOND VERSION"), ("g.txt", b"new")]);
        assert_eq!(same_size.len(), fs::metadata(&zip).unwrap().len() as usize);
        fs::write(&zip, &same_size).unwrap();
        let handle = fs::File::options().write(true).open(&zip).unwrap();
        handle
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(120))
            .unwrap();
        drop(handle);
        let third = router.archive_for(&file, &budget()).unwrap();
        assert!(!Arc::ptr_eq(&changed, &third));
        assert_eq!(
            read(&*router.vfs_for(&path).unwrap(), &path),
            b"SECOND VERSION"
        );
        // forget drops the entry
        router.forget(&file);
        let fourth = router.archive_for(&file, &budget()).unwrap();
        assert!(!Arc::ptr_eq(&third, &fourth));
    }

    #[test]
    fn nested_archives_open_through_the_router_and_invalidate_with_their_outer() {
        let tmp = tempfile::tempdir().unwrap();
        let inner = zip_with(&[("deep.txt", b"deep")]);
        let outer = tmp.path().join("outer.zip");
        fs::write(
            &outer,
            zip_with(&[("in/inner.zip", &inner), ("top.txt", b"top")]),
        )
        .unwrap();
        let router = VfsRouter::new();
        let outer_path = VPath::local(&outer).unwrap();
        let inner_file = VPath::archive(outer_path.clone(), "/in/inner.zip").unwrap();
        let target = VPath::archive(inner_file.clone(), "/deep.txt").unwrap();
        let vfs = router.vfs_for(&target).unwrap();
        assert_eq!(read(&*vfs, &target), b"deep");
        assert_eq!(
            crate::vfs::format_ipc_path(&target),
            format!(
                "archive:archive:{}%21/in/inner.zip!/deep.txt",
                outer.display()
            )
        );
        let listing = vfs
            .list(&VPath::archive(inner_file.clone(), "/").unwrap())
            .unwrap();
        assert_eq!(listing.len(), 1);
        // the outer archive changes: the nested index is rebuilt from the new content
        let inner2 = zip_with(&[("deep.txt", b"DEEP")]);
        fs::write(
            &outer,
            zip_with(&[("in/inner.zip", &inner2), ("top.txt", b"top")]),
        )
        .unwrap();
        let vfs = router.vfs_for(&target).unwrap();
        assert_eq!(read(&*vfs, &target), b"DEEP");
    }

    #[test]
    fn copy_from_archive_to_local_goes_through_ops_with_two_different_vfs() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        fs::write(&zip, zip_with(&[("d/f.txt", b"payload"), ("g.txt", b"g")])).unwrap();
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        let router = VfsRouter::new();
        let file = VPath::local(&zip).unwrap();
        let sources = [
            VPath::archive(file.clone(), "/d").unwrap(),
            VPath::archive(file.clone(), "/g.txt").unwrap(),
        ];
        let dst = VPath::local(&out).unwrap();
        let src_vfs = router.vfs_for(&sources[0]).unwrap();
        let dst_vfs = router.vfs_for(&dst).unwrap();
        let report = ops::copy(
            &*src_vfs,
            &sources,
            &*dst_vfs,
            &dst,
            None,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(fs::read(out.join("d/f.txt")).unwrap(), b"payload");
        assert_eq!(fs::read(out.join("g.txt")).unwrap(), b"g");
        // the other direction (into the archive) is refused by the core
        let into = VPath::archive(file, "/").unwrap();
        let local_src = [VPath::local(out.join("g.txt")).unwrap()];
        let report = ops::copy(
            &*dst_vfs,
            &local_src,
            &*src_vfs,
            &into,
            None,
            &JobHandle::silent(),
        );
        assert_eq!(report.failed.len(), 1);
    }

    /// A packer that serves `name.xyz` archives made of lines `path<TAB>content`.
    struct LinePacker {
        max: u64,
        lie_about_size: bool,
    }

    impl crate::archive::ExternalPacker for LinePacker {
        fn max_input_bytes(&self) -> u64 {
            self.max
        }
        fn list(&self, _name: &str, data: &[u8]) -> Result<Vec<crate::archive::ExternalEntry>> {
            Ok(String::from_utf8_lossy(data)
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(path, content)| crate::archive::ExternalEntry {
                    path: path.to_owned(),
                    is_dir: path.ends_with('/'),
                    size: content.len() as u64 + u64::from(self.lie_about_size),
                    problem: None,
                })
                .collect())
        }
        fn read(&self, _name: &str, data: &[u8], member: &str) -> Result<Vec<u8>> {
            String::from_utf8_lossy(data)
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .find(|(path, _)| *path == member)
                .map(|(_, content)| content.as_bytes().to_vec())
                .ok_or(Error::Archive("fehlt"))
        }
    }

    fn binding(id: &str, exts: &[&str], max: u64, lie: bool) -> PackerBinding {
        PackerBinding::new(
            id,
            exts.iter().map(|e| e.to_string()).collect(),
            Arc::new(LinePacker {
                max,
                lie_about_size: lie,
            }),
        )
        .unwrap()
    }

    #[test]
    fn a_plugin_packer_serves_its_extension_with_the_usual_safety_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.xyz");
        fs::write(&file, "docs/a.txt\tAAA\n../evil\tX\n/abs\tX\nb.txt\tB\n").unwrap();
        let router = VfsRouter::new();
        let loc = VPath::local(&file).unwrap();
        let root = VPath::archive(loc.clone(), "/").unwrap();
        // no packer registered: not an archive
        assert!(router.vfs_for(&root).is_err());
        router
            .register_packer(binding("lines", &["xyz"], 1 << 20, false))
            .unwrap();
        let vfs = router.vfs_for(&root).unwrap();
        assert!(vfs.is_read_only());
        let names: Vec<(String, bool)> = vfs
            .list(&root)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.error.is_some()))
            .collect();
        assert!(names.contains(&("docs".into(), false)));
        assert!(names.contains(&("b.txt".into(), false)));
        assert_eq!(
            names
                .iter()
                .filter(|(n, bad)| *bad && n.starts_with("[unsicher"))
                .count(),
            2
        );
        let a = VPath::archive(loc.clone(), "/docs/a.txt").unwrap();
        assert_eq!(read(&*vfs, &a), b"AAA");
        // unsafe entries can never be read
        let unsafe_name = names
            .iter()
            .find(|(n, bad)| *bad && n.starts_with("[unsicher"))
            .unwrap();
        let bad = VPath::archive(loc.clone(), &format!("/{}", unsafe_name.0)).unwrap();
        assert!(vfs.open_read(&bad).is_err());
        // unregistering drops the format again
        router.unregister_packer("lines");
        assert!(router.vfs_for(&root).is_err());
    }

    #[test]
    fn plugin_packers_never_replace_built_in_formats_and_respect_their_limit() {
        let tmp = tempfile::tempdir().unwrap();
        // a real ZIP named like the plugin's extension is read by the built-in code
        let zip = tmp.path().join("real.xyz");
        fs::write(&zip, zip_with(&[("f.txt", b"zip")])).unwrap();
        let router = VfsRouter::new();
        router
            .register_packer(binding("lines", &["xyz"], 64, false))
            .unwrap();
        let loc = VPath::local(&zip).unwrap();
        let f = VPath::archive(loc, "/f.txt").unwrap();
        assert_eq!(read(&*router.vfs_for(&f).unwrap(), &f), b"zip");
        // a plugin archive above the plugin's limit gets a clear message
        let big = tmp.path().join("big.xyz");
        fs::write(&big, "x\t".to_string() + &"y".repeat(200)).unwrap();
        let root = VPath::archive(VPath::local(&big).unwrap(), "/").unwrap();
        match router.vfs_for(&root) {
            Err(Error::Archive(message)) => assert!(message.contains("zu groß"), "{message}"),
            other => panic!("expected an archive error, got {:?}", other.err()),
        }
    }

    #[test]
    fn a_size_the_plugin_announces_is_checked_against_the_bytes_that_arrive() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.xyz");
        fs::write(&file, "f.txt\tcontent\n").unwrap();
        let router = VfsRouter::new();
        router
            .register_packer(binding("lines", &["xyz"], 1 << 20, true))
            .unwrap();
        let loc = VPath::local(&file).unwrap();
        let f = VPath::archive(loc, "/f.txt").unwrap();
        let vfs = router.vfs_for(&f).unwrap();
        let mut data = Vec::new();
        let result = vfs.open_read(&f).and_then(|mut r| {
            r.read_to_end(&mut data)
                .map_err(|e| crate::archive::map_io(e, f.path()))
        });
        assert!(result.is_err(), "a wrong announced size must fail the read");
    }

    #[test]
    fn packer_registration_refuses_clashes_and_bad_extensions() {
        let router = VfsRouter::new();
        router
            .register_packer(binding("one", &["xyz"], 10, false))
            .unwrap();
        assert!(router
            .register_packer(binding("two", &["abc", "xyz"], 10, false))
            .is_err());
        assert!(router
            .register_packer(binding("one", &["abc"], 10, false))
            .is_err());
        let packer: Arc<dyn crate::archive::ExternalPacker> = Arc::new(LinePacker {
            max: 1,
            lie_about_size: false,
        });
        for bad in ["", "A", "x.y", "a/b", "waytoolongextension1"] {
            assert!(
                PackerBinding::new("p", vec![bad.into()], packer.clone()).is_err(),
                "{bad}"
            );
        }
        assert!(PackerBinding::new("P", vec!["ok".into()], packer.clone()).is_err());
        assert!(PackerBinding::new("p", vec![], packer).is_err());
    }
}
