//! Book availability is a derived, eventually consistent display hint. The UI
//! thread reads an in-memory inventory and never probes the filesystem for it.
//! Unknown books are shown as available on both platforms. Opening a book
//! still reports source errors through the normal open path.

use gashuu_core::Book;

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn book_path_exists(book: &Book) -> bool {
    crate::book_path::exists(&crate::book_path::locate(book.id()))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use desktop::{desktop_inventory, request_check};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_available(book: &Book) -> bool {
    desktop_inventory().is_available(book.id())
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use gashuu_core::BookId;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::{mpsc, Arc, OnceLock};
    use std::thread;

    #[derive(Default)]
    pub(crate) struct DesktopAvailability {
        cache: RefCell<HashMap<BookId, bool>>,
        generation: Cell<u64>,
    }

    impl DesktopAvailability {
        pub(super) fn is_available(&self, id: &BookId) -> bool {
            self.cache.borrow().get(id).copied().unwrap_or(true)
        }

        pub(crate) fn begin_check(&self) -> u64 {
            let generation = self.generation.get() + 1;
            self.generation.set(generation);
            generation
        }

        pub(crate) fn apply(
            &self,
            generation: u64,
            results: Vec<(BookId, bool)>,
        ) -> Option<Vec<(BookId, bool)>> {
            if generation != self.generation.get() {
                return None;
            }

            let mut cache = self.cache.borrow_mut();
            let mut changed = Vec::new();
            for (id, available) in results {
                let previous = cache.insert(id.clone(), available).unwrap_or(true);
                if previous != available {
                    changed.push((id, available));
                }
            }
            Some(changed)
        }
    }

    pub(crate) fn desktop_inventory() -> Rc<DesktopAvailability> {
        thread_local! {
            static INVENTORY: Rc<DesktopAvailability> = Rc::new(DesktopAvailability::default());
        }
        INVENTORY.with(Rc::clone)
    }

    type CheckDone = Box<dyn FnOnce(u64, Vec<(BookId, bool)>) + Send>;

    struct Job {
        generation: u64,
        ids: Vec<BookId>,
        done: CheckDone,
    }

    pub(super) struct AvailabilityProber {
        jobs: mpsc::Sender<Job>,
        latest: Arc<AtomicU64>,
    }

    impl AvailabilityProber {
        pub(super) fn with_stat(stat: Arc<dyn Fn(&BookId) -> bool + Send + Sync>) -> Self {
            let (jobs, pending) = mpsc::channel::<Job>();
            let latest = Arc::new(AtomicU64::new(0));
            let worker_latest = Arc::clone(&latest);
            thread::Builder::new()
                .name("gashuu-availability".into())
                .spawn(move || {
                    'jobs: while let Ok(mut job) = pending.recv() {
                        while let Ok(newer) = pending.try_recv() {
                            job = newer;
                        }

                        let mut results = Vec::with_capacity(job.ids.len());
                        for id in job.ids {
                            if worker_latest.load(Relaxed) != job.generation {
                                continue 'jobs;
                            }
                            let available = stat(&id);
                            results.push((id, available));
                        }
                        // A newer request may arrive during the final stat.
                        if worker_latest.load(Relaxed) == job.generation {
                            (job.done)(job.generation, results);
                        }
                    }
                })
                .expect("spawn book availability worker");
            Self { jobs, latest }
        }

        pub(super) fn submit(
            &self,
            generation: u64,
            ids: Vec<BookId>,
            done: impl FnOnce(u64, Vec<(BookId, bool)>) + Send + 'static,
        ) {
            self.latest.store(generation, Relaxed);
            let job = Job {
                generation,
                ids,
                done: Box::new(done),
            };
            if self.jobs.send(job).is_err() {
                tracing::warn!("book availability worker disconnected");
            }
        }
    }

    pub(crate) fn request_check(
        generation: u64,
        ids: Vec<BookId>,
        done: impl FnOnce(u64, Vec<(BookId, bool)>) + Send + 'static,
    ) {
        static PROBER: OnceLock<AvailabilityProber> = OnceLock::new();
        let prober = PROBER.get_or_init(|| {
            AvailabilityProber::with_stat(Arc::new(|id| {
                crate::book_path::exists(&crate::book_path::locate(id))
            }))
        });
        prober.submit(generation, ids, done);
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use web::WebAvailability;

#[cfg(any(test, target_arch = "wasm32"))]
mod web {
    use gashuu_core::BookId;
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    pub(crate) struct WebAvailability(Arc<Mutex<Option<HashSet<String>>>>);

    impl WebAvailability {
        pub(crate) fn contains(&self, id: &BookId) -> bool {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|set| set.contains(id.as_str()))
        }
        pub(crate) fn replace(&self, present: Vec<BookId>) {
            *self.0.lock().unwrap() = Some(present.into_iter().map(|id| id.to_string()).collect());
        }
        pub(crate) fn insert(&self, id: &BookId) {
            if let Some(set) = self.0.lock().unwrap().as_mut() {
                set.insert(id.to_string());
            }
        }
        pub(crate) fn remove(&self, id: &BookId) {
            if let Some(set) = self.0.lock().unwrap().as_mut() {
                set.remove(id.as_str());
            }
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn web_inventory() -> WebAvailability {
    thread_local! { static INVENTORY: WebAvailability = WebAvailability::default(); }
    INVENTORY.with(Clone::clone)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_available(book: &Book) -> bool {
    web_inventory().contains(book.id())
}

#[cfg(all(test, target_arch = "wasm32"))]
pub(crate) use is_available as book_path_exists;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::desktop::{AvailabilityProber, DesktopAvailability};
    use super::*;
    use gashuu_core::{BookId, Library};
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn unknown_book_is_available_until_checked() {
        let inventory = DesktopAvailability::default();
        let id = BookId::new("/manga/missing.cbz");

        let generation = inventory.begin_check();
        assert!(inventory.is_available(&id));
        assert_eq!(
            inventory.apply(generation, vec![(id.clone(), false)]),
            Some(vec![(id.clone(), false)])
        );
        assert!(!inventory.is_available(&id));
    }

    #[test]
    fn stale_generation_results_are_ignored() {
        let inventory = DesktopAvailability::default();
        let id = BookId::new("/manga/missing.cbz");
        let stale = inventory.begin_check();
        let current = inventory.begin_check();

        assert_eq!(
            inventory.apply(current, vec![(id.clone(), false)]),
            Some(vec![(id.clone(), false)])
        );
        assert_eq!(inventory.apply(stale, vec![(id.clone(), true)]), None);
        assert!(!inventory.is_available(&id));
    }

    #[test]
    fn apply_returns_only_changed_entries() {
        let inventory = DesktopAvailability::default();
        let present = BookId::new("/manga/present.cbz");
        let missing = BookId::new("/manga/missing.cbz");
        let restored = BookId::new("/manga/restored.cbz");
        let generation = inventory.begin_check();
        assert_eq!(
            inventory.apply(
                generation,
                vec![
                    (present.clone(), true),
                    (missing.clone(), false),
                    (restored.clone(), false),
                ],
            ),
            Some(vec![(missing.clone(), false), (restored.clone(), false)])
        );

        let generation = inventory.begin_check();
        assert_eq!(
            inventory.apply(
                generation,
                vec![(present, true), (missing, false), (restored.clone(), true),],
            ),
            Some(vec![(restored.clone(), true)])
        );
        assert!(inventory.is_available(&restored));
    }

    #[test]
    fn prober_reports_each_id_with_the_injected_stat() {
        let present = BookId::new("/manga/present.cbz");
        let missing = BookId::new("/manga/missing.cbz");
        let prober = AvailabilityProber::with_stat(Arc::new({
            let present = present.clone();
            move |id| id == &present
        }));
        let (done, result) = mpsc::channel();

        prober.submit(
            1,
            vec![present.clone(), missing.clone()],
            move |gen, results| {
                done.send((gen, results)).unwrap();
            },
        );

        assert_eq!(
            result.recv_timeout(PROBE_TIMEOUT).unwrap(),
            (1, vec![(present, true), (missing, false)])
        );
    }

    #[test]
    fn newer_job_supersedes_an_older_one() {
        let old = BookId::new("/manga/old.cbz");
        let skipped = BookId::new("/manga/skipped.cbz");
        let new = BookId::new("/manga/new.cbz");
        let barrier = Arc::new(Barrier::new(2));
        let (started, waiting) = mpsc::channel();
        let prober = AvailabilityProber::with_stat(Arc::new({
            let old = old.clone();
            let new = new.clone();
            let barrier = Arc::clone(&barrier);
            move |id| {
                if id == &old {
                    started.send(()).unwrap();
                    barrier.wait();
                } else {
                    assert_eq!(id, &new);
                }
                true
            }
        }));
        let (old_done, old_result) = mpsc::channel();
        let (new_done, new_result) = mpsc::channel();

        prober.submit(1, vec![old, skipped], move |gen, _| {
            old_done.send(gen).unwrap();
        });
        waiting.recv_timeout(PROBE_TIMEOUT).unwrap();
        prober.submit(2, vec![new.clone()], move |gen, results| {
            new_done.send((gen, results)).unwrap();
        });
        barrier.wait();

        assert_eq!(
            new_result.recv_timeout(PROBE_TIMEOUT).unwrap(),
            (2, vec![(new, true)])
        );
        assert_eq!(old_result.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn prober_never_stats_on_the_calling_thread() {
        let caller = thread::current().id();
        let (stat_thread, recorded) = mpsc::channel();
        let prober = AvailabilityProber::with_stat(Arc::new(move |_| {
            stat_thread.send(thread::current().id()).unwrap();
            true
        }));

        prober.submit(1, vec![BookId::new("/manga/book.cbz")], |_, _| {});

        assert_ne!(recorded.recv_timeout(PROBE_TIMEOUT).unwrap(), caller);
    }

    #[test]
    fn book_path_exists_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let mut library = Library::new();
        let (present_id, title) = crate::book_path::identify(&folder, true).unwrap();
        let missing_id = BookId::new("/manga/missing.cbz");
        library.add(present_id.clone(), title);
        library.register_opened(&missing_id, "missing", None);
        let present = library
            .books()
            .iter()
            .find(|book| book.id() == &present_id)
            .unwrap();
        let missing = library
            .books()
            .iter()
            .find(|book| book.id() == &missing_id)
            .unwrap();
        assert!(book_path_exists(present));
        assert!(!book_path_exists(missing));
    }
}

#[cfg(test)]
#[test]
fn web_inventory_tracks_import_and_removal_after_sweep() {
    use gashuu_core::BookId;
    let inventory = WebAvailability::default();
    let a = BookId::new("books/a/book");
    let b = BookId::new("books/b/book");
    assert!(inventory.contains(&a));
    inventory.replace(vec![b.clone()]);
    assert!(!inventory.contains(&a));
    assert!(inventory.contains(&b));
    inventory.insert(&a);
    assert!(inventory.contains(&a));
    inventory.remove(&b);
    assert!(!inventory.contains(&b));
}
