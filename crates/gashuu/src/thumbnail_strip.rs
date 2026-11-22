//! Desktop thumbnail strip. Each open requests the leading window from the backend,
//! then scroll and resize events request newly visible pages and their neighbours.
//! Closing the book session suppresses pending deliveries. The strip epoch also
//! guards the model against callbacks from an earlier strip generation.

use crate::backend::{BookBackend, SessionId};
use crate::to_slint_image;
use crate::ui_marshal::ui_done;
use crate::{ThumbnailItem, ViewerWindow};
use gashuu_core::DEFAULT_THUMB_MAX_SIDE;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
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
    // debug_assert guards a future hand-edit to the match arms in `thumbnail_item`.
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

/// Update one row of the current strip on the UI thread.
fn marshal_cell(ui: &ViewerWindow, page: usize, cell: ThumbCell) {
    let model = ui.get_thumbnails();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>() else {
        return;
    };
    if page < vm.row_count() {
        vm.set_row_data(page, thumbnail_item(page, cell));
    }
}

struct StripGen {
    session: SessionId,
    page_count: usize,
    epoch: usize,
    requested: HashSet<usize>,
}

struct Inner {
    backend: Rc<dyn BookBackend>,
    model: Rc<VecModel<ThumbnailItem>>,
    epoch: Arc<AtomicUsize>,
    current: RefCell<Option<StripGen>>,
}

impl Inner {
    fn request_pages(&self, weak: slint::Weak<ViewerWindow>, pages: Vec<usize>) {
        let mut current = self.current.borrow_mut();
        let Some(gen) = current.as_mut() else {
            return;
        };
        let fresh = take_fresh(&mut gen.requested, gen.page_count, pages);
        for page in fresh {
            self.backend.thumbnail(
                gen.session,
                page,
                DEFAULT_THUMB_MAX_SIDE,
                ui_done::<Result<Arc<_>, _>>(
                    weak.clone(),
                    Arc::clone(&self.epoch),
                    gen.epoch,
                    move |ui, result| {
                        let cell = match result {
                            Ok(img) => ThumbCell::Loaded(to_slint_image(&img)),
                            Err(e) => {
                                tracing::warn!(page, error = %e, "thumbnail generation failed");
                                ThumbCell::Failed
                            }
                        };
                        marshal_cell(ui, page, cell);
                    },
                ),
            );
        }
    }
}

/// Owns the thumbnail strip model and its per-open request bookkeeping.
pub struct ThumbnailController {
    inner: Rc<Inner>,
}

impl ThumbnailController {
    /// Build the strip's model, bind it to the UI as the `thumbnails` model, wire
    /// the lazy backfill callback, and return the controller. Call once during UI
    /// setup.
    pub fn new(ui: &ViewerWindow, backend: Rc<dyn BookBackend>) -> Self {
        let model = Rc::new(VecModel::<ThumbnailItem>::default());
        ui.set_thumbnails(ModelRc::from(model.clone()));
        let inner = Rc::new(Inner {
            model,
            epoch: Arc::new(AtomicUsize::new(0)),
            backend,
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

    /// Reset the strip and request the leading window of the active session.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        session: Option<SessionId>,
        page_count: usize,
    ) {
        let epoch = self.inner.epoch.fetch_add(1, Relaxed) + 1;
        *self.inner.current.borrow_mut() = session.map(|session| StripGen {
            session,
            page_count,
            epoch,
            requested: HashSet::new(),
        });
        if session.is_none() {
            self.inner.model.set_vec(Vec::new());
            return;
        }

        let placeholders: Vec<_> = (0..page_count)
            .map(|i| thumbnail_item(i, ThumbCell::Loading))
            .collect();
        self.inner.model.set_vec(placeholders);
        let initial = (0..INITIAL_VISIBLE_PAGES.min(page_count)).collect();
        self.inner.request_pages(ui_weak, initial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::fake::{Call, FakeBackend};
    use crate::backend::BackendError;

    fn headless_window() -> ViewerWindow {
        struct HeadlessPlatform;
        impl slint::platform::Platform for HeadlessPlatform {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                use slint::platform::software_renderer::{
                    MinimalSoftwareWindow, RepaintBufferType,
                };
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(HeadlessPlatform)).unwrap();
        ViewerWindow::new().unwrap()
    }

    #[test]
    fn strip_requests_initial_window_once_per_page() {
        let ui = headless_window();
        let backend = FakeBackend::new();
        let strip = ThumbnailController::new(&ui, backend.clone());
        let session = backend.new_session();
        strip.start(ui.as_weak(), Some(session), INITIAL_VISIBLE_PAGES + 5);
        ui.invoke_thumbnail_strip_visible_range_changed(0.0, 300.0, 100.0);
        let expected: Vec<_> = (0..INITIAL_VISIBLE_PAGES)
            .map(|page| Call::Thumbnail(session, page, DEFAULT_THUMB_MAX_SIDE))
            .collect();
        assert_eq!(backend.calls(), expected);
    }

    #[test]
    fn strip_failed_thumbnail_sets_failed_cell() {
        let ui = headless_window();
        let backend = FakeBackend::new();
        let strip = ThumbnailController::new(&ui, backend.clone());
        strip.start(ui.as_weak(), Some(backend.new_session()), 2);
        backend.complete_thumbnail(
            1,
            Err(BackendError::Other {
                detail: "corrupt page".into(),
            }),
        );
        let model = ui.get_thumbnails();
        let failed = model.row_data(1).unwrap();
        assert!(failed.failed);
        assert!(!failed.loaded);
        assert!(!model.row_data(0).unwrap().failed);
    }

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
