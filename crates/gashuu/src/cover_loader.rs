//! Book-cover loading controller for the library carousel.
//!
//! Each book's cover is the thumbnail of its page 0 (see core `generate_cover`).
//! `start` is DISPATCH-ONLY on the UI thread: every request becomes one
//! fire-and-forget rayon job, and the worker does ALL the per-book I/O — derive
//! `cache_key(path, mtime_secs, max_side)`, try the disk cache (a hit reads and
//! decodes the cached PNG on the worker), or on a miss open the source via
//! `ArchiveLoader`, call `generate_cover`, and store the result with
//! `ThumbnailCache::put`. Either way the cover is marshalled back to the row via
//! `invoke_from_event_loop`. Hit and miss share this single worker path so a
//! large library can never freeze the event loop (a 500-book warm start used to
//! decode 500 cached PNGs inline on the UI thread).
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

/// Reorder cover requests so the row closest to `focus_row` is dispatched
/// first, expanding outward (focus, focus±1, focus±2, …). Workers are picked up
/// roughly in dispatch order, so on a large library the covers around the
/// focused book stream in first instead of waiting behind hundreds of off-screen
/// rows. A distance tie (focus−d vs focus+d) keeps the input's ascending-row
/// order (stable sort); a stale `focus_row` beyond the last row is harmless
/// (`abs_diff` never panics — the nearest rows still sort first).
pub(crate) fn prioritize_by_focus(
    mut requests: Vec<CoverRequest>,
    focus_row: usize,
) -> Vec<CoverRequest> {
    requests.sort_by_key(|r| r.row.abs_diff(focus_row));
    requests
}

