//! Thumbnail-strip generation controller (lazy / virtualized).
//!
//! Owns the strip's backing `VecModel` plus the bookkeeping that keeps thumbnail
//! generation correct across book re-opens: an epoch counter that tags each
//! generation (so a superseded book's late callbacks are dropped) and a cancel
//! flag that stops the previous generation's CPU work the instant a new book
//! opens.
//!
//! Generation is **lazy**: `start` paints `N` `Loading` placeholders but decodes
//! nothing eagerly. Pages are decoded only when their cell is at/near the visible
//! viewport — once for the initial leading window, then on every
//! `visible-range-changed` event the strip fires as the user scrolls/resizes. A
//! per-generation `requested` set de-dups so a page is never decoded twice. This
//! makes first-open work O(visible) instead of O(all pages) and reuses W1-D's
//! per-page disk cache (a cached visible page is a fast read). The synchronous
//! decode/downscale of one page lives in core's `generate_one_thumbnail`.
//!
//! Thread-boundary rule: each visible-range batch spawns a `std::thread` that
//! `par_iter`s the batch and calls `generate_one_thumbnail` per page, marshaling
//! results back to the UI thread. The worker may capture only `Send` values — the
//! `slint::Weak` (Send+Sync), the epoch `Arc`, the cancel `Arc`, and the `Arc`
//! source. The `Rc` model and `slint::Image` are NOT `Send`; the model is
//! re-fetched and the image is built inside `invoke_from_event_loop`, i.e. only
//! ever on the UI thread.

use crate::to_slint_image;
use crate::ui_marshal::marshal_to_ui;
use crate::{ThumbnailItem, ViewerWindow};
use gashuu_core::{
    generate_one_thumbnail, CoreError, DecodedImage, PageSource, PageThumbContext, ThumbnailCache,
    DEFAULT_THUMB_MAX_SIDE,
};
use rayon::prelude::*;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Extra cells fetched on each side of the visible window so a small scroll
/// reveals already-decoded thumbnails instead of momentary placeholders.
const VISIBLE_MARGIN: usize = 3;

/// Pages requested on the initial open before any viewport-geometry event has
/// arrived (the strip's `changed` handlers do not fire on the first bind). A
/// modest leading window keeps first-open work O(visible) on a typical display;
/// wider strips and scrolling backfill the rest via `visible-range-changed`.
const INITIAL_VISIBLE_PAGES: usize = 16;

/// Page indices visible in a horizontal strip whose first on-screen content pixel
/// is `first_px` from the content origin, in a viewport `viewport_w` px wide, with
/// cells laid out every `cell_stride` px. Includes `margin` extra cells on each
/// side and clamps the result to `[0, n)`.
///
/// Returns an empty vec when `n == 0` or the geometry is degenerate (a
/// non-positive stride or viewport) so a not-yet-laid-out strip requests nothing.
/// Pure (no I/O, no UI) — unit-tested at the boundaries like core's
/// `prefetch_indices`.
fn visible_pages(
    first_px: f32,
    viewport_w: f32,
    cell_stride: f32,
    n: usize,
    margin: usize,
) -> Vec<usize> {
    if n == 0 || cell_stride <= 0.0 || viewport_w <= 0.0 {
        return Vec::new();
    }
    let first_px = first_px.max(0.0);
    let first_cell = (first_px / cell_stride).floor() as isize;
    let last_cell = ((first_px + viewport_w) / cell_stride).ceil() as isize;
    let margin = margin as isize;
    let lo = (first_cell - margin).max(0) as usize;
    let hi = ((last_cell + margin).max(0) as usize).min(n);
    (lo..hi).collect()
}

/// From `pages`, keep only indices within `[0, page_count)` that are not already
/// in `requested`, inserting the kept ones so a later call returns them empty.
/// This is the de-dup gate guaranteeing each page is decoded at most once across
/// repeated/overlapping visible-range events. Pure over its inputs (no UI).
fn take_fresh(requested: &mut HashSet<usize>, page_count: usize, pages: Vec<usize>) -> Vec<usize> {
    pages
        .into_iter()
        .filter(|&p| p < page_count && requested.insert(p))
        .collect()
}

