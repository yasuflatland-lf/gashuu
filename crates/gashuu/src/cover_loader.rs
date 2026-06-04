//! Book-cover loading controller for the library carousel.
//!
//! Each book's cover is the thumbnail of its page 0 (see core `generate_cover`).
//! For every book in the carousel this controller derives `cache_key(path,
//! mtime_secs, max_side)`, tries the disk cache, and on a hit sets the row's
//! `cover` immediately (on the UI thread). On a miss it fires a fire-and-forget
//! rayon job that opens the source via `ArchiveLoader`, calls `generate_cover`,
//! stores the result with `ThumbnailCache::put`, then marshals the cover back to
//! the row via `invoke_from_event_loop`.
//!
//! Correctness across library refreshes is the SAME epoch + cancel double-guard
//! as `ThumbnailController` (see `thumbnail_strip.rs`): `start` bumps an
//! `AtomicUsize` epoch (so a late `invoke_from_event_loop` whose captured
//! `my_epoch` mismatches the current epoch is dropped) AND flips the previous
//! generation's `AtomicBool` cancel flag (so a worker that has not yet started
//! its heavy open+decode stops promptly). Either guard alone is insufficient.
//!
//! Thread-boundary rule (identical to the thumbnail strip): only `Send` values
//! cross into the rayon job and the event-loop closure — `slint::Weak` (Send+Sync),
//! the epoch and cancel `Arc`s, the row `usize`, `my_epoch`, the cache-key
//! `String`, the `PathBuf`, and the resulting `DecodedImage`. The `Rc` `VecModel`
//! and `slint::Image` (both `!Send`) are NEVER moved: the model is re-fetched and
//! the image built INSIDE the event-loop closure.

use crate::library_model::clamp_to_i32;
use crate::to_slint_image;
use crate::{CarouselItem, ViewerWindow};
use gashuu_core::{
    cache_key, generate_cover, ArchiveLoader, DecodedImage, Library, ThumbnailCache,
};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Longer-edge size, in pixels, for generated carousel covers. Decoupled from the
/// page strip's `DEFAULT_THUMB_MAX_SIDE` (160 px): a focused cover slot is up to
/// 240×336 logical px, which is 480×672 PHYSICAL px on a 2× (Retina) display, so a
/// 160 px buffer was upscaled ~4× and looked blurry. 512 px keeps covers sharp at
/// 1× and near-sharp at 2× while staying a single cover per book in the cache.
/// `max_side` is part of the cache key, so raising it transparently invalidates
/// and regenerates every stale 160 px cover on the next run.
///
/// `pub(crate)` so the bulk-removal purge in `app.rs` passes the SAME single
/// persistent cover side to `ThumbnailCache::purge_for`, instead of duplicating
/// the literal — strip thumbnails are RAM-only and never persisted, so 512 is the
/// only on-disk cover variant a removed book can have.
pub(crate) const COVER_MAX_SIDE: u32 = 512;

/// One book the controller must load a cover for: its carousel row index and its
/// canonical filesystem path. Built on the UI thread from the `Library`, then the
/// `PathBuf` (Send) is what crosses into the worker.
pub struct CoverRequest {
    pub row: usize,
    pub path: PathBuf,
    /// The book's page count is unknown (never opened, nothing persisted). When
    /// set, the controller resolves the real total in the background (one archive
    /// open) and streams it to the row's `total`, fixing the "1 / 0" display.
    pub needs_count: bool,
}

/// Owns the carousel-cover generation bookkeeping (epoch + cancel double-guard).
/// It does NOT own the `VecModel` — that is built and bound into the UI by
/// `carousel::build_carousel_model`; this controller re-fetches it through the
/// `Weak<ViewerWindow>` inside each event-loop closure (the model is `!Send`).
pub struct CoverController {
    epoch: Arc<AtomicUsize>,
    cancel: RefCell<Arc<AtomicBool>>,
    /// Page counts resolved by background workers, awaiting persistence on the UI
    /// thread. A worker cannot touch the `!Send` `Rc<RefCell<Library>>`, so it
    /// pushes `(canonical path, count)` here (the `Arc<Mutex>` is `Send`); the UI
    /// thread drains and applies them via `apply_pending_counts` at the next
    /// `start` and at shutdown (`flush_counts`). The live carousel `total` is
    /// updated separately and immediately by `marshal_total` (display vs persist
    /// are independent — the queue is only the persistence bridge).
    pending_counts: Arc<Mutex<Vec<(PathBuf, NonZeroUsize)>>>,
}

