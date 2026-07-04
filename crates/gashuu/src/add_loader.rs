//! Off-UI-thread bulk-add controller (issue 206).
//!
//! Bulk-adding sources used to freeze the event loop: `add_paths` opened every
//! picked archive synchronously on the UI thread (`probe_page_count_with_policy`
//! reads each ZIP's central directory; on cloud-synced volumes `File::open`
//! blocks on hydration), so the window was unresponsive until the whole batch
//! was probed, saved, and the carousel rebuilt.
//!
//! This controller moves the per-source PROBE onto rayon workers — the same
//! async harness covers (`cover_loader.rs`) and thumbnails (`thumbnail_strip.rs`)
//! already use — while keeping the strict reject-before-add semantics and the
//! exact "added N / skipped M" notice. The add still happens as ONE batch once
//! probing finishes (the confirmed UX: books appear together, not optimistically).
//!
//! Split:
//! - the PROBE half ([`probe_path`]) is pure and `Send`: `(index, PathBuf,
//!   ArchivePolicy)` → [`ProbeOutcome`]. It touches NO `Library`.
//! - the APPLY half lives on the UI thread (`crate::add_books::apply_outcomes` +
//!   `crate::carousel_refresh::apply_add_report`): it mutates the `!Send` `Rc<RefCell<Library>>`,
//!   saves, rebuilds the carousel, and shows the notice — driven by the
//!   `add-finalize` Slint callback wired in `handlers/library.rs`.
//!
//! Thread-boundary rule (identical to the cover loader): only `Send` values
//! cross into the rayon jobs and the event-loop closures — the `slint::Weak`,
//! the epoch `Arc`, the `Arc<Mutex<Vec<ProbeOutcome>>>` accumulator, the
//! `Arc<AtomicUsize>` remaining counter, the `PathBuf`, the `ArchivePolicy`
//! (`Copy`), and `usize`s. The `Rc<RefCell<Library>>` / `Settings` / `VecModel`
//! (all `!Send`) are NEVER moved: every `Library` mutation runs in the
//! `add-finalize` handler on the UI thread.
//!
//! Supersede guard (same shape as the cover loader's epoch): each `start` bumps
//! an `AtomicUsize` epoch and installs a fresh accumulator. A late `add-progress`
//! / `add-finalize` whose captured `my_epoch` mismatches the current epoch is
//! dropped, so a second add started mid-probe cleanly supersedes the first with
//! no stale clobber.