/// One page count a background worker resolved for a book, awaiting UI-thread
/// persistence into the `Library`. Named (rather than a bare `(PathBuf,
/// NonZeroUsize)` tuple) so the queue's element reads as what it is: a resolved
/// fact being carried home, not anonymous data.
struct ResolvedCount {
    /// The book's canonical path — the `Library::set_page_count` lookup key.
    path: PathBuf,
    count: NonZeroUsize,
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
    /// pushes a [`ResolvedCount`] here (the `Arc<Mutex>` is `Send`); the UI
    /// thread drains and applies them via `apply_pending_counts` at the next
    /// `start` and at shutdown (`flush_counts`). The live carousel `total` is
    /// updated separately and immediately by `marshal_total` (display vs persist
    /// are independent — the queue is only the persistence bridge).
    pending_counts: Arc<Mutex<Vec<ResolvedCount>>>,
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

/// Marshal one cover image onto the UI thread for carousel row `row`: the
/// event-loop closure applies the epoch-guard + upgrade preamble, then writes
/// the row via `set_cover`. The `slint::Image` is built INSIDE this closure
/// (it is `!Send`); the captured `img` is the Send `DecodedImage`.
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
    pending: &Arc<Mutex<Vec<ResolvedCount>>>,
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
                .push(ResolvedCount { path, count: nz });
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
        let drained: Vec<ResolvedCount> = {
            let mut queue = self
                .pending_counts
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

    /// Persist any still-pending prefetched page counts. Call once on shutdown
    /// (after the event loop ends) so counts resolved after the last `start`
    /// survive a restart instead of being recomputed by re-opening every archive.
    pub fn flush_counts(&self, library: &Rc<RefCell<Library>>) {
        self.apply_pending_counts(library);
    }

    /// Load one cover on a rayon worker — the SINGLE per-book path for cache hit
    /// and miss alike, so no per-book I/O (mtime `fs::metadata`, cached-PNG read +
    /// decode, archive open, page-0 decode) ever runs on the UI thread.
    ///
    /// Worker flow: derive `cache_key` (reads the mtime HERE, not at dispatch) →
    /// try the disk cache. HIT → marshal the decoded cover to the row, then, when
    /// the count is still unknown, resolve it with one archive open. MISS → open
    /// the source, capture the count from the same open (when needed), generate +
    /// store the cover, then marshal count and cover. Cancel is polled before each
    /// heavy step and re-checked before crossing to the UI thread; every crossing
    /// goes through the epoch-guarded marshal helpers. Capture rule: only `Send`
    /// values enter the closure.
    fn spawn_load(
        &self,
        ui_weak: &slint::Weak<ViewerWindow>,
        cancel_flag: &Arc<AtomicBool>,
        my_epoch: usize,
        req: CoverRequest,
    ) {
        let weak = ui_weak.clone();
        let epoch = Arc::clone(&self.epoch);
        let cancel = Arc::clone(cancel_flag);
        let pending = Arc::clone(&self.pending_counts);
        rayon::spawn(move || {
            // Bail before the heavy work if a newer generation superseded us.
            if cancel.load(Relaxed) {
                return;
            }
            // Reconstruct the cache on this thread (ThumbnailCache is not Clone
            // and holds only a dir, so this is cheap and side-steps a !Send/!Clone
            // capture). `start` already logged the no-cache-dir case once.
            let Ok(cache) = ThumbnailCache::new() else {
                return;
            };
            let key = cache_key(&req.path, mtime_secs(&req.path), COVER_MAX_SIDE);
            // HIT: the decode is already done on disk; read + decode it on THIS
            // worker and stream it to the row first (the count open below may be
            // slow — the user should not wait on it to see the cover).
            if let Some(decoded) = cache.get(&key) {
                marshal_cover(weak.clone(), Arc::clone(&epoch), my_epoch, req.row, decoded);
                // Cover is cached but the page count may still be unknown —
                // resolve it with one archive open so the row shows "1 / N".
                if req.needs_count && !cancel.load(Relaxed) {
                    match ArchiveLoader::open(&req.path) {
                        Ok(source) => {
                            let count = source.list_pages().len();
                            // Post-open cancel re-check before crossing to the UI thread.
                            if cancel.load(Relaxed) {
                                return;
                            }
                            push_and_marshal_count(
                                &pending, &weak, &epoch, my_epoch, req.row, req.path, count,
                            );
                        }
                        Err(e) => {
                            tracing::warn!(path = %req.path.display(), error = %e, "cover: count open failed");
                        }
                    }
                }
                return;
            }
            // MISS: open the source for THIS book. A missing/unsupported file
            // leaves the row a placeholder — log and return, never panic.
            let source = match ArchiveLoader::open(&req.path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %req.path.display(), error = %e, "cover: open failed");
                    return;
                }
            };
            // The same open yields the page count for free; capture it BEFORE
            // `source` moves into generate_cover (only when the row needs it).
            let count = req.needs_count.then(|| source.list_pages().len());
            let decoded = match generate_cover(source, COVER_MAX_SIDE) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(path = %req.path.display(), error = %e, "cover: generate failed");
                    return;
                }
            };
            // Store to the disk cache so the next launch is a hit. A cache write
            // failure is non-fatal (we already have the image to show).
            if let Err(e) = cache.put(&key, &decoded) {
                tracing::warn!(path = %req.path.display(), error = %e, "cover: cache put failed");
            }
            // Post-generate cancel re-check before crossing to the UI thread
            // (mirrors generate_thumbnails' second cancel poll).
            if cancel.load(Relaxed) {
                return;
            }
            // Stream the resolved count to the row and queue it for persistence.
            if let Some(c) = count {
                push_and_marshal_count(&pending, &weak, &epoch, my_epoch, req.row, req.path, c);
            }
            marshal_cover(weak, epoch, my_epoch, req.row, decoded);
        });
    }

    /// (Re)load covers for the given books. First persists any page counts resolved
    /// since the previous generation (UI thread), then cancels any in-flight
    /// generation, bumps the epoch, and dispatches EVERY request to a rayon worker
    /// (`spawn_load` — hit and miss share one off-thread path; results stream back
    /// via `invoke_from_event_loop`). The UI-thread work here is the count
    /// persistence, a cheap cache-dir probe, and the dispatch loop — no per-book
    /// I/O, regardless of cache state or library size. Call after the carousel
    /// model is built or refreshed (initial library load + every add/remove). The
    /// caller MUST build `requests` BEFORE the call (releasing its own
    /// `library.borrow()`) so the `apply_pending_counts` `borrow_mut` here cannot
    /// double-borrow.
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

        // Probe the disk cache once so the no-cache-dir degraded state (covers
        // stay placeholders) is logged ONCE here, not by every worker. Workers
        // rebuild their own handle — it only holds a directory path.
        if let Err(e) = ThumbnailCache::new() {
            tracing::warn!(error = %e, "no cover cache available; covers stay placeholders");
            return;
        }

        // Window already gone (shutdown race): don't queue useless workers.
        if ui_weak.upgrade().is_none() {
            return;
        }

        // 3. Dispatch. The elapsed log pins the UI-thread cost of `start` itself
        // (dispatch only — all per-book I/O happens on the workers).
        let started = std::time::Instant::now();
        let dispatched = requests.len();
        for req in requests {
            self.spawn_load(&ui_weak, &cancel_flag, my_epoch, req);
        }
        tracing::debug!(
            dispatched,
            elapsed_us = started.elapsed().as_micros() as u64,
            "cover loading dispatched"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(row: usize) -> CoverRequest {
        CoverRequest {
            row,
            path: PathBuf::from(format!("/manga/{row}.cbz")),
            needs_count: false,
        }
    }

    fn rows(requests: &[CoverRequest]) -> Vec<usize> {
        requests.iter().map(|r| r.row).collect()
    }

    /// Rows closest to the focused row come first, expanding outward; the
    /// distance tie (focus−d vs focus+d) keeps the input's ascending order
    /// (stable sort), so the lower row precedes the higher one.
    #[test]
    fn prioritize_orders_by_distance_from_focus() {
        let ordered = prioritize_by_focus((0..5).map(req).collect(), 2);
        assert_eq!(rows(&ordered), vec![2, 1, 3, 0, 4]);
    }

    /// Focus on row 0 (the reset-focus case) keeps the natural ascending order —
    /// distance from 0 IS the row index.
    #[test]
    fn prioritize_focus_zero_is_identity() {
        let ordered = prioritize_by_focus((0..4).map(req).collect(), 0);
        assert_eq!(rows(&ordered), vec![0, 1, 2, 3]);
    }

    /// A focus beyond the last row (stale index after the model shrank) must not
    /// panic; the nearest (= highest) rows simply come first.
    #[test]
    fn prioritize_focus_beyond_last_row_orders_descending() {
        let ordered = prioritize_by_focus((0..3).map(req).collect(), 10);
        assert_eq!(rows(&ordered), vec![2, 1, 0]);
    }

    /// An empty request list stays empty (no panic on the degenerate input).
    #[test]
    fn prioritize_empty_is_empty() {
        assert!(prioritize_by_focus(Vec::new(), 5).is_empty());
    }
}
