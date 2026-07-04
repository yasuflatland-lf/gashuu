//! Page-count prefetch concern for the library carousel.
//!
//! A book whose page count is unknown (never opened, nothing persisted) is
//! resolved in the background by `cover_loader`'s worker — the SAME archive open
//! that produces the cover also yields the count, so no extra `ArchiveLoader::open`
//! is added here. The worker streams the resolved total to the row for immediate
//! display (`marshal_total`) and queues it for UI-thread persistence into the
//! `Library` (the worker cannot touch the `!Send` `Rc<RefCell<Library>>`, so it
//! pushes a `ResolvedCount` onto the `Send` `Arc<Mutex>` queue owned by
//! [`PageCountPrefetch`]). The UI thread drains and applies the queue via
//! [`PageCountPrefetch::apply`] at the next `start` and at shutdown
//! ([`PageCountPrefetch::flush`]).

use crate::carousel::update_carousel_row;
use crate::library_model::clamp_to_i32;
use crate::ui_marshal::marshal_to_ui;
use crate::ViewerWindow;
use gashuu_core::Library;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// One page count a background worker resolved for a book, awaiting UI-thread
/// persistence into the `Library`. Named (rather than a bare `(PathBuf,
/// NonZeroUsize)` tuple) so the queue's element reads as what it is: a resolved
/// fact being carried home, not anonymous data.
pub(crate) struct ResolvedCount {
    /// The book's canonical path — the `Library::set_page_count` lookup key.
    path: PathBuf,
    count: NonZeroUsize,
}

/// Set the displayed `total` of carousel row `row`, on the UI thread. The cover
/// counterpart of `set_cover`: same `!Send`-`VecModel`-via-`ui` re-fetch and same
/// row-bounds check (tolerating a model that shrank since the request was built),
/// swapping only the row's `total` so the focused-book counter reads "1 / N"
/// instead of "1 / 0" the moment the background count resolves. `progress` is left
/// untouched — an unread book is 0 % regardless of its (now known) total.
fn set_carousel_total(ui: &ViewerWindow, row: usize, total: i32) {
    update_carousel_row(ui, row, |item| item.total = total);
}

/// Marshal a resolved page `count` onto the UI thread as carousel row `row`'s
/// `total`. The count counterpart of `marshal_cover`: same epoch-guard + upgrade
/// preamble, so a count from a superseded generation (library refreshed since) is
/// dropped rather than flashing a stale total. `count` is the raw `usize` page
/// count (a `Send` `Copy` value); the saturating `i32` map happens here.
fn marshal_total(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    count: usize,
) {
    let total = clamp_to_i32(count);
    marshal_to_ui(weak, epoch, my_epoch, "total", move |ui| {
        set_carousel_total(ui, row, total);
    });
}

/// Worker-side: record a resolved page `count` for `path`/`row`. Queues `(path,
/// count)` for UI-thread persistence (a zero-page archive yields `count == 0`,
/// which `NonZeroUsize::new` maps to `None` so nothing is queued or persisted) and
/// marshals the total to the row for immediate display. Runs on a rayon worker, so
/// it only touches `Send` state (the `Arc<Mutex>` queue, the `Weak`, the epoch).
pub(crate) fn push_and_marshal_count(
    pending: &Arc<Mutex<Vec<ResolvedCount>>>,
    weak: &slint::Weak<ViewerWindow>,
    epoch: &Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    path: PathBuf,
    count: usize,
) {
    // Persist only while OUR generation is current: a stale push can't lose a count
    // (the fresh generation re-dispatches), it only clobbers a good one. Mirrors `marshal_total`.
    if epoch.load(Relaxed) == my_epoch {
        if let Some(nz) = NonZeroUsize::new(count) {
            pending
                .lock()
                .expect("cover pending_counts mutex poisoned")
                .push(ResolvedCount { path, count: nz });
        }
    }
    marshal_total(weak.clone(), Arc::clone(epoch), my_epoch, row, count);
}

/// Owns the page-count prefetch pending queue and its UI-thread lifecycle. A
/// background worker resolves a book's page count from the same archive open that
/// produced its cover and pushes it here; the UI thread drains and persists.
pub(crate) struct PageCountPrefetch {
    /// Page counts resolved by background workers, awaiting persistence on the
    /// UI thread. A worker cannot touch the `!Send` `Rc<RefCell<Library>>`, so it
    /// pushes a `ResolvedCount` here (the `Arc<Mutex>` is `Send`); the UI thread
    /// drains and applies them via `apply` at the next `start` and at shutdown
    /// (`flush`).
    pending: Arc<Mutex<Vec<ResolvedCount>>>,
}

impl PageCountPrefetch {
    pub(crate) fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A `Send` handle to the pending queue for a worker to push resolved counts into.
    pub(crate) fn queue(&self) -> Arc<Mutex<Vec<ResolvedCount>>> {
        Arc::clone(&self.pending)
    }

    /// Persist page counts resolved by background workers since the last call, on
    /// the UI thread (a worker cannot touch the `!Send` `Rc<RefCell<Library>>`).
    /// Drains the pending queue, applies each via `Library::set_page_count`, and
    /// saves ONCE if any changed. Borrow discipline: each `borrow_mut` is confined
    /// to its own statement (dropped at the `;`); the final `borrow` for `save` is
    /// a separate statement — collapsing them would double-borrow-panic.
    pub(crate) fn apply(&self, library: &Rc<RefCell<Library>>) {
        let drained: Vec<ResolvedCount> = {
            let mut queue = self
                .pending
                .lock()
                .expect("cover pending_counts mutex poisoned");
            std::mem::take(&mut *queue)
        };
        let mut changed = false;
        for resolved in &drained {
            if library
                .borrow_mut()
                .set_page_count(&resolved.path, resolved.count)
            {
                changed = true;
            }
        }
        if changed {
            if let Err(e) = library.borrow().save() {
                tracing::error!(error = %e, "cover: failed to save library after page-count prefetch");
            }
        }
    }

    /// Persist any still-pending prefetched page counts. Call once on shutdown.
    pub(crate) fn flush(&self, library: &Rc<RefCell<Library>>) {
        self.apply(library);
    }
}