use crate::ui_marshal::marshal_to_ui;
use crate::ViewerWindow;
use gashuu_core::{ArchiveLoader, ArchivePolicy, CoreError};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// How one probed source classified — the `Send` result of opening it on a
/// worker, carrying just enough to run the UI-thread apply half. Mirrors the
/// four arms of the old synchronous `add_paths` match: a real book with its page
/// count, an empty source, a policy-disabled format, or an unreadable source.
/// `FormatDisabled`/`Unreadable` keep the log detail (the format tag / error
/// text) so the apply half logs exactly what the synchronous path did.
pub(crate) enum ProbeKind {
    /// Opened cleanly with `n` image pages — add it and record the count.
    Counted(NonZeroUsize),
    /// Opened cleanly but zero image pages (the empty-book rule) — skip.
    Empty,
    /// A RAR/CBR rejected because `ArchivePolicy::allow_rar` is false — skip.
    FormatDisabled { format: &'static str },
    /// Could not be opened at all (I/O error, unsupported format) — skip.
    Unreadable { error: String },
}

/// One probed source awaiting the UI-thread apply: its INPUT-order `index` (so
/// the apply half can restore input order regardless of probe completion order),
/// its canonical-on-add `path`, and its classified `kind`. All fields `Send`.
pub(crate) struct ProbeOutcome {
    /// Position in the picked-paths input, used to re-sort to input order before
    /// applying (probes complete out of order on parallel workers).
    pub index: usize,
    pub path: PathBuf,
    pub kind: ProbeKind,
}

/// Probe ONE source off the UI thread: classify it without touching the
/// `Library`. Pure and `Send` (the worker payload), so the classification is
/// unit-testable without a Slint event loop. Mirrors the old `add_paths` probe
/// arm exactly — `Ok` → `Counted`, `EmptyBook` → `Empty`, `FormatDisabled` →
/// `FormatDisabled`, any other error → `Unreadable` — but logging is deferred to
/// the apply half (which owns the same path + kind on the UI thread).
pub(crate) fn probe_path(index: usize, path: PathBuf, policy: ArchivePolicy) -> ProbeOutcome {
    let kind = match ArchiveLoader::probe_page_count_with_policy(&path, policy) {
        Ok(count) => ProbeKind::Counted(count),
        Err(CoreError::EmptyBook { .. }) => ProbeKind::Empty,
        Err(CoreError::FormatDisabled { format }) => ProbeKind::FormatDisabled { format },
        Err(e) => ProbeKind::Unreadable {
            error: e.to_string(),
        },
    };
    ProbeOutcome { index, path, kind }
}

/// Completed fraction of a bulk add for the progress bar fill, in `[0.0, 1.0]`.
/// Returns `0.0` for an empty batch (no divide-by-zero) and clamps so a stray
/// over-count never overfills the bar. Pure so the mapping is unit-testable
/// without a Slint event loop.
pub(crate) fn add_progress_ratio(done: usize, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (done as f32 / total as f32).clamp(0.0, 1.0)
}

/// Owns the bulk-add async bookkeeping: the supersede `epoch` and the current
/// generation's probe-outcome accumulator. Like `CoverController`, it does NOT
/// own any `!Send` `Rc` collaborator — the apply half re-acquires those in the
/// `add-finalize` handler on the UI thread. Held in an `Rc` so the add handlers
/// (`on_add_books` / `on_add_folder`) and the finalize handler share one.
pub(crate) struct AddController {
    epoch: Arc<AtomicUsize>,
    /// The CURRENT generation's accumulator. Replaced by a fresh `Arc` on every
    /// `start` (a superseded generation's workers keep pushing into their own
    /// now-orphaned `Arc`, harmlessly). `RefCell` because the controller lives on
    /// the UI thread; the inner `Arc<Mutex<_>>` is what crosses into the workers.
    outcomes: RefCell<Arc<Mutex<Vec<ProbeOutcome>>>>,
    /// The op tag ("add-books" / "add-folder") of the current generation, used
    /// only in the apply half's save-failure trace. Replaced on every `start`.
    op: RefCell<&'static str>,
}

impl AddController {
    /// Build the controller. Call once during UI setup.
    pub(crate) fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            outcomes: RefCell::new(Arc::new(Mutex::new(Vec::new()))),
            op: RefCell::new("add"),
        }
    }

    /// Start probing `paths` for a bulk add off the UI thread. Bumps the epoch,
    /// installs a fresh accumulator, shows the initial `Adding… (0/N)` status,
    /// then dispatches one rayon probe per path. Each worker pushes its
    /// [`ProbeOutcome`], marshals an epoch-guarded progress tick, and the worker
    /// that drains the remaining counter to zero marshals `add-finalize`.
    ///
    /// Called on the UI thread from the add handlers. The empty-`paths` case is
    /// handled inline (no workers): finalize is invoked directly so the apply
    /// half still runs (yielding the "already in library" notice), matching the
    /// old synchronous `add_paths(vec![])`.
    pub(crate) fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        paths: Vec<PathBuf>,
        policy: ArchivePolicy,
        op: &'static str,
    ) {
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;
        *self.op.borrow_mut() = op;
        let total = paths.len();

        // Fresh accumulator for this generation; supersede the previous one.
        let acc = Arc::new(Mutex::new(Vec::with_capacity(total)));
        *self.outcomes.borrow_mut() = Arc::clone(&acc);

        // We are on the UI thread, so drive the first UI write directly.
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if total == 0 {
            // No sources to probe: run the apply half immediately so the
            // "already in library" notice still fires (parity with add_paths([])).
            ui.invoke_add_finalize(my_epoch as i32);
            return;
        }
        ui.invoke_add_progress(0, total as i32);

        let remaining = Arc::new(AtomicUsize::new(total));
        for (index, path) in paths.into_iter().enumerate() {
            let acc = Arc::clone(&acc);
            let remaining = Arc::clone(&remaining);
            let epoch = Arc::clone(&self.epoch);
            let weak = ui_weak.clone();
            rayon::spawn(move || {
                let outcome = probe_path(index, path, policy);
                acc.lock()
                    .expect("add outcomes mutex poisoned")
                    .push(outcome);
                // fetch_sub returns the PRE-decrement value; `left` is what
                // remains after this worker.
                let left = remaining.fetch_sub(1, Relaxed) - 1;
                let done = total - left;
                marshal_progress(weak.clone(), Arc::clone(&epoch), my_epoch, done, total);
                if left == 0 {
                    marshal_finalize(weak, epoch, my_epoch);
                }
            });
        }
    }

    /// Drain this generation's probe outcomes for the apply half, in INPUT
    /// order, IFF `my_epoch` is still current. Returns `None` for a superseded
    /// generation (a second add started since this batch was dispatched), so the
    /// finalize handler drops a stale batch instead of clobbering the live one.
    /// The `op` tag is returned alongside for the save-failure trace.
    pub(crate) fn take_outcomes(
        &self,
        my_epoch: usize,
    ) -> Option<(Vec<ProbeOutcome>, &'static str)> {
        if self.epoch.load(Relaxed) != my_epoch {
            return None;
        }
        let mut outcomes = {
            let acc = self.outcomes.borrow();
            let mut guard = acc.lock().expect("add outcomes mutex poisoned");
            std::mem::take(&mut *guard)
        };
        // Probes complete out of order on parallel workers; restore input order so
        // the apply half adds books and reports skips deterministically.
        outcomes.sort_by_key(|o| o.index);
        Some((outcomes, *self.op.borrow()))
    }
}