/// The three mutually-exclusive states of a thumbnail cell, collapsing the
/// (loaded, failed) boolean pair that the Slint `ThumbnailItem` exposes into a
/// single sum type. `Loaded` carries a `slint::Image` (NOT Send), so only
/// `ThumbCell::Loaded` may be constructed on the UI thread; `Loading` and
/// `Failed` carry no image and are unconstrained.
enum ThumbCell {
    Loading,
    Loaded(slint::Image),
    Failed,
}

/// Build a `ThumbnailItem` from a page index and its cell state. Centralizes the
/// (image, loaded, failed) triple so the three boolean-pair invariants live in
/// ONE place instead of being re-spelled at each construction site (Slint
/// structs can't express a sum type, so the mapping is enforced here).
fn thumbnail_item(page: usize, cell: ThumbCell) -> ThumbnailItem {
    let (image, loaded, failed) = match cell {
        ThumbCell::Loading => (Default::default(), false, false),
        ThumbCell::Loaded(image) => (image, true, false),
        ThumbCell::Failed => (Default::default(), false, true),
    };
    // The (loaded, failed) pair is mutually exclusive by construction above; this
    // debug_assert guards a future hand-edit to the match arms (see `seq_index`).
    debug_assert!(
        !(loaded && failed),
        "thumbnail cell cannot be both loaded and failed"
    );
    ThumbnailItem {
        image,
        page: page as i32,
        loaded,
        failed,
    }
}

/// Marshal a single thumbnail-cell update onto the UI thread, applying the
/// epoch + upgrade + model-downcast + bounds preamble shared by the success and
/// failure paths. `make_cell` is evaluated INSIDE the event-loop closure (i.e.
/// on the UI thread), so a `!Send` value such as `slint::Image` may be built
/// there even though it could not cross the thread boundary as a capture; the
/// closure stays `Send` because only its captures must be `Send` (the `!Send`
/// `ThumbCell::Loaded` it returns does not affect that).
fn marshal_cell(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    i: usize,
    make_cell: impl FnOnce() -> ThumbCell + Send + 'static,
) {
    marshal_to_ui(weak, epoch, my_epoch, "thumbnail", move |ui| {
        // Re-fetch the model on the UI thread (never move the Rc across threads);
        // any non-Send cell payload (e.g. a slint::Image) is built here.
        let model = ui.get_thumbnails();
        let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>() else {
            return;
        };
        if i < vm.row_count() {
            vm.set_row_data(i, thumbnail_item(i, make_cell()));
        }
    });
}

/// Per-generation state captured at `start`: the source + cache inputs, the
/// epoch/cancel guards for this open, and the set of page indices already
/// requested. Replaced wholesale on each open so a re-open starts with an empty
/// `requested` set and a fresh epoch/cancel.
struct StripGen {
    source: Arc<dyn PageSource>,
    page_count: usize,
    /// Canonical book path keying each page in the on-disk cache; `None` (or an
    /// unavailable cache dir) degrades gracefully to no persistence.
    path: Option<PathBuf>,
    epoch: usize,
    cancel: Arc<AtomicBool>,
    /// Page indices already dispatched to a worker — the de-dup set that keeps a
    /// page from being decoded twice across overlapping visible-range events.
    requested: HashSet<usize>,
}

/// Shared controller state held behind an `Rc` so the `visible-range-changed`
/// callback wired in `new` can dispatch backfill without a handle to the outer
/// `ThumbnailController`. All fields are touched only on the UI thread except the
/// `epoch`/cancel `Arc`s, which the spawned workers also read.
struct Inner {
    model: Rc<VecModel<ThumbnailItem>>,
    epoch: Arc<AtomicUsize>,
    cancel: RefCell<Arc<AtomicBool>>,
    /// The active generation, or `None` before the first open / after a sourceless
    /// open. Borrowed only on the UI thread (in `start` and the scroll callback).
    current: RefCell<Option<StripGen>>,
}

