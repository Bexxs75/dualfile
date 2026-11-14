//! Folder watching: `notify` with debouncing. Only the folders the UI reports are watched
//! (non-recursive); every debounced change turns into one `dirChanged` per affected folder.

use crate::convert::{path_text, CoreEventDto, UiError};
use crate::state::EventSink;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Quiet time before a burst of file system events is reported.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// Which watched folders a set of changed paths concerns: the folder itself (renamed or
/// deleted) or a direct child of it. Deeper paths belong to folders that are not watched.
pub fn affected_dirs(watched: &BTreeSet<PathBuf>, changed: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut result = BTreeSet::new();
    for path in changed {
        if watched.contains(path) {
            result.insert(path.clone());
        }
        if let Some(parent) = path.parent() {
            if watched.contains(parent) {
                result.insert(parent.to_path_buf());
            }
        }
    }
    result
}

/// Difference between the watched folders and the wanted ones: (to remove, to add).
pub fn diff(
    current: &BTreeSet<PathBuf>,
    wanted: &BTreeSet<PathBuf>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    (
        current.difference(wanted).cloned().collect(),
        wanted.difference(current).cloned().collect(),
    )
}

pub struct WatchService {
    debouncer: Mutex<Debouncer<RecommendedWatcher>>,
    watched: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl WatchService {
    pub fn new(sink: Arc<dyn EventSink>, debounce: Duration) -> Result<Self, UiError> {
        let watched: Arc<Mutex<BTreeSet<PathBuf>>> = Arc::default();
        let shared = watched.clone();
        let debouncer = new_debouncer(debounce, move |result: DebounceEventResult| {
            // Errors of the watcher (queue overflow, vanished folder) carry nothing the UI
            // could act on; the manual refresh stays available.
            let Ok(events) = result else { return };
            let changed: Vec<PathBuf> = events.into_iter().map(|event| event.path).collect();
            let dirs = {
                let watched = shared.lock().unwrap_or_else(|e| e.into_inner());
                affected_dirs(&watched, &changed)
            };
            for dir in dirs {
                sink.emit(CoreEventDto::DirChanged {
                    path: path_text(&dir),
                });
            }
        })
        .map_err(|_| UiError::new("io", "Die Ordnerbeobachtung konnte nicht gestartet werden"))?;
        Ok(Self {
            debouncer: Mutex::new(debouncer),
            watched,
        })
    }

    /// Makes `wanted` the set of watched folders. Folders that cannot be watched (gone,
    /// no permission) are skipped; the returned list names them.
    pub fn set_paths(&self, wanted: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
        let mut debouncer = self.debouncer.lock().unwrap_or_else(|e| e.into_inner());
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        let (remove, add) = diff(&watched, wanted);
        for path in remove {
            let _ = debouncer.watcher().unwatch(&path);
            watched.remove(&path);
        }
        let mut failed = Vec::new();
        for path in add {
            match debouncer
                .watcher()
                .watch(&path, RecursiveMode::NonRecursive)
            {
                Ok(()) => {
                    watched.insert(path);
                }
                Err(_) => failed.push(path),
            }
        }
        failed
    }

    #[cfg(test)]
    pub fn watched(&self) -> BTreeSet<PathBuf> {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn children_and_the_folder_itself_map_to_the_watched_folder() {
        let watched = set(&["/a", "/b/c"]);
        let changed = vec![
            PathBuf::from("/a/file"),
            PathBuf::from("/a/other"),
            PathBuf::from("/b/c"),
            PathBuf::from("/b/c/deep/file"),
            PathBuf::from("/x/y"),
        ];
        assert_eq!(affected_dirs(&watched, &changed), set(&["/a", "/b/c"]));
    }

    #[test]
    fn unrelated_changes_report_nothing() {
        let watched = set(&["/a"]);
        assert!(affected_dirs(&watched, &[PathBuf::from("/a/b/c"), PathBuf::from("/")]).is_empty());
        assert!(affected_dirs(&BTreeSet::new(), &[PathBuf::from("/a/b")]).is_empty());
    }

    #[test]
    fn diff_lists_what_to_unwatch_and_what_to_watch() {
        let (remove, add) = diff(&set(&["/a", "/b"]), &set(&["/b", "/c"]));
        assert_eq!(remove, vec![PathBuf::from("/a")]);
        assert_eq!(add, vec![PathBuf::from("/c")]);
        let (remove, add) = diff(&set(&["/a"]), &set(&["/a"]));
        assert!(remove.is_empty() && add.is_empty());
    }

    struct ChannelSink(Mutex<mpsc::Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap().send(event);
        }
    }

    fn service() -> (WatchService, mpsc::Receiver<CoreEventDto>) {
        let (tx, rx) = mpsc::channel();
        let service = WatchService::new(
            Arc::new(ChannelSink(Mutex::new(tx))),
            Duration::from_millis(60),
        )
        .expect("watcher starts");
        (service, rx)
    }

    #[test]
    fn new_files_in_a_watched_folder_report_only_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (service, rx) = service();
        assert!(service
            .set_paths(&set(&[root.to_str().unwrap()]))
            .is_empty());
        for i in 0..5 {
            std::fs::write(root.join(format!("f{i}")), b"x").unwrap();
        }
        let expected = CoreEventDto::DirChanged {
            path: path_text(&root),
        };
        let first = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("dirChanged arrives");
        assert_eq!(first, expected);
        // Whatever follows (the debouncer may split a burst) names the same folder only,
        // and the stream goes quiet.
        while let Ok(more) = rx.recv_timeout(Duration::from_millis(500)) {
            assert_eq!(more, expected);
        }
    }

    #[test]
    fn a_burst_of_writes_is_debounced_into_far_fewer_events() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (tx, rx) = mpsc::channel();
        let service = WatchService::new(
            Arc::new(ChannelSink(Mutex::new(tx))),
            Duration::from_millis(300),
        )
        .expect("watcher starts");
        service.set_paths(&set(&[root.to_str().unwrap()]));
        let file = root.join("busy");
        for round in 0..40 {
            std::fs::write(&file, format!("{round}")).unwrap();
        }
        std::thread::sleep(Duration::from_millis(900));
        let events = rx.try_iter().count();
        assert!((1..=3).contains(&events), "40 writes gave {events} events");
    }

    #[test]
    fn unwatched_folders_stay_silent_and_vanished_ones_are_reported_as_failed() {
        let watched = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (service, rx) = service();
        let missing = watched.path().join("nicht-da");
        let failed = service.set_paths(&set(&[
            watched.path().to_str().unwrap(),
            missing.to_str().unwrap(),
        ]));
        assert_eq!(failed, vec![missing]);
        assert_eq!(service.watched().len(), 1);
        std::fs::write(other.path().join("f"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }

    #[test]
    fn replacing_the_set_stops_watching_the_old_folder() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let (service, rx) = service();
        service.set_paths(&set(&[first.path().to_str().unwrap()]));
        service.set_paths(&set(&[second.path().to_str().unwrap()]));
        assert_eq!(service.watched(), set(&[second.path().to_str().unwrap()]));
        std::fs::write(first.path().join("f"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
        std::fs::write(second.path().join("f"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
