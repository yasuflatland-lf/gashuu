//! Thumbnail-strip generation controller.
//!
//! Owns the strip's backing `VecModel` plus the bookkeeping that keeps parallel
//! thumbnail generation correct across book re-opens: an epoch counter that tags
//! each generation (so a superseded book's late callbacks are dropped) and a
//! cancel flag that stops the previous generation's CPU work the instant a new
//! book opens. The synchronous decode/downscale work lives in core's
//! `generate_thumbnails`; this controller just launches it on a background
//! thread and marshals each result back to the UI thread.
//!
//! Thread-boundary rule: `start` spawns a `std::thread` that calls the blocking
//! `generate_thumbnails`; rayon's `par_iter` inside it invokes `on_ready` on
//! rayon pool threads. `on_ready` may capture only `Send` values — the
//! `slint::Weak` (Send+Sync), the epoch `Arc`, and the epoch `usize`. The decoded
//! `DecodedImage` is NOT a capture: it arrives as `on_ready`'s `res` parameter
//! and must also be `Send`. The `Rc` model and `slint::Image` are NOT `Send`; the
//! model is re-fetched and the image is built inside `invoke_from_event_loop`,
//! i.e. only ever on the UI thread.

use crate::to_slint_image;
use crate::{ThumbnailItem, ViewerWindow};
use gashuu_core::{
    generate_thumbnails, CoreError, DecodedImage, PageSource, PageThumbCache, ThumbnailCache,
    DEFAULT_THUMB_MAX_SIDE,
};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

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
    // guards a future hand-edit to the match arms (mirrors the codebase's
    // load-bearing-invariant `debug_assert` philosophy — see `seq_index`).
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
    if let Err(e) = slint::invoke_from_event_loop(move || {
        // Drop results from a superseded generation.
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Re-fetch the model on the UI thread (never move the Rc across threads);
        // any non-Send cell payload (e.g. a slint::Image) is built here.
        let model = ui.get_thumbnails();
        let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>() else {
            return;
        };
        if i < vm.row_count() {
            vm.set_row_data(i, thumbnail_item(i, make_cell()));
        }
    }) {
        tracing::debug!(page = i, error = %e, "dropped thumbnail update; event loop gone");
    }
}

/// Owns the thumbnail strip's model and the generation bookkeeping.
///
/// `model` is the live backing store for the strip (placeholders first, filled
/// in as decodes complete on a background thread). `epoch` tags each generation
/// so a superseded book's late callbacks are dropped; `cancel` lets us flip the
/// previous generation's cancel flag the instant a new book opens.
pub struct ThumbnailController {
    model: Rc<VecModel<ThumbnailItem>>,
    epoch: Arc<AtomicUsize>,
    cancel: RefCell<Arc<AtomicBool>>,
}

impl ThumbnailController {
    /// Build the strip's model, bind it to the UI as the `thumbnails` model, and
    /// return the controller. Call once during UI setup.
    pub fn new(ui: &ViewerWindow) -> Self {
        let model = Rc::new(VecModel::<ThumbnailItem>::default());
        ui.set_thumbnails(ModelRc::from(model.clone()));
        Self {
            model,
            epoch: Arc::new(AtomicUsize::new(0)),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        }
    }

    /// Supersede the previous generation's cancel flag and install a fresh one,
    /// returning a clone of the new flag for the just-started generation. Each
    /// `RefCell` borrow is confined to its own statement (dropped at the `;`) so no
    /// two overlap — collapsing these into one expression would compile but panic
    /// at runtime with a double borrow. Shared shape with `CoverController`.
    fn rotate_cancel(&self) -> Arc<AtomicBool> {
        self.cancel.borrow().store(true, Relaxed);
        *self.cancel.borrow_mut() = Arc::new(AtomicBool::new(false));
        Arc::clone(&self.cancel.borrow())
    }

    /// Launch a fresh parallel thumbnail generation for the given source. Runs on
    /// the UI thread: cancels the previous generation, resets the model to
    /// `page_count` unloaded placeholders (`loaded = false`), then (when `source`
    /// is `Some`) spawns a worker that streams decoded thumbnails back via
    /// `invoke_from_event_loop`. Invoked after every successful open.
    ///
    /// `path` is the canonical book path used to key each page's thumbnail in the
    /// on-disk cache so a second open of the same unchanged book reads ~10-30 KB
    /// PNGs instead of re-decoding every full-resolution page. `None` (or a cache
    /// dir that cannot be built) degrades gracefully to no persistence.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        source: Option<Arc<dyn PageSource>>,
        page_count: usize,
        path: Option<PathBuf>,
    ) {
        // 1. Supersede the previous generation and take this one's fresh cancel flag.
        let cancel_flag = self.rotate_cancel();

        // 2. Tag this generation so superseded callbacks can be dropped.
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;

        // 3. Rebuild the model with N unloaded placeholders (`loaded = false`).
        let placeholders: Vec<ThumbnailItem> = (0..page_count)
            .map(|i| thumbnail_item(i, ThumbCell::Loading))
            .collect();
        self.model.set_vec(placeholders);

        // 4. Grab the source (None => nothing to generate).
        let source = match source {
            Some(s) => s,
            None => return,
        };

        // 5. Spawn the worker. `on_ready` may capture only Send values: the
        //    Weak (Send+Sync), the epoch Arc, and the epoch usize. It must
        //    NOT capture the Rc model nor build a slint::Image off-thread
        //    (both non-Send) — the cell is built inside the event loop.
        //    The outer `std::thread::spawn` closure also moves `cancel_flag`
        //    and `source` (both Send) into the worker thread.
        let weak = ui_weak.clone();
        let epoch = Arc::clone(&self.epoch);
        let on_ready = move |i: usize, res: Result<DecodedImage, CoreError>| match res {
            // Build the slint::Image INSIDE the marshalled closure (UI thread);
            // `img` (Send) is captured, the `!Send` image is produced there.
            Ok(img) => marshal_cell(weak.clone(), Arc::clone(&epoch), my_epoch, i, move || {
                ThumbCell::Loaded(to_slint_image(&img))
            }),
            Err(e) => {
                // Log the error on the worker thread before marshaling so the
                // CoreError never crosses the thread boundary. The marshalled
                // failed cell shows a distinct red placeholder instead of the
                // indistinguishable gray loading state.
                tracing::warn!(page = i, error = %e, "thumbnail generation failed");
                marshal_cell(weak.clone(), Arc::clone(&epoch), my_epoch, i, || {
                    ThumbCell::Failed
                });
            }
        };
        std::thread::spawn(move || {
            // Build the on-disk cache on the worker (ThumbnailCache is not Clone
            // and holds only a directory, so this is cheap and side-steps a !Send
            // capture — mirrors cover_loader). Best-effort: with no book path or an
            // unavailable cache dir, fall back to no persistence (the strip still
            // generates, just isn't cached). One worker => the degraded state is
            // logged once.
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
                (Some(cache), Some(path)) => Some(PageThumbCache { cache, path }),
                _ => None,
            };
            generate_thumbnails(
                source,
                DEFAULT_THUMB_MAX_SIDE,
                cancel_flag,
                cache_ctx,
                on_ready,
            );
        });
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
}
