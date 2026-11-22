//! Off-UI-thread single-open controller.
//!
//! Opening an archive or folder can block while a cloud-synced or network path
//! hydrates. This controller moves only that read-only probe onto a rayon worker:
//! archive/folder open, page listing, skipped count, listing truncation,
//! canonicalization, and path metadata. The `open-finalize` callback drains the
//! result and runs every state mutation and persistence effect on the UI thread.
//!
//! Each start begins a new [`Generation`]. The worker delivers the probe result
//! through its ticket, which also guards the finalize callback on the UI thread.

use crate::ui_marshal::Generation;
use crate::{book_path, worker, ViewerWindow};
use gashuu_core::{ArchivePolicy, BookId, PageSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A successfully opened source plus all filesystem metadata needed by the
/// UI-thread apply half.
pub(crate) struct OpenProbe {
    pub(crate) source: Arc<dyn PageSource>,
    pub(crate) skipped: usize,
    pub(crate) truncated: bool,
    pub(crate) id: BookId,
    pub(crate) title: String,
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
    match book_path::open(path, policy) {
        Ok(source) => {
            let is_dir = book_path::is_dir(path);
            let (id, title) = match book_path::identify(path, is_dir) {
                Ok(identified) => identified,
                Err(error) => {
                    return OpenProbeOutcome::Failed {
                        error: error.to_string(),
                        path_exists: book_path::exists(path),
                    }
                }
            };
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
                id,
                title,
                is_dir,
            })
        }
        Err(error) => OpenProbeOutcome::Failed {
            error: error.to_string(),
            path_exists: book_path::exists(path),
        },
    }
}

/// Holds the worker result for the current single-open operation.
pub(crate) struct OpenController {
    generation: Generation<Option<(PathBuf, OpenProbeOutcome)>>,
}

impl OpenController {
    /// Build the controller once during UI setup.
    pub(crate) fn new() -> Self {
        Self {
            generation: Generation::new(),
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
        let ticket = self.generation.begin();
        worker::spawn(move || {
            let outcome = probe_open(&path, policy);
            ticket.deliver(|slot| *slot = Some((path, outcome)));
            let id = ticket.id();
            ticket.marshal(ui_weak, "open-finalize", move |ui| {
                ui.invoke_open_finalize(id as i32);
            });
        });
    }

    /// Drain the current generation's worker result exactly once. A stale
    /// finalize returns `None` without touching the live generation's slot.
    pub(crate) fn take_outcome(&self, my_epoch: usize) -> Option<(PathBuf, OpenProbeOutcome)> {
        self.generation.take(my_epoch).flatten()
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
            book_path::locate(&probe.id),
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
        let old = controller.generation.begin();
        old.deliver(|slot| {
            *slot = Some((
                PathBuf::from("/old"),
                OpenProbeOutcome::Failed {
                    error: "old".into(),
                    path_exists: false,
                },
            ));
        });

        let current = controller.generation.begin();
        assert!(controller.take_outcome(old.id()).is_none());

        current.deliver(|slot| {
            *slot = Some((
                PathBuf::from("/new"),
                OpenProbeOutcome::Failed {
                    error: "new".into(),
                    path_exists: true,
                },
            ));
        });
        let (path, outcome) = controller
            .take_outcome(current.id())
            .expect("current outcome");
        assert_eq!(path, PathBuf::from("/new"));
        assert!(matches!(
            outcome,
            OpenProbeOutcome::Failed {
                path_exists: true,
                ..
            }
        ));
        assert!(controller.take_outcome(current.id()).is_none());
    }
}
