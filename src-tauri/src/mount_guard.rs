//! Atomic admission of jobs and unmounts. Paths and mount identities never enter IPC.
use crate::convert::UiError;
use dualfile_core::{
    places_pool,
    vfs::{Scheme, VPath},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

#[derive(Default)]
pub struct Registry(Mutex<Claims>);
#[derive(Default)]
struct Claims {
    next: u64,
    generation: u64,
    jobs: HashMap<u64, Vec<PathBuf>>,
    unmounts: HashMap<u64, PathBuf>,
    changed: HashMap<PathBuf, u64>,
}
pub struct Lease {
    registry: Arc<Registry>,
    id: u64,
    unmount: bool,
    generation: u64,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut claims = self.registry.0.lock().unwrap();
        if self.unmount {
            if let Some(point) = claims.unmounts.remove(&self.id) {
                claims.generation += 1;
                let generation = claims.generation;
                claims.changed.insert(point, generation);
            }
        } else {
            claims.jobs.remove(&self.id);
        }
    }
}
impl Lease {
    fn resolve(self, paths: Vec<PathBuf>) -> Result<Self, UiError> {
        {
            let mut c = self.registry.0.lock().unwrap();
            // An unmount racing alias resolution invalidates admission, even if its
            // helper has already exited. Never queue a write onto the underlying directory.
            if c.changed.iter().any(|(m, generation)| {
                *generation > self.generation && paths.iter().any(|p| overlaps(p, m))
            }) || c
                .unmounts
                .values()
                .any(|m| paths.iter().any(|p| overlaps(p, m)))
            {
                return Err(busy());
            }
            c.jobs.insert(self.id, paths);
        }
        Ok(self)
    }
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
pub fn busy() -> UiError {
    UiError::new("busy", "busy")
}
impl Registry {
    pub fn job(self: &Arc<Self>, paths: Vec<PathBuf>) -> Result<Lease, UiError> {
        let mut c = self.0.lock().unwrap();
        if c.unmounts
            .values()
            .any(|m| paths.iter().any(|p| overlaps(p, m)))
        {
            return Err(busy());
        }
        c.next += 1;
        let id = c.next;
        c.jobs.insert(id, paths);
        Ok(Lease {
            registry: self.clone(),
            id,
            unmount: false,
            generation: c.generation,
        })
    }
    pub fn unmount(self: &Arc<Self>, point: PathBuf) -> Result<Lease, UiError> {
        let mut c = self.0.lock().unwrap();
        if c.jobs.values().flatten().any(|p| overlaps(p, &point))
            || c.unmounts.values().any(|p| overlaps(p, &point))
        {
            return Err(busy());
        }
        c.next += 1;
        let id = c.next;
        c.unmounts.insert(id, point.clone());
        c.generation += 1;
        let generation = c.generation;
        c.changed.insert(point, generation);
        Ok(Lease {
            registry: self.clone(),
            id,
            unmount: true,
            generation: c.generation,
        })
    }
}
pub fn registry() -> &'static Arc<Registry> {
    static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
fn local_path(path: &VPath) -> Option<PathBuf> {
    if let Some(outer) = path.outer() {
        return local_path(outer);
    }
    (path.scheme() == Scheme::Local).then(|| path.path().to_owned())
}
pub fn job(paths: &[String]) -> Result<Lease, UiError> {
    let paths: Vec<_> = paths
        .iter()
        .filter_map(|p| {
            dualfile_core::parse_ipc_path(p)
                .ok()
                .as_ref()
                .and_then(local_path)
        })
        .collect();
    let lease = registry().job(paths.clone())?;
    if paths.is_empty() {
        return Ok(lease);
    }
    let points = mount_points()?;
    let mut resolved = paths.clone();
    resolved.extend(resolve_batch(
        &paths,
        &points,
        admission_pool(),
        |parent| {
            resolve_with(
                parent.to_owned(),
                &points,
                admission_pool(),
                Duration::from_millis(750),
            )
        },
    )?);
    lease.resolve(resolved)
}

// Admission never shares workers with discovery, helpers, or mount-state checks.
fn admission_pool() -> &'static places_pool::Pool {
    static POOL: OnceLock<places_pool::Pool> = OnceLock::new();
    POOL.get_or_init(|| places_pool::Pool::new(4))
}
fn path_key(path: &Path, points: &[PathBuf]) -> String {
    points
        .iter()
        .filter(|p| p.as_path() != Path::new("/") && path.starts_with(p))
        .max_by_key(|p| p.components().count())
        .cloned()
        .unwrap_or_else(|| path.components().take(2).collect())
        .to_string_lossy()
        .into_owned()
}
/// Read one size- and time-bounded kernel snapshot per admission.
fn mount_points() -> Result<Vec<PathBuf>, UiError> {
    const CAP: u64 = 4 * 1024 * 1024;
    let table = crate::places::bounded_read("/proc/self/mountinfo".into(), CAP)
        .filter(|s| !s.is_empty() && s.len() < CAP as usize)
        .ok_or_else(busy)?;
    Ok(table
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4))
        .map(|p| dualfile_core::drives::unescape(p).into())
        .collect())
}
/// Resolve each distinct parent once, then inspect its leaves in one keyed task.
/// Leaves still need lstat: a selected symlink may itself point into another mount.
fn resolve_batch(
    paths: &[PathBuf],
    points: &[PathBuf],
    pool: &places_pool::Pool,
    mut resolve_parent: impl FnMut(&Path) -> Result<PathBuf, UiError>,
) -> Result<Vec<PathBuf>, UiError> {
    let mut parents: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        parents
            .entry(path.parent().unwrap_or(path).to_owned())
            .or_default()
            .push(path.clone());
    }
    let mut result = Vec::with_capacity(paths.len());
    for (parent, children) in parents {
        let real = resolve_parent(&parent)?;
        let leaves: Vec<_> = children
            .iter()
            .map(|child| {
                child
                    .file_name()
                    .map_or_else(|| real.clone(), |name| real.join(name))
            })
            .collect();
        let probes: Vec<_> = leaves
            .iter()
            .filter(|p| !points.contains(p))
            .cloned()
            .collect();
        let aliases = if probes.is_empty() {
            Vec::new()
        } else {
            leaf_aliases(
                pool,
                path_key(&real, points),
                probes,
                Duration::from_millis(750),
                |path| std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()),
            )?
        };
        result.extend(leaves);
        for alias in aliases {
            result.push(resolve_with(
                alias,
                points,
                pool,
                Duration::from_millis(750),
            )?);
        }
    }
    Ok(result)
}
/// A large selection can take arbitrarily long while each leaf makes progress.
/// A stalled syscall still occupies at most one worker for this mount key.
fn leaf_aliases(
    pool: &places_pool::Pool,
    key: String,
    paths: Vec<PathBuf>,
    idle: Duration,
    is_symlink: impl Fn(&Path) -> std::io::Result<bool> + Send + 'static,
) -> Result<Vec<PathBuf>, UiError> {
    pool.run_keyed_watchdog(
        key,
        idle,
        || false,
        move |progress| {
            let mut aliases = Vec::new();
            for path in paths {
                match is_symlink(&path) {
                    Ok(true) => aliases.push(path),
                    Ok(false) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(busy()),
                }
                progress.progress();
            }
            Ok(aliases)
        },
    )
    .ok_or_else(busy)?
}
/// Walk without following symlinks implicitly. Each probe is keyed by its actual
/// lexical mount, including after a symlink redirects into another filesystem.
/// Kernel mount points need no stat, so even a dead mount root remains resolvable.
fn resolve_with(
    mut path: PathBuf,
    points: &[PathBuf],
    pool: &places_pool::Pool,
    budget: Duration,
) -> Result<PathBuf, UiError> {
    use std::path::Component;
    let deadline = std::time::Instant::now() + budget;
    for _ in 0..40 {
        let mut real = PathBuf::from("/");
        let mut components = path.components();
        let mut redirected = false;
        while let Some(component) = components.next() {
            match component {
                Component::RootDir => continue,
                Component::CurDir => continue,
                Component::ParentDir => {
                    real.pop();
                    continue;
                }
                Component::Normal(name) => real.push(name),
                _ => return Err(busy()),
            }
            if points.contains(&real) {
                continue;
            }
            let probe = real.clone();
            let result = pool
                .run_keyed_wait(
                    path_key(&probe, points),
                    deadline.saturating_duration_since(std::time::Instant::now()),
                    move || {
                        std::fs::symlink_metadata(&probe).and_then(|m| {
                            if m.file_type().is_symlink() {
                                std::fs::read_link(&probe).map(Some)
                            } else {
                                Ok(None)
                            }
                        })
                    },
                )
                .ok_or_else(busy)?;
            match result {
                Ok(Some(target)) => {
                    real.pop();
                    path = real.join(target).join(components.as_path());
                    redirected = true;
                    break;
                }
                Ok(None) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(real.join(components.as_path()))
                }
                Err(_) => return Err(busy()),
            }
        }
        if !redirected {
            return Ok(real);
        }
    }
    Err(busy())
}

