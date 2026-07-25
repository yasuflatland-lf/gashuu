//! Off-UI-thread single-open controller.
//!
//! Opening an archive or folder can block while a cloud-synced or network path
//! hydrates. This controller moves only that read-only probe onto a rayon worker:
//! archive/folder open, page listing, skipped count, listing truncation,
//! canonicalization, and path metadata. The `open-finalize` callback drains the
//! result and runs every state mutation and persistence effect on the UI thread.
//!
//! Each start bumps an epoch on the UI thread and installs a fresh result slot.
//! A late worker from an older generation writes only to its orphaned slot, and
//! [`marshal_to_ui`] drops its stale callback before it can touch the UI.

use crate::ui_marshal::marshal_to_ui;
use crate::ViewerWindow;
use gashuu_core::{canonical_identity, ArchiveLoader, ArchivePolicy, PageSource};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// A successfully opened source plus all filesystem metadata needed by the
/// UI-thread apply half.
pub(crate) struct OpenProbe {
    pub(crate) source: Arc<dyn PageSource>,
    pub(crate) skipped: usize,
    pub(crate) truncated: bool,
    pub(crate) canonical: PathBuf,
    pub(crate) is_dir: bool,
}

/// The `Send` result of probing one path without mutating application state.
pub(crate) enum OpenProbeOutcome {
    Opened(OpenProbe),
    Failed { error: String, path_exists: bool },
}

/// Open and inspect one source without touching `ViewerState`, `Library`, or
/// `Settings`. This is the complete worker-thread portion of a single open.
pub(crate) fn probe_open(path: &Path, policy: ArchivePolicy) -> OpenProbeOutcome {
    match ArchiveLoader::open_with_policy(path, policy) {
        Ok(source) => {
            let skipped = source.skipped_count();
            let truncated = source.listing_truncated();
            if skipped > 0 {
                tracing::warn!(skipped, path = %path.display(), "entries skipped while opening path");
            }
            if truncated {
                tracing::warn!(
                    path = %path.display(),
                    "archive listing truncated; later pages are missing"
                );
            }
            OpenProbeOutcome::Opened(OpenProbe {
                source,
                skipped,
                truncated,
                // The same persistable identity `Library::add` uses, including its
                // fallback when canonicalization produces a non-UTF-8 path.
                canonical: canonical_identity(path),
                is_dir: path.is_dir(),
            })
        }
        Err(error) => OpenProbeOutcome::Failed {
            error: error.to_string(),
            path_exists: path.exists(),
        },
    }
}

type PendingOutcome = Option<(PathBuf, OpenProbeOutcome)>;

/// Owns the single-open epoch and the current generation's worker result.
///
/// The outer `RefCell` is replaced only on the UI thread. The inner
/// `Arc<Mutex<_>>` is the `Send` bridge used by the rayon worker; superseded
/// workers retain only an orphaned slot.
pub(crate) struct OpenController {
    epoch: Arc<AtomicUsize>,
    pending: RefCell<Arc<Mutex<PendingOutcome>>>,
}

impl OpenController {
    /// Build the controller once during UI setup.
    pub(crate) fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            pending: RefCell::new(Arc::new(Mutex::new(None))),
        }
    }

    /// Probe `path` on a rayon worker and marshal one epoch-guarded finalize
    /// callback back to the Slint event loop.
    pub(crate) fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        path: PathBuf,
        policy: ArchivePolicy,
    ) {
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;
        let pending = Arc::new(Mutex::new(None));
        *self.pending.borrow_mut() = Arc::clone(&pending);
        let epoch = Arc::clone(&self.epoch);

        rayon::spawn(move || {
            let outcome = probe_open(&path, policy);
            *pending.lock().expect("open outcome mutex poisoned") = Some((path, outcome));
            marshal_to_ui(ui_weak, epoch, my_epoch, "open-finalize", move |ui| {
                ui.invoke_open_finalize(my_epoch as i32)
            });
        });
    }

    /// Drain the current generation's worker result exactly once. A stale
    /// finalize returns `None` without touching the live generation's slot.
    pub(crate) fn take_outcome(&self, my_epoch: usize) -> Option<(PathBuf, OpenProbeOutcome)> {
        if self.epoch.load(Relaxed) != my_epoch {
            return None;
        }
        self.pending
            .borrow()
            .lock()
            .expect("open outcome mutex poisoned")
            .take()
    }

    #[cfg(test)]
    fn stash_for_test(&self, path: PathBuf, outcome: OpenProbeOutcome) {
        *self.pending.borrow().lock().expect("mutex") = Some((path, outcome));
    }

    #[cfg(test)]
    fn bump_for_test(&self) {
        self.epoch.fetch_add(1, Relaxed);
        *self.pending.borrow_mut() = Arc::new(Mutex::new(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn make_book_dir(parent: &Path, name: &str, pages: usize) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).expect("create book dir");
        for page in 0..pages {
            std::fs::write(dir.join(format!("page{page:03}.png")), []).expect("write fixture page");
        }
        dir
    }

    #[test]
    fn probe_open_opens_a_real_folder() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = make_book_dir(root.path(), "book", 3);

        let OpenProbeOutcome::Opened(probe) =
            probe_open(&path, gashuu_core::ArchivePolicy::default())
        else {
            panic!("a real folder must open");
        };

        assert_eq!(probe.source.list_pages().len(), 3);
        assert_eq!(probe.skipped, 0);
        assert_eq!(
            probe.canonical,
            std::fs::canonicalize(&path).expect("canonical fixture")
        );
        assert!(probe.is_dir);
    }

    #[test]
    fn probe_open_keeps_empty_folder_for_apply_classification() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = make_book_dir(root.path(), "empty", 0);

        let OpenProbeOutcome::Opened(probe) =
            probe_open(&path, gashuu_core::ArchivePolicy::default())
        else {
            panic!("an empty folder opens cleanly");
        };

        assert!(probe.source.list_pages().is_empty());
        assert_eq!(probe.skipped, 0);
        assert!(probe.is_dir);
    }

    #[test]
    fn probe_open_reports_missing_path_without_a_ui_thread_stat() {
        let root = tempfile::tempdir().expect("tempdir");
        let missing = root.path().join("missing");

        match probe_open(&missing, gashuu_core::ArchivePolicy::default()) {
            OpenProbeOutcome::Failed { error, path_exists } => {
                assert!(!error.is_empty());
                assert!(!path_exists);
            }
            OpenProbeOutcome::Opened(_) => panic!("a missing path must fail"),
        }
    }

    #[test]
    fn take_outcome_drops_superseded_and_drains_current_once() {
        let controller = OpenController::new();
        controller.epoch.store(1, Relaxed);
        controller.stash_for_test(
            PathBuf::from("/old"),
            OpenProbeOutcome::Failed {
                error: "old".into(),
                path_exists: false,
            },
        );

        controller.bump_for_test();
        assert!(controller.take_outcome(1).is_none());

        controller.stash_for_test(
            PathBuf::from("/new"),
            OpenProbeOutcome::Failed {
                error: "new".into(),
                path_exists: true,
            },
        );
        let (path, outcome) = controller.take_outcome(2).expect("current outcome");
        assert_eq!(path, PathBuf::from("/new"));
        assert!(matches!(
            outcome,
            OpenProbeOutcome::Failed {
                path_exists: true,
                ..
            }
        ));
        assert!(controller.take_outcome(2).is_none());
    }
}