impl Inner {
    /// Supersede the previous generation's cancel flag and install a fresh one,
    /// returning a clone of the new flag for the just-started generation. Delegates
    /// to the shared [`crate::ui_marshal::rotate_cancel`], which single-homes the
    /// one-borrow-per-statement discipline; shared shape with `CoverController`.
    fn rotate_cancel(&self) -> Arc<AtomicBool> {
        crate::ui_marshal::rotate_cancel(&self.cancel)
    }

    /// Decode the not-yet-requested subset of `pages` (clamped to the current
    /// generation's range) on a background worker. The de-dup decision is recorded
    /// in `requested` under the UI thread *before* the worker spawns, so two
    /// overlapping visible-range events never decode the same page twice. A no-op
    /// when there is no active generation or every page was already requested.
    fn request_pages(&self, weak: slint::Weak<ViewerWindow>, pages: Vec<usize>) {
        let mut guard = self.current.borrow_mut();
        let Some(gen) = guard.as_mut() else {
            return;
        };
        let fresh = take_fresh(&mut gen.requested, gen.page_count, pages);
        if fresh.is_empty() {
            return;
        }
        let source = Arc::clone(&gen.source);
        let path = gen.path.clone();
        let cancel = Arc::clone(&gen.cancel);
        let my_epoch = gen.epoch;
        let epoch = Arc::clone(&self.epoch);
        // Drop the borrow before spawning so a worker that synchronously re-enters
        // the UI (it does not, but defensively) cannot double-borrow `current`.
        drop(guard);
        spawn_decode(weak, source, path, cancel, epoch, my_epoch, fresh);
    }
}

/// Decode each page in `pages` on a background thread and marshal the results to
/// the strip, scoped to one visible-range batch. Mirrors the epoch + double
/// cancel-check discipline of the former all-pages worker. The on-disk cache is
/// built once per batch on the worker (it holds only a directory; building it off
/// the UI thread side-steps a `!Send` capture — mirrors `cover_loader`): a miss
/// decodes + persists, a hit is a fast read. Best-effort — with no path or an
/// unavailable cache dir the batch still decodes, just without persistence.
fn spawn_decode(
    weak: slint::Weak<ViewerWindow>,
    source: Arc<dyn PageSource>,
    path: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    pages: Vec<usize>,
) {
    std::thread::spawn(move || {
        let cache = match path.as_deref() {
            Some(_) => match ThumbnailCache::new() {
                Ok(cache) => Some(cache),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "no thumbnail cache available; strip thumbnails will not be persisted"
                    );
                    None
                }
            },
            None => None,
        };
        let cache_ctx = match (cache.as_ref(), path.as_deref()) {
            (Some(cache), Some(path)) => Some(PageThumbContext { cache, path }),
            _ => None,
        };
        pages.par_iter().for_each(|&i| {
            // Skip work for a superseded generation before AND after the decode so
            // a re-open stops promptly and never delivers a stale result.
            if cancel.load(Relaxed) {
                return;
            }
            let res: Result<DecodedImage, CoreError> =
                generate_one_thumbnail(&source, DEFAULT_THUMB_MAX_SIDE, i, cache_ctx);
            if cancel.load(Relaxed) {
                return;
            }
            match res {
                // Build the slint::Image INSIDE the marshalled closure (UI thread);
                // `img` (Send) is captured, the `!Send` image is produced there.
                Ok(img) => marshal_cell(weak.clone(), Arc::clone(&epoch), my_epoch, i, move || {
                    ThumbCell::Loaded(to_slint_image(&img))
                }),
                Err(e) => {
                    // Log on the worker so the CoreError never crosses the thread
                    // boundary; the failed cell shows a distinct red placeholder.
                    tracing::warn!(page = i, error = %e, "thumbnail generation failed");
                    marshal_cell(weak.clone(), Arc::clone(&epoch), my_epoch, i, || {
                        ThumbCell::Failed
                    });
                }
            }
        });
    });
}