/// Marshal an epoch-guarded progress tick onto the UI thread: the `add-progress`
/// callback fires with `(done, total)` so the handler can show the transient
/// "Adding… (k/N)" status. A tick from a superseded generation (epoch moved) is
/// dropped rather than flashing a stale count. Mirrors `cover_loader`'s
/// `marshal_*` helpers — only `Send` values cross the boundary.
fn marshal_progress(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    done: usize,
    total: usize,
) {
    marshal_to_ui(weak, epoch, my_epoch, "add-progress", move |ui| {
        ui.invoke_add_progress(done as i32, total as i32);
    });
}

/// Marshal the final `add-finalize` onto the UI thread once the last probe
/// completes: the handler drains the outcomes (epoch-guarded inside
/// `take_outcomes`) and runs the apply half. The epoch is carried through so a
/// superseded batch's finalize is a no-op. Only `Send` values cross.
fn marshal_finalize(weak: slint::Weak<ViewerWindow>, epoch: Arc<AtomicUsize>, my_epoch: usize) {
    marshal_to_ui(weak, epoch, my_epoch, "add-finalize", move |ui| {
        ui.invoke_add_finalize(my_epoch as i32);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a temp directory `parent/<name>` holding `pages` zero-byte `*.png`
    /// files (so it probes to a `pages`-page book; `pages == 0` probes to
    /// `Empty`). Listing is extension-based, so empty files count as pages.
    fn make_book_dir(parent: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).expect("create book dir");
        for i in 0..pages {
            std::fs::write(dir.join(format!("page{i:03}.png")), []).expect("write page");
        }
        dir
    }

    /// A folder with image pages probes to `Counted(n)`, carrying the input index
    /// and the path unchanged (canonicalization happens later, in the apply half).
    #[test]
    fn probe_path_counts_a_real_book() {
        let root = tempfile::tempdir().expect("tempdir");
        let three = make_book_dir(root.path(), "three", 3);
        let outcome = probe_path(7, three.clone(), ArchivePolicy::default());
        assert_eq!(outcome.index, 7);
        assert_eq!(outcome.path, three);
        match outcome.kind {
            ProbeKind::Counted(n) => assert_eq!(n.get(), 3),
            _ => panic!("expected Counted, got a skip"),
        }
    }

    /// An empty folder opens cleanly but has zero pages → `Empty` (the
    /// empty-book rule), never `Unreadable`.
    #[test]
    fn probe_path_classifies_empty_source() {
        let root = tempfile::tempdir().expect("tempdir");
        let empty = make_book_dir(root.path(), "empty", 0);
        let outcome = probe_path(0, empty, ArchivePolicy::default());
        assert!(matches!(outcome.kind, ProbeKind::Empty));
    }

    /// A nonexistent path cannot be opened → `Unreadable` (distinct from empty),
    /// carrying a non-empty error string for the apply half's log.
    #[test]
    fn probe_path_classifies_unreadable_source() {
        let outcome = probe_path(
            0,
            std::path::PathBuf::from("/nonexistent_gashuu_probe_path_unreadable"),
            ArchivePolicy::default(),
        );
        match outcome.kind {
            ProbeKind::Unreadable { error } => assert!(!error.is_empty()),
            _ => panic!("a missing path must classify as Unreadable"),
        }
    }

    /// A `.cbr` is rejected at probe time when `allow_rar` is false →
    /// `FormatDisabled`, carrying the format tag for the log. The extension check
    /// fires before any bytes are read, so any file content works.
    #[test]
    fn probe_path_classifies_format_disabled() {
        let root = tempfile::tempdir().expect("tempdir");
        let cbr = root.path().join("manga.cbr");
        std::fs::write(&cbr, b"dummy").expect("write dummy cbr");
        let outcome = probe_path(0, cbr, ArchivePolicy { allow_rar: false });
        assert!(matches!(outcome.kind, ProbeKind::FormatDisabled { .. }));
    }

    /// Push an `Empty` outcome carrying `index` into the controller's CURRENT
    /// accumulator, simulating one worker's completion (without a Slint event
    /// loop). Same-module access to the private accumulator is the point — it
    /// lets us exercise the input-order restoration in isolation.
    fn push_outcome(ctrl: &AddController, index: usize) {
        let acc = ctrl.outcomes.borrow();
        acc.lock().expect("mutex").push(ProbeOutcome {
            index,
            path: PathBuf::from(format!("/p{index}")),
            kind: ProbeKind::Empty,
        });
    }

    /// The PARALLEL concern: probes complete out of order on the workers, but
    /// `take_outcomes` restores INPUT order before the apply half runs. Pushing
    /// indices 2, 0, 1 (completion order) must drain as 0, 1, 2 (input order).
    /// Verified without any wall-clock dependency.
    #[test]
    fn take_outcomes_restores_input_order() {
        let ctrl = AddController::new();
        for index in [2usize, 0, 1] {
            push_outcome(&ctrl, index);
        }
        // The epoch is still its initial value (no `start` bumped it), so the
        // drain is accepted and the outcomes come back in input order.
        let (outcomes, _op) = ctrl.take_outcomes(0).expect("current generation drains");
        let order: Vec<usize> = outcomes.iter().map(|o| o.index).collect();
        assert_eq!(order, vec![0, 1, 2], "outcomes restored to input order");
        // A second drain of the same generation yields nothing (already taken).
        let (again, _) = ctrl.take_outcomes(0).expect("still current");
        assert!(again.is_empty(), "outcomes drained exactly once");
    }

    /// The SUPERSEDE guard: a finalize from an older generation (its `my_epoch`
    /// no longer matches the bumped epoch) drains NOTHING, so a second add started
    /// mid-probe cannot be clobbered by the first's stale finalize. Verified via
    /// the generation counter, not timing.
    #[test]
    fn take_outcomes_drops_superseded_generation() {
        let ctrl = AddController::new();
        push_outcome(&ctrl, 0);
        // A newer generation advanced the epoch since this batch (my_epoch = 0)
        // was dispatched.
        ctrl.epoch.store(5, Relaxed);
        assert!(
            ctrl.take_outcomes(0).is_none(),
            "a superseded generation's finalize drains nothing"
        );
    }

    /// The bar fill fraction is done/total, with a zero-guard for the empty
    /// batch and a clamp so an over-count can never exceed a full bar.
    #[test]
    fn add_progress_ratio_maps_counts_to_fraction() {
        assert_eq!(add_progress_ratio(0, 18), 0.0, "start of batch is empty");
        assert_eq!(add_progress_ratio(9, 18), 0.5, "half done is half full");
        assert_eq!(add_progress_ratio(18, 18), 1.0, "all done is full");
        assert_eq!(add_progress_ratio(0, 0), 0.0, "empty batch never divides by zero");
        assert_eq!(add_progress_ratio(20, 18), 1.0, "over-count clamps to full");
    }
}
