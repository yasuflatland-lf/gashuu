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
    generate_thumbnails, CoreError, DecodedImage, PageSource, DEFAULT_THUMB_MAX_SIDE,
};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
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
    ThumbnailItem {
        image,
        page: page as i32,
        loaded,
        failed,
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

    /// Launch a fresh parallel thumbnail generation for the given source. Runs on
    /// the UI thread: cancels the previous generation, resets the model to
    /// `page_count` unloaded placeholders (`loaded = false`), then (when `source`
    /// is `Some`) spawns a worker that streams decoded thumbnails back via
    /// `invoke_from_event_loop`. Invoked after every successful open.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        source: Option<Arc<dyn PageSource>>,
        page_count: usize,
    ) {
        // 1. Cancel any in-flight generation, then install a fresh flag.
        //    Each borrow is confined to its own statement and drops at the `;`,
        //    so no borrow is held across the next — avoiding a double-borrow
        //    panic.
        self.cancel.borrow().store(true, Relaxed);
        *self.cancel.borrow_mut() = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&self.cancel.borrow());

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
        let on_ready = move |i: usize, res: Result<DecodedImage, CoreError>| {
            let img = match res {
                Ok(img) => img,
                Err(e) => {
                    // Log the error on the worker thread before marshaling so
                    // CoreError never crosses the thread boundary.
                    tracing::warn!(page = i, error = %e, "thumbnail generation failed");
                    // Marshal a failed-cell update to the UI thread so the
                    // cell shows a distinct red placeholder instead of the
                    // indistinguishable gray loading state.
                    let weak = weak.clone();
                    let epoch = Arc::clone(&epoch);
                    if let Err(e) = slint::invoke_from_event_loop(move || {
                        // Drop results from a superseded generation.
                        if epoch.load(Relaxed) != my_epoch {
                            return;
                        }
                        let Some(ui) = weak.upgrade() else {
                            return;
                        };
                        let model = ui.get_thumbnails();
                        let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>()
                        else {
                            return;
                        };
                        if i < vm.row_count() {
                            vm.set_row_data(i, thumbnail_item(i, ThumbCell::Failed));
                        }
                    }) {
                        tracing::debug!(
                            page = i,
                            error = %e,
                            "dropped failed-thumbnail update; event loop gone"
                        );
                    }
                    return;
                }
            };
            let weak = weak.clone();
            let epoch = Arc::clone(&epoch);
            if let Err(e) = slint::invoke_from_event_loop(move || {
                // Drop results from a superseded generation.
                if epoch.load(Relaxed) != my_epoch {
                    return;
                }
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                // Re-fetch the model on the UI thread (never move the Rc
                // across threads); convert to slint::Image here (non-Send).
                let model = ui.get_thumbnails();
                let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>() else {
                    return;
                };
                if i < vm.row_count() {
                    vm.set_row_data(
                        i,
                        thumbnail_item(i, ThumbCell::Loaded(to_slint_image(&img))),
                    );
                }
            }) {
                tracing::debug!(page = i, error = %e, "dropped thumbnail update; event loop gone");
            }
        };
        std::thread::spawn(move || {
            generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancel_flag, on_ready);
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