/// Owns the thumbnail strip's model and the lazy-generation bookkeeping.
///
/// `model` is the live backing store for the strip (placeholders first, filled in
/// as visible pages decode on background threads). The shared `Inner` carries the
/// epoch (tags each generation so a superseded book's late callbacks are dropped),
/// the cancel flag (flipped the instant a new book opens), and the active
/// generation's de-dup set.
pub struct ThumbnailController {
    inner: Rc<Inner>,
}

impl ThumbnailController {
    /// Build the strip's model, bind it to the UI as the `thumbnails` model, wire
    /// the lazy backfill callback, and return the controller. Call once during UI
    /// setup.
    pub fn new(ui: &ViewerWindow) -> Self {
        let model = Rc::new(VecModel::<ThumbnailItem>::default());
        ui.set_thumbnails(ModelRc::from(model.clone()));
        let inner = Rc::new(Inner {
            model,
            epoch: Arc::new(AtomicUsize::new(0)),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
            current: RefCell::new(None),
        });

        // Backfill on scroll/resize: translate each event's pixel geometry (first pixel,
        // viewport width, cell stride) to a page window (+margin) and request new pages.
        let cb_inner = Rc::clone(&inner);
        let weak = ui.as_weak();
        ui.on_thumbnail_strip_visible_range_changed(move |first_px, viewport_w, cell_stride| {
            let pages = match cb_inner.current.borrow().as_ref() {
                Some(gen) => visible_pages(
                    first_px,
                    viewport_w,
                    cell_stride,
                    gen.page_count,
                    VISIBLE_MARGIN,
                ),
                None => return,
            };
            cb_inner.request_pages(weak.clone(), pages);
        });

        Self { inner }
    }