pub fn affected_paths(point: &Path, paths: &[String]) -> Result<Vec<usize>, UiError> {
    let points = mount_points()?;
    let mut affected = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if let Some(path) = dualfile_core::parse_ipc_path(path)
            .ok()
            .as_ref()
            .and_then(local_path)
        {
            if path.starts_with(point)
                || resolve_with(path, &points, admission_pool(), Duration::from_millis(750))?
                    .starts_with(point)
            {
                affected.push(i);
            }
        }
    }
    Ok(affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_leaf_admissions_on_the_same_healthy_mount_both_succeed() {
        let pool = places_pool::Pool::new(2);
        let (started, entered) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                leaf_aliases(
                    &pool,
                    "/home".into(),
                    vec!["/home/first".into()],
                    Duration::from_secs(2),
                    move |_| {
                        started.send(()).unwrap();
                        blocked.recv().unwrap();
                        Ok(false)
                    },
                )
            });
            entered.recv_timeout(Duration::from_secs(1)).unwrap();
            let (called, waiting) = std::sync::mpsc::channel();
            let (done, result) = std::sync::mpsc::channel();
            let pool = &pool;
            scope.spawn(move || {
                called.send(()).unwrap();
                done.send(leaf_aliases(
                    pool,
                    "/home".into(),
                    vec!["/home/second".into()],
                    Duration::from_secs(2),
                    |_| Ok(false),
                ))
                .unwrap();
            });
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
            let early = result.recv_timeout(Duration::from_millis(30));
            release.send(()).unwrap();
            assert!(matches!(
                early,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            assert!(first.join().unwrap().unwrap().is_empty());
            assert!(result
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .is_empty());
        });
    }
    #[test]
    fn slow_leaves_keep_admission_alive_beyond_the_old_total_budget() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let checked = Arc::new(AtomicUsize::new(0));
        let count = checked.clone();
        let paths = (0..100)
            .map(|i| PathBuf::from(format!("/slow/{i}")))
            .collect();
        let start = std::time::Instant::now();
        let aliases = leaf_aliases(
            &places_pool::Pool::new(1),
            "/slow".into(),
            paths,
            Duration::from_millis(750),
            move |path| {
                std::thread::sleep(Duration::from_millis(10));
                count.fetch_add(1, Ordering::SeqCst);
                Ok(path == Path::new("/slow/99"))
            },
        )
        .unwrap();
        assert!(start.elapsed() > Duration::from_millis(750));
        assert_eq!(checked.load(Ordering::SeqCst), 100);
        assert_eq!(aliases, vec![PathBuf::from("/slow/99")]);
    }
    #[test]
    fn a_large_selection_resolves_its_shared_parent_once_and_keeps_leaf_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = temp.path().join("selected-link");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut paths: Vec<_> = (0..10_000)
            .map(|i| temp.path().join(format!("file-{i}")))
            .collect();
        paths.push(alias);
        let mut count = 0;
        let resolved = resolve_batch(&paths, &[], &places_pool::Pool::new(2), |parent| {
            count += 1;
            Ok(parent.to_owned())
        })
        .unwrap();
        assert_eq!(count, 1);
        assert!(resolved.contains(&real));
        assert!(resolved.contains(&temp.path().join("file-9999")));
    }
    #[test]
    fn stuck_empty_trash_keeps_only_its_actual_trash_mount_busy() {
        let registry = Arc::new(Registry::default());
        let lease = registry
            .job(vec!["/home/test/.local/share/Trash".into()])
            .unwrap();
        let pool = places_pool::Pool::new(1);
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        assert!(pool
            .run_keyed("trash".into(), Duration::from_millis(20), move || {
                let _lease = lease;
                blocked.recv().unwrap();
            })
            .is_none());
        assert!(registry.unmount("/home".into()).is_err());
        assert!(registry.unmount("/network".into()).is_ok());
        assert!(registry.unmount("/media/usb".into()).is_ok());
        release.send(()).unwrap();
    }
    #[test]
    fn queued_running_paused_sources_destinations_and_nested_mounts() {
        let registry = Arc::new(Registry::default());
        let job = registry
            .job(vec!["/source/file".into(), "/target/sub".into()])
            .unwrap();
        for point in ["/source", "/target", "/target/sub/nested"] {
            assert!(registry.unmount(point.into()).is_err());
        }
        assert!(registry.unmount("/source-other".into()).is_ok());
        drop(job);
        let unmount = registry.unmount("/source".into()).unwrap();
        assert!(registry.job(vec!["/source/new".into()]).is_err());
        assert!(registry.job(vec!["/".into()]).is_err());
        drop(unmount);
        assert!(registry.job(vec!["/source/new".into()]).is_ok());
    }
    #[test]
    fn unrelated_unmount_does_not_invalidate_admission() {
        let registry = Arc::new(Registry::default());
        let pending = registry.job(vec!["/local".into()]).unwrap();
        drop(registry.unmount("/network".into()).unwrap());
        assert!(pending.resolve(vec!["/local/file".into()]).is_ok());
    }
    #[test]
    fn blocked_path_does_not_starve_local_admission() {
        let pool = places_pool::Pool::new(2);
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        assert!(pool
            .run_keyed("/dead".into(), Duration::from_millis(20), move || blocked
                .recv()
                .unwrap())
            .is_none());
        for _ in 0..10 {
            assert!(resolve_with(
                "/dead/file".into(),
                &["/dead".into()],
                &pool,
                Duration::from_millis(50)
            )
            .is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        assert!(resolve_with(temp.path().join("new"), &[], &pool, Duration::from_secs(1)).is_ok());
        release.send(()).unwrap();
    }
    #[test]
    fn symlink_tabs_and_archive_hosts_are_affected() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let paths = vec![
            alias.display().to_string(),
            format!("archive:{}!/sub", alias.join("a.zip").display()),
            "/unrelated".into(),
        ];
        assert_eq!(affected_paths(&real, &paths).unwrap(), vec![0, 1]);
    }
    #[test]
    fn archive_sources_resolve_to_the_outer_mount() {
        assert_eq!(
            local_path(&dualfile_core::parse_ipc_path("archive:/mnt/archive.zip!/folder").unwrap()),
            Some("/mnt/archive.zip".into())
        );
    }
    #[test]
    fn aliases_are_reserved_and_release_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(temp.path().join("real"), temp.path().join("alias")).unwrap();
        let lease = job(&[temp.path().join("alias/new").to_string_lossy().into_owned()]).unwrap();
        assert!(registry().unmount(temp.path().join("real")).is_err());
        drop(lease);
        assert!(registry().unmount(temp.path().join("real")).is_ok());
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use dualfile_core::jobs::{JobManager, JobState};
    #[test]
    fn queued_cancellation_releases_mount_but_paused_work_retains_it() {
        let registry = Arc::new(Registry::default());
        let jobs = JobManager::default();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (release, blocked) = std::sync::mpsc::channel();
            let (started, ready) = std::sync::mpsc::channel();
            releases.push(release);
            jobs.submit(
                "blocker",
                Box::new(move |_| {
                    started.send(()).unwrap();
                    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                    Default::default()
                }),
            );
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let lease = registry
            .job(vec!["/source".into(), "/target".into()])
            .unwrap();
        let id = jobs.submit(
            "queued",
            Box::new(move |_| {
                let _lease = lease;
                Default::default()
            }),
        );
        assert_eq!(jobs.status(id).unwrap().state, JobState::Queued);
        jobs.pause(id);
        assert!(registry.unmount("/source".into()).is_err());
        assert!(registry.unmount("/target".into()).is_err());
        jobs.cancel(id);
        assert_eq!(jobs.wait(id).unwrap().state, JobState::Cancelled);
        assert!(registry.unmount("/source".into()).is_ok());
        for release in releases {
            release.send(()).unwrap();
        }
    }
    #[test]
    fn an_unmount_racing_alias_resolution_invalidates_admission() {
        let registry = Arc::new(Registry::default());
        let provisional = registry.job(vec!["/alias".into()]).unwrap();
        let unmount = registry.unmount("/actual".into()).unwrap();
        drop(unmount);
        assert!(provisional.resolve(vec!["/actual/file".into()]).is_err());
    }
}
