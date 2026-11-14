//! Commands for the search over many files and archives. Part of `AppState`; the DTOs and
//! the result store live in `crate::search`.

use super::AppState;
use crate::convert::{JobTitleDto, UiError};
use crate::search::{page, SearchOptionsDto, SearchPageDto};
use dualfile_core::ops::OpReport;
use dualfile_core::search;
use dualfile_core::vfs::EntryKind;

fn not_found_search() -> UiError {
    UiError::new("notFound", "Das Suchergebnis ist nicht (mehr) vorhanden")
}

impl AppState {
    /// Starts a search below `root` as a job. The job number is the id of the result:
    /// `search_page` reads it (also while the job still runs it answers `notFound`; the
    /// result appears when the job ends), `search_close` frees it. A cancelled search keeps
    /// its partial result, marked `cancelled` and `incomplete`.
    pub fn search_start(&self, root: &str, options: &SearchOptionsDto) -> Result<u64, UiError> {
        let opts = options.to_core()?;
        let start = self.path(root)?;
        search::check_start(&start, &opts)?;
        let entry = self.vfs(&start)?.stat(&start)?;
        if entry.kind != EntryKind::Dir {
            return Err(UiError::new(
                "notDirectory",
                format!("Der Pfad ist kein Ordner: {}", start.path().display()),
            ));
        }
        let router = self.router.clone();
        let reservation = self.searches.reserve()?;
        let name = format!(
            "Suchen: {}",
            start
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| start.path().to_string_lossy().into_owned())
        );
        let title = JobTitleDto::new(
            "search",
            start
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| start.path().to_string_lossy().into_owned()),
            0,
        );
        Ok(
            self.submit_job_titled(name, title, Vec::new(), move |resolver| {
                Box::new(move |job| {
                    let mut report = OpReport::default();
                    match search::search(&router, &start, &opts, job) {
                        Ok(result) => {
                            report.cancelled = result.cancelled;
                            // Delivered before the job counts as finished, so the UI can read it.
                            match resolver.job.get() {
                                Some(id) => reservation.finish(id, result),
                                None => drop(reservation),
                            }
                        }
                        Err(error) => {
                            drop(reservation);
                            report.failed.push((start, error));
                        }
                    }
                    report
                })
            }),
        )
    }

    pub fn search_page(&self, id: u64, offset: u64, limit: u64) -> Result<SearchPageDto, UiError> {
        let result = self.searches.get(id).ok_or_else(not_found_search)?;
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(page(&result, id, offset, limit))
    }

    /// Frees a result.
    pub fn search_close(&self, id: u64) -> Result<(), UiError> {
        if !self.searches.close(id) {
            return Err(not_found_search());
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn open_searches(&self) -> usize {
        self.searches.len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AppState, EventSink};
    use crate::convert::{CoreEventDto, PackFormatDto, UiError};
    use crate::search::{SearchOptionsDto, MAX_SEARCHES, MAX_SEARCH_PAGE};
    use dualfile_core::jobs::{JobState, JobStatus};
    use std::fs;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct ChannelSink(Mutex<Sender<CoreEventDto>>);
    impl EventSink for ChannelSink {
        fn emit(&self, event: CoreEventDto) {
            let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send(event);
        }
    }

    struct Fixture {
        state: AppState,
        root: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("home")).unwrap();
        let (tx, _events) = channel();
        let state = AppState::new(
            Arc::new(ChannelSink(Mutex::new(tx))),
            root.path().join("home"),
            root.path().join("config"),
        );
        Fixture { state, root }
    }

    impl Fixture {
        fn p(&self, rel: &str) -> String {
            self.root.path().join(rel).to_string_lossy().into_owned()
        }
        fn put(&self, rel: &str, content: &str) {
            let path = self.root.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        fn finish(&self, id: u64) -> JobStatus {
            self.state.jobs.wait(id).expect("job exists")
        }
        /// Searches `root` and waits; returns the result id.
        fn search(&self, root: &str, options: &SearchOptionsDto) -> u64 {
            let id = self.state.search_start(&self.p(root), options).unwrap();
            assert_eq!(self.finish(id).state, JobState::Done);
            id
        }
    }

    fn text(t: &str) -> SearchOptionsDto {
        SearchOptionsDto {
            text: Some(t.into()),
            ..SearchOptionsDto::default()
        }
    }

    fn name(n: &str) -> SearchOptionsDto {
        SearchOptionsDto {
            name: Some(n.into()),
            ..SearchOptionsDto::default()
        }
    }

    fn start_error(f: &Fixture, root: &str, options: &SearchOptionsDto) -> UiError {
        f.state.search_start(root, options).unwrap_err()
    }

    #[test]
    fn a_search_runs_as_a_job_and_is_read_in_pages() {
        let f = fixture();
        for i in 0..5 {
            f.put(&format!("d/f{i}.txt"), &format!("a\nneedle {i}\n"));
        }
        f.put("d/other.md", "nothing\n");
        let id = f.search("d", &text("needle"));
        let page = f.state.search_page(id, 0, 100).unwrap();
        assert_eq!(page.id, id);
        assert_eq!(page.total, 5);
        assert_eq!(page.hits.len(), 5);
        assert_eq!(page.hits[0].line, Some(2));
        assert_eq!(page.hits[0].excerpt.as_deref(), Some("needle 0"));
        assert_eq!(page.hits[0].kind, "file");
        assert!(page.hits[0].path.ends_with("d/f0.txt"));
        assert_eq!(page.summary.total, 5);
        assert!(!page.summary.incomplete && !page.summary.cancelled);
        assert_eq!(page.summary.files_visited, 6);
        assert_eq!(page.summary.stopped, None);
        let first = f.state.search_page(id, 0, 2).unwrap();
        let second = f.state.search_page(id, 2, 2).unwrap();
        assert_eq!((first.hits.len(), second.hits.len()), (2, 2));
        assert_ne!(first.hits[0].path, second.hits[0].path);
        // behind the end: empty, the total stays
        let past = f.state.search_page(id, 1000, 10).unwrap();
        assert!(past.hits.is_empty());
        assert_eq!(past.total, 5);
        // pages are limited
        for i in 0..(MAX_SEARCH_PAGE + 20) {
            f.put(&format!("big/n{i}.txt"), "x");
        }
        let big = f.search("big", &name("n*.txt"));
        let all = f.state.search_page(big, 0, u64::MAX).unwrap();
        assert_eq!(all.hits.len(), MAX_SEARCH_PAGE);
        assert_eq!(all.total as usize, MAX_SEARCH_PAGE + 20);
        // a name search sees the same tree
        let by_name = f.search("d", &name("*.md"));
        assert_eq!(f.state.search_page(by_name, 0, 10).unwrap().total, 1);
    }

    #[test]
    fn hits_in_archives_come_back_as_archive_paths_that_list_opens() {
        let f = fixture();
        f.put("src/a.txt", "one\nneedle in a zip\n");
        let id = f
            .state
            .pack_archive(
                &[f.p("src/a.txt")],
                &f.p(""),
                "pack.zip",
                PackFormatDto::Zip,
            )
            .unwrap();
        assert_eq!(f.finish(id).state, JobState::Done);
        fs::remove_dir_all(f.root.path().join("src")).unwrap();
        let none = f.search("", &text("needle"));
        assert_eq!(f.state.search_page(none, 0, 10).unwrap().total, 0);
        let options = SearchOptionsDto {
            in_archives: true,
            ..text("needle")
        };
        let id = f.search("", &options);
        let page = f.state.search_page(id, 0, 10).unwrap();
        assert_eq!(page.total, 1, "{:?}", page.summary.issues);
        assert_eq!(page.summary.archives_searched, 1);
        let hit = &page.hits[0];
        assert!(hit.path.starts_with("archive:"), "{}", hit.path);
        assert_eq!(hit.line, Some(2));
        let parent = hit.path.rsplit_once('/').unwrap().0;
        // `parent` is the archive root without its closing slash
        let listing = f.state.list(&format!("{parent}/")).unwrap();
        assert!(listing.entries.iter().any(|e| e.name == "a.txt"));
    }

    #[test]
    fn bad_paths_and_options_are_refused_before_a_job_exists() {
        let f = fixture();
        f.put("file", "x");
        f.put("d/a", "x");
        let ok = text("x");
        for (root, kind) in [
            ("relativ".to_string(), "invalidPath"),
            (String::new(), "invalidPath"),
            ("/a/../b".to_string(), "invalidPath"),
            ("/a\0b".to_string(), "invalidPath"),
            (format!("/{}", "x".repeat(5000)), "invalidPath"),
            (f.p("fehlt"), "notFound"),
            (f.p("file"), "notDirectory"),
        ] {
            let error = start_error(&f, &root, &ok);
            assert_eq!(error.kind, kind, "{root:?}: {error:?}");
        }
        let regex = |t: &str| SearchOptionsDto {
            text_regex: true,
            ..text(t)
        };
        for options in [
            SearchOptionsDto::default(),
            text(""),
            text(&"x".repeat(2000)),
            text("a\0b"),
            regex("("),
            regex(&"(".repeat(200)),
            regex("(a{1000}){1000}"),
            SearchOptionsDto {
                min_size: Some(10),
                max_size: Some(5),
                ..text("x")
            },
            SearchOptionsDto {
                modified_after: Some(10),
                modified_before: Some(5),
                ..text("x")
            },
            SearchOptionsDto {
                archive_depth: 0,
                ..text("x")
            },
            SearchOptionsDto {
                archive_depth: 4,
                ..text("x")
            },
        ] {
            let error = start_error(&f, &f.p("d"), &options);
            assert_eq!(error.kind, "invalidInput", "{options:?}: {error:?}");
        }
        assert!(f.state.jobs().is_empty());
        assert_eq!(f.state.open_searches(), 0);
        // nothing was reserved by the refused calls
        assert!(f.state.search_start(&f.p("d"), &ok).is_ok());
    }

    #[test]
    fn servers_need_an_explicit_choice() {
        let f = fixture();
        let remote = SearchOptionsDto {
            include_remote: false,
            ..text("x")
        };
        let error = start_error(&f, "ftp:abc!/data", &remote);
        assert_eq!(error.kind, "unsupported", "{error:?}");
        // chosen, but there is no such connection: refused as disconnected, nothing contacted
        let chosen = SearchOptionsDto {
            include_remote: true,
            ..text("x")
        };
        let error = start_error(&f, "ftp:abc!/data", &chosen);
        assert_eq!(error.kind, "disconnected", "{error:?}");
        assert!(f.state.jobs().is_empty());
        assert_eq!(f.state.open_searches(), 0);
    }

    #[test]
    fn closed_and_unknown_results_are_gone_and_slots_are_released() {
        let f = fixture();
        f.put("d/a.txt", "x");
        let id = f.search("d", &name("*.txt"));
        assert_eq!(f.state.open_searches(), 1);
        assert_eq!(
            f.state.search_page(id + 100, 0, 1).unwrap_err().kind,
            "notFound"
        );
        f.state.search_close(id).unwrap();
        assert_eq!(f.state.open_searches(), 0);
        assert_eq!(f.state.search_page(id, 0, 1).unwrap_err().kind, "notFound");
        assert_eq!(f.state.search_close(id).unwrap_err().kind, "notFound");
    }

    #[test]
    fn at_most_four_results_exist_at_the_same_time() {
        let f = fixture();
        f.put("d/a.txt", "x");
        let ids: Vec<u64> = (0..MAX_SEARCHES)
            .map(|_| f.search("d", &name("*.txt")))
            .collect();
        let error = start_error(&f, &f.p("d"), &name("*.txt"));
        assert_eq!(error.kind, "invalidInput");
        assert!(error.message.contains(&MAX_SEARCHES.to_string()));
        f.state.search_close(ids[0]).unwrap();
        let again = f.search("d", &name("*.txt"));
        assert_eq!(f.state.open_searches(), MAX_SEARCHES);
        assert!(f.state.search_page(again, 0, 1).is_ok());
    }

    #[test]
    fn a_search_cancelled_before_it_starts_leaves_no_result_and_frees_its_slot() {
        let f = fixture();
        f.put("d/a.txt", "x");
        // Queue behind two blocking jobs so the search is cancelled before it starts.
        let (started, release) = std::sync::mpsc::channel::<()>();
        let release = Arc::new(Mutex::new(release));
        let mut blockers = Vec::new();
        for _ in 0..2 {
            let release = Arc::clone(&release);
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            blockers.push(f.state.submit_job("block".into(), vec![], move |_| {
                Box::new(move |_| {
                    let _ = tx.send(());
                    let _ = release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10));
                    Default::default()
                })
            }));
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let id = f.state.search_start(&f.p("d"), &name("*.txt")).unwrap();
        f.state.cancel_job(id).unwrap();
        for _ in &blockers {
            let _ = started.send(());
        }
        assert_eq!(f.finish(id).state, JobState::Cancelled);
        for id in blockers {
            f.finish(id);
        }
        assert_eq!(f.state.open_searches(), 0);
        assert_eq!(f.state.search_page(id, 0, 1).unwrap_err().kind, "notFound");
        // the slot is free again
        let ok = f.search("d", &name("*.txt"));
        assert!(f.state.search_page(ok, 0, 1).is_ok());
    }

    #[test]
    fn a_search_cancelled_while_running_keeps_a_partial_marked_result() {
        let f = fixture();
        for i in 0..300 {
            f.put(&format!("d/d{i:03}/a.txt"), "needle\n");
        }
        let id = f.state.search_start(&f.p("d"), &text("needle")).unwrap();
        // The cancel can arrive at any stage (queued, running, finished); every outcome must
        // be consistent: no result, a complete result, or a partial one that says so. (The
        // core test `cancelling_during_the_search_stops_it_with_partial_hits` and
        // `a_cancelled_result_is_kept_and_marked` cover the cut-short case deterministically.)
        let _ = f.state.cancel_job(id);
        let status = f.finish(id);
        match f.state.search_page(id, 0, 1000) {
            Ok(page) => {
                if page.summary.cancelled {
                    assert!(page.summary.incomplete);
                    assert_eq!(page.summary.stopped, Some("cancelled"));
                } else {
                    assert_eq!(page.total, 300);
                }
            }
            Err(error) => {
                assert_eq!(error.kind, "notFound");
                assert_eq!(status.state, JobState::Cancelled);
            }
        }
        assert!(f.state.open_searches() <= 1);
    }
}