    /// Start a fresh lazy generation for the given source. Runs on the UI thread:
    /// supersedes the previous generation, resets the model to `page_count`
    /// `Loading` placeholders, records the new generation, then (when `source` is
    /// `Some`) requests only a modest initial window — NOT all pages. The rest is
    /// backfilled on demand by `visible-range-changed`. Invoked after every open.
    ///
    /// `path` is the canonical book path used to key each page's thumbnail in the
    /// on-disk cache so a second open of the same unchanged book reads ~10-30 KB
    /// PNGs instead of re-decoding full-resolution pages. `None` (or a cache dir
    /// that cannot be built) degrades gracefully to no persistence.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        source: Option<Arc<dyn PageSource>>,
        page_count: usize,
        path: Option<PathBuf>,
    ) {
        // 1. Supersede the previous generation and take this one's fresh cancel flag.
        let cancel_flag = self.inner.rotate_cancel();

        // 2. Tag this generation so superseded callbacks can be dropped.
        let my_epoch = self.inner.epoch.fetch_add(1, Relaxed) + 1;

        // 3. Rebuild the model with N `Loading` placeholders (decode nothing yet).
        let placeholders: Vec<ThumbnailItem> = (0..page_count)
            .map(|i| thumbnail_item(i, ThumbCell::Loading))
            .collect();
        self.inner.model.set_vec(placeholders);

        // 4. Grab the source (None => nothing to generate; clear any prior state).
        let source = match source {
            Some(s) => s,
            None => {
                *self.inner.current.borrow_mut() = None;
                return;
            }
        };

        // 5. Record the generation, then request only the initial leading window.
        *self.inner.current.borrow_mut() = Some(StripGen {
            source,
            page_count,
            path,
            epoch: my_epoch,
            cancel: cancel_flag,
            requested: HashSet::new(),
        });
        let initial: Vec<usize> = (0..INITIAL_VISIBLE_PAGES.min(page_count)).collect();
        self.inner.request_pages(ui_weak, initial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure mapping test (no display server): proves the sum type collapses to
    // the correct (loaded, failed) booleans at each of the three cell states.
    #[test]
    fn thumbnail_item_loading_maps_to_neither_flag() {
        let item = thumbnail_item(5, ThumbCell::Loading);
        assert_eq!(item.page, 5);
        assert!(!item.loaded);
        assert!(!item.failed);
    }

    #[test]
    fn thumbnail_item_loaded_sets_loaded_flag() {
        // slint::Image::default() constructs headlessly (no backend).
        let item = thumbnail_item(2, ThumbCell::Loaded(slint::Image::default()));
        assert_eq!(item.page, 2);
        assert!(item.loaded);
        assert!(!item.failed);
    }

    #[test]
    fn thumbnail_item_failed_sets_failed_flag() {
        let item = thumbnail_item(9, ThumbCell::Failed);
        assert_eq!(item.page, 9);
        assert!(!item.loaded);
        assert!(item.failed);
    }

    // ---- visible_pages: pure boundary tests (mirror cache::prefetch_indices) ----

    #[test]
    fn visible_pages_from_origin_no_margin() {
        // stride 100, viewport 300 at the origin → exactly cells 0,1,2.
        assert_eq!(visible_pages(0.0, 300.0, 100.0, 10, 0), vec![0, 1, 2]);
    }

    #[test]
    fn visible_pages_includes_margin_and_clamps_low() {
        // At the origin the low side cannot go negative — clamps to 0.
        assert_eq!(visible_pages(0.0, 300.0, 100.0, 10, 2), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn visible_pages_scrolled_middle() {
        // first_px 250 → first cell 2; last px 550 → ceil 6; margin 0 → [2,3,4,5].
        assert_eq!(visible_pages(250.0, 300.0, 100.0, 20, 0), vec![2, 3, 4, 5]);
    }

    #[test]
    fn visible_pages_clamps_high_at_n() {
        // Far right with margin 1: the high side clamps to the last page.
        assert_eq!(
            visible_pages(1700.0, 300.0, 100.0, 20, 1),
            vec![16, 17, 18, 19]
        );
    }

    #[test]
    fn visible_pages_negative_first_px_treated_as_origin() {
        // A Flickable can momentarily over-scroll; a negative first pixel clamps to 0.
        assert_eq!(visible_pages(-50.0, 300.0, 100.0, 10, 0), vec![0, 1, 2]);
    }

    #[test]
    fn visible_pages_empty_source_is_empty() {
        assert_eq!(visible_pages(0.0, 300.0, 100.0, 0, 3), Vec::<usize>::new());
    }

    #[test]
    fn visible_pages_degenerate_geometry_is_empty() {
        // Not-yet-laid-out strip: a zero viewport or zero stride requests nothing.
        assert_eq!(visible_pages(0.0, 0.0, 100.0, 10, 1), Vec::<usize>::new());
        assert_eq!(visible_pages(0.0, 300.0, 0.0, 10, 1), Vec::<usize>::new());
    }

    // ---- take_fresh: de-dup gate ----

    #[test]
    fn take_fresh_dedups_repeated_requests() {
        let mut req = HashSet::new();
        assert_eq!(take_fresh(&mut req, 10, vec![0, 1, 2]), vec![0, 1, 2]);
        // Overlapping event: only the genuinely new page survives.
        assert_eq!(take_fresh(&mut req, 10, vec![1, 2, 3]), vec![3]);
        // A fully-covered event issues no new decode.
        assert_eq!(
            take_fresh(&mut req, 10, vec![0, 1, 2, 3]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn take_fresh_clamps_out_of_range() {
        let mut req = HashSet::new();
        // Indices >= page_count are dropped (never marked requested).
        assert_eq!(take_fresh(&mut req, 3, vec![1, 2, 3, 4]), vec![1, 2]);
        assert!(!req.contains(&3) && !req.contains(&4));
    }
}