impl Default for CoverController {
    fn default() -> Self {
        Self::new()
    }
}

/// Filesystem mtime of `path` as whole seconds since the Unix epoch, or `0` when
/// the file is missing / has no readable mtime (an unavailable book still gets a
/// stable, if degenerate, cache key). This is one of the three `cache_key` inputs
/// so a modified file regenerates its cover automatically (the cache owns hashing).
///
/// `pub(crate)` so the bulk-removal purge in `app.rs` derives the cover's cache
/// key under the book's CURRENT mtime (the same value the cover was generated
/// under, modulo drift) when calling `ThumbnailCache::purge_for`.
pub(crate) fn mtime_secs(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Set the `cover` image of carousel row `row` to `img`, on the UI thread.
/// Re-fetches the `!Send` `VecModel` through `ui` (never moved across threads),
/// reads the existing row, swaps only its `cover`, and writes it back. A
/// row-count bound check tolerates a model that shrank since the request was
/// built (e.g. a book removed between scheduling and delivery).
fn set_cover(ui: &ViewerWindow, row: usize, img: slint::Image) {
    let model = ui.get_carousel_items();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<CarouselItem>>() else {
        return;
    };
    if row < vm.row_count() {
        let mut item = vm.row_data(row).expect("row < row_count checked above");
        item.cover = img;
        vm.set_row_data(row, item);
    }
}

/// Set the displayed `total` of carousel row `row`, on the UI thread. The cover
/// counterpart of `set_cover`: same `!Send`-`VecModel`-via-`ui` re-fetch and same
/// row-bounds check (tolerating a model that shrank since the request was built),
/// swapping only the row's `total` so the focused-book counter reads "1 / N"
/// instead of "1 / 0" the moment the background count resolves. `progress` is left
/// untouched — an unread book is 0 % regardless of its (now known) total.
fn set_carousel_total(ui: &ViewerWindow, row: usize, total: i32) {
    let model = ui.get_carousel_items();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<CarouselItem>>() else {
        return;
    };
    if row < vm.row_count() {
        let mut item = vm.row_data(row).expect("row < row_count checked above");
        item.total = total;
        vm.set_row_data(row, item);
    }
}

/// Marshal one cover image onto the UI thread for carousel row `row`, applying
/// the epoch-guard + upgrade + model-row preamble shared with the worker. The
/// `slint::Image` is built INSIDE this closure (it is `!Send`); the captured
/// `img` is the Send `DecodedImage`.
fn marshal_cover(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    img: DecodedImage,
) {
    if let Err(e) = slint::invoke_from_event_loop(move || {
        // Drop results from a superseded generation (library refreshed since).
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Build the `!Send` slint::Image here, on the UI thread, then write the row.
        set_cover(&ui, row, to_slint_image(&img));
    }) {
        tracing::debug!(row, error = %e, "dropped cover update; event loop gone");
    }
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
    if let Err(e) = slint::invoke_from_event_loop(move || {
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        set_carousel_total(&ui, row, total);
    }) {
        tracing::debug!(row, error = %e, "dropped total update; event loop gone");
    }
}

/// Worker-side: record a resolved page `count` for `path`/`row`. Queues `(path,
/// count)` for UI-thread persistence (a zero-page archive yields `count == 0`,
/// which `NonZeroUsize::new` maps to `None` so nothing is queued or persisted) and
/// marshals the total to the row for immediate display. Runs on a rayon worker, so
/// it only touches `Send` state (the `Arc<Mutex>` queue, the `Weak`, the epoch).
fn push_and_marshal_count(
    pending: &Arc<Mutex<Vec<(PathBuf, NonZeroUsize)>>>,
    weak: &slint::Weak<ViewerWindow>,
    epoch: &Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    path: PathBuf,
    count: usize,
) {
    // Persist only while OUR generation is still current. A superseded generation
    // (library refreshed since dispatch) may have counted a since-changed archive;
    // the fresh generation re-dispatches any book still missing a count, so dropping
    // a stale push here cannot lose a real count — it only avoids overwriting a good
    // one (e.g. a count just back-filled by opening the book). Mirrors the epoch
    // guard inside `marshal_total` (the display side).
    if epoch.load(Relaxed) == my_epoch {
        if let Some(nz) = NonZeroUsize::new(count) {
            pending
                .lock()
                .expect("cover pending_counts mutex poisoned")
                .push((path, nz));
        }
    }
    marshal_total(weak.clone(), Arc::clone(epoch), my_epoch, row, count);
}

impl CoverController {
    /// Build the controller. Call once during UI setup.
    pub fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
            pending_counts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Supersede the previous generation's cancel flag and install a fresh one,
    /// returning a clone of the new flag for the just-started generation. Each
    /// `RefCell` borrow is confined to its own statement (dropped at the `;`) so no
    /// two overlap — collapsing these into one expression would compile but panic
    /// at runtime with a double borrow. Shared shape with `ThumbnailController`.
    fn rotate_cancel(&self) -> Arc<AtomicBool> {
        self.cancel.borrow().store(true, Relaxed);
        *self.cancel.borrow_mut() = Arc::new(AtomicBool::new(false));
        Arc::clone(&self.cancel.borrow())
    }

    /// Persist page counts resolved by background workers since the last call, on
    /// the UI thread (a worker cannot touch the `!Send` `Rc<RefCell<Library>>`).
    /// Drains the pending queue, applies each via `Library::set_page_count`, and
    /// saves ONCE if any changed. Borrow discipline: each `borrow_mut` is confined
    /// to its own statement (dropped at the `;`); the final `borrow` for `save` is
    /// a separate statement — collapsing them would double-borrow-panic.
    fn apply_pending_counts(&self, library: &Rc<RefCell<Library>>) {
        let drained: Vec<(PathBuf, NonZeroUsize)> = {
            let mut queue = self
                .pending_counts
                .lock()
                .expect("cover pending_counts mutex poisoned");
            std::mem::take(&mut *queue)
        };
        let mut changed = false;
        for (path, count) in &drained {
            if library.borrow_mut().set_page_count(path, *count) {
                changed = true;
            }
        }
        if changed {
            if let Err(e) = library.borrow().save() {
                tracing::error!(error = %e, "cover: failed to save library after page-count prefetch");
            }
        }
    }

    /// Persist any still-pending prefetched page counts. Call once on shutdown
    /// (after the event loop ends) so counts resolved after the last `start`
    /// survive a restart instead of being recomputed by re-opening every archive.
    pub fn flush_counts(&self, library: &Rc<RefCell<Library>>) {
        self.apply_pending_counts(library);
    }

    /// Resolve a book's page count on a rayon worker without (re)generating its
    /// cover — used when the cover is a cache HIT but the count is still unknown.
    /// One archive open + `list_pages().len()`, cancel-guarded on both sides, then
    /// queued for persistence and streamed to the row (`push_and_marshal_count`).
    fn spawn_count_only(
        &self,
        ui_weak: &slint::Weak<ViewerWindow>,
        cancel_flag: &Arc<AtomicBool>,
        my_epoch: usize,
        row: usize,
        path: PathBuf,
    ) {
        let weak = ui_weak.clone();
        let epoch = Arc::clone(&self.epoch);
        let cancel = Arc::clone(cancel_flag);
        let pending = Arc::clone(&self.pending_counts);
        rayon::spawn(move || {
            if cancel.load(Relaxed) {
                return;
            }
            let source = match ArchiveLoader::open(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cover: count open failed");
                    return;
                }
            };
            let count = source.list_pages().len();
            // Post-open cancel re-check before crossing to the UI thread.
            if cancel.load(Relaxed) {
                return;
            }
            push_and_marshal_count(&pending, &weak, &epoch, my_epoch, row, path, count);
        });
    }

    /// (Re)load covers for the given books. First persists any page counts resolved
    /// since the previous generation (UI thread), then cancels any in-flight
    /// generation, bumps the epoch, and for each request tries the cache (hit → set
    /// the row now; if its count is still unknown, resolve it in the background) or
    /// fires a rayon worker (miss → the same open yields cover + count). Call after
    /// the carousel model is built or refreshed (initial library load + every
    /// add/remove). The caller MUST build `requests` BEFORE the call (releasing its
    /// own `library.borrow()`) so the `apply_pending_counts` `borrow_mut` here
    /// cannot double-borrow.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        library: &Rc<RefCell<Library>>,
        requests: Vec<CoverRequest>,
    ) {
        // 0. Persist counts the PREVIOUS generation's workers resolved (UI thread).
        self.apply_pending_counts(library);

        // 1. Supersede the previous generation and take this one's fresh cancel flag.
        let cancel_flag = self.rotate_cancel();

        // 2. Tag this generation so superseded callbacks are dropped.
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;

        // Open the disk cache once for this generation. If the OS gave us no cache
        // dir, skip cover loading entirely (covers stay placeholders — a non-fatal
        // degraded state, logged once).
        let cache = match ThumbnailCache::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "no cover cache available; covers stay placeholders");
                return;
            }
        };

        let Some(ui) = ui_weak.upgrade() else {
            return;
        };

        for req in requests {
            let key = cache_key(&req.path, mtime_secs(&req.path), COVER_MAX_SIDE);
            // HIT: decode is already done on disk; set the row now (UI thread).
            if let Some(decoded) = cache.get(&key) {
                set_cover(&ui, req.row, to_slint_image(&decoded));
                // Cover is cached but the page count may still be unknown — resolve
                // it in the background (one archive open) so the row shows "1 / N".
                if req.needs_count {
                    self.spawn_count_only(
                        &ui_weak,
                        &cancel_flag,
                        my_epoch,
                        req.row,
                        req.path.clone(),
                    );
                }
                continue;
            }
            // MISS: generate the cover on a rayon worker, store it, and stream it
            // back. The SAME open also yields the page count when needed. Fire-and-
            // forget (no join handle). Capture only Send values.
            let weak = ui_weak.clone();
            let epoch = Arc::clone(&self.epoch);
            let cancel = Arc::clone(&cancel_flag);
            let pending = Arc::clone(&self.pending_counts);
            let row = req.row;
            let path = req.path.clone();
            let needs_count = req.needs_count;
            rayon::spawn(move || {
                // Bail before the heavy work if a newer generation superseded us.
                if cancel.load(Relaxed) {
                    return;
                }
                // Open the source for THIS book. A missing/unsupported file leaves
                // the row a placeholder — log and return, never panic.
                let source = match ArchiveLoader::open(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cover: open failed");
                        return;
                    }
                };
                // The same open yields the page count for free; capture it BEFORE
                // `source` moves into generate_cover (only when the row needs it).
                let count = needs_count.then(|| source.list_pages().len());
                let decoded = match generate_cover(source, COVER_MAX_SIDE) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cover: generate failed");
                        return;
                    }
                };
                // Store to the disk cache so the next launch is a hit. A cache write
                // failure is non-fatal (we already have the image to show).
                // Reconstruct the cache on this thread (ThumbnailCache is not Clone
                // and holds only a dir, so this is cheap and side-steps a !Send/!Clone
                // capture).
                if let Ok(cache) = ThumbnailCache::new() {
                    if let Err(e) = cache.put(&key, &decoded) {
                        tracing::warn!(path = %path.display(), error = %e, "cover: cache put failed");
                    }
                }
                // Post-generate cancel re-check before crossing to the UI thread
                // (mirrors generate_thumbnails' second cancel poll).
                if cancel.load(Relaxed) {
                    return;
                }
                // Stream the resolved count to the row and queue it for persistence.
                if let Some(c) = count {
                    push_and_marshal_count(&pending, &weak, &epoch, my_epoch, row, path, c);
                }
                marshal_cover(weak, epoch, my_epoch, row, decoded);
            });
        }
    }
}
