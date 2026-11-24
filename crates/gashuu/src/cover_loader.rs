//! Load carousel covers through the shared book backend in focus order.
//! Each refresh persists resolved counts, cancels older requests and advances
//! the UI epoch. Backend completions update their row on the UI thread.
//! Unknown counts arrive with the cover request and are queued for persistence.
//! Empty books trigger the library removal handler.

use crate::backend::{BackendError, BookBackend};
use crate::carousel::update_carousel_row;
use crate::page_count_prefetch::PageCountPrefetch;
use crate::ui_marshal::ui_done;
use crate::{to_slint_image, CarouselItem, LibraryStoreHandle, ViewerWindow};
use gashuu_core::{
    ArchivePolicy, BookFingerprint, BookId, DecodedImage, Library, Settings, ThumbStore,
    COVER_MAX_SIDE,
};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Cover generation and removal share `max_side` because it is part of the
/// persistent cache key. Desktop startup uses `max_bytes` for pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoverCachePolicy {
    /// Bytes the startup sweep can retain across covers and strip thumbnails.
    pub(crate) max_bytes: u64,
    /// Longer-edge size, in pixels, for generated carousel covers. Decoupled
    /// from the page strip's `DEFAULT_THUMB_MAX_SIDE` (160 px): a focused cover
    /// slot is up to 240×336 logical px, which is 480×672 PHYSICAL px on a 2×
    /// (Retina) display, so a 160 px buffer was upscaled ~4× and looked blurry.
    /// 512 px keeps covers sharp at 1× and near-sharp at 2× while staying a
    /// single cover per book in the cache. `max_side` is part of the cache key,
    /// so raising it transparently invalidates and regenerates every stale 160
    /// px cover on the next run.
    max_side: u32,
}

impl CoverCachePolicy {
    /// The single canonical cover-cache policy. The values are load-bearing:
    /// `max_side` (512) is a cover-key ingredient, so changing it
    /// invalidates the existing on-disk cache; `max_bytes` (256 MiB) is the
    /// prune target.
    pub(crate) const DEFAULT: Self = Self {
        max_bytes: 256 * 1024 * 1024,
        max_side: COVER_MAX_SIDE,
    };
}

/// One carousel row, its backend request ID and its location for diagnostics.
pub struct CoverRequest {
    pub row: usize,
    pub id: BookId,
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

/// Keeps each carousel refresh's callbacks tied to its own rows.
pub struct CoverController {
    backend: Rc<dyn BookBackend>,
    settings: Rc<RefCell<Settings>>,
    epoch: Arc<AtomicUsize>,
    prefetch: PageCountPrefetch,
}

/// Purge the current cover and return its fingerprint for any strip cleanup.
/// A missing cover or mtime drift can leave zero matches. The startup sweep
/// reclaims orphaned files when the cache exceeds its budget.
pub(crate) fn purge_cover(thumbs: &dyn ThumbStore, id: &BookId) -> BookFingerprint {
    let fp = thumbs.fingerprint(id);
    let removed = thumbs.purge_cover(fp, &[CoverCachePolicy::DEFAULT.max_side]);
    if removed == 0 {
        tracing::warn!(
            path = %id,
            "no persistent cover purged for removed book (missing, mtime drift, or unwritable cache)"
        );
    }
    fp
}

/// Apply a freshly loaded `cover` image to carousel row `row`, on the UI thread.
/// Re-fetches the `!Send` `VecModel` through `ui` (never moved across threads),
/// reads the existing row, sets its `cover` AND flips `cover_loaded` to `true`
/// (hence "loaded", not just a cover swap), and writes it back. A row-count bound
/// check tolerates a model that shrank since the request was built (e.g. a book
/// removed between scheduling and delivery).
fn apply_loaded_cover(ui: &ViewerWindow, row: usize, img: slint::Image) {
    update_carousel_row(ui, row, |item| {
        item.cover = img;
        item.cover_loaded = true;
    });
}

/// Mark carousel row `row`'s cover load as FAILED in `vm` (issue 144): swaps
/// only the row's `cover_failed` flag so the card renders the shared failed
/// treatment instead of the indistinguishable-from-loading placeholder. The
/// headless model core of [`set_cover_failed`] (split out so the bounds
/// tolerance and the single-field swap are unit-testable without a window —
/// see docs/quality-gates.md "A function returning ModelRc<T>"); the same
/// row-bounds check as `apply_loaded_cover` tolerates a model that shrank since
/// the request was dispatched.
fn mark_cover_failed(vm: &VecModel<CarouselItem>, row: usize) {
    if row < vm.row_count() {
        let mut item = vm.row_data(row).expect("row < row_count checked above");
        item.cover_failed = true;
        vm.set_row_data(row, item);
    }
}

/// Set the `cover_failed` flag of carousel row `row`, on the UI thread. The
/// failed-state counterpart of `apply_loaded_cover`: same `!Send`-`VecModel`-via-`ui`
/// re-fetch (never moved across threads), then the headless
/// [`mark_cover_failed`] does the bounds-checked single-field swap.
fn set_cover_failed(ui: &ViewerWindow, row: usize) {
    let model = ui.get_carousel_items();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<CarouselItem>>() else {
        return;
    };
    mark_cover_failed(vm, row);
}

impl CoverController {
    pub fn new(backend: Rc<dyn BookBackend>, settings: Rc<RefCell<Settings>>) -> Self {
        Self {
            backend,
            settings,
            epoch: Arc::new(AtomicUsize::new(0)),
            prefetch: PageCountPrefetch::new(),
        }
    }

    fn request_cover(
        &self,
        weak: &slint::Weak<ViewerWindow>,
        my_epoch: usize,
        req: CoverRequest,
        policy: ArchivePolicy,
    ) {
        let row = req.row;
        let id = req.id.clone();
        let on_cover = ui_done(
            weak.clone(),
            Arc::clone(&self.epoch),
            my_epoch,
            move |ui, result: Result<Arc<DecodedImage>, BackendError>| match result {
                Ok(image) => apply_loaded_cover(ui, row, to_slint_image(&image)),
                Err(BackendError::EmptyBook) => ui.invoke_empty_book_detected(id.as_str().into()),
                Err(error @ BackendError::FormatDisabled { .. }) => {
                    tracing::debug!(book = %id, path = %req.path.display(), %error, "cover: load failed");
                    set_cover_failed(ui, row);
                }
                Err(error) => {
                    tracing::warn!(book = %id, path = %req.path.display(), %error, "cover: load failed");
                    set_cover_failed(ui, row);
                }
            },
        );
        let on_count = req.needs_count.then(|| {
            let id = req.id.clone();
            let pending = self.prefetch.queue();
            ui_done(
                weak.clone(),
                Arc::clone(&self.epoch),
                my_epoch,
                move |ui, result| match result {
                    Ok(count) => PageCountPrefetch::record_count(&pending, ui, row, id, count),
                    Err(BackendError::EmptyBook) => {
                        ui.invoke_empty_book_detected(id.as_str().into())
                    }
                    Err(error @ BackendError::FormatDisabled { .. }) => {
                        tracing::debug!(book = %id, %error, "cover: count open failed");
                    }
                    Err(error) => tracing::warn!(book = %id, %error, "cover: count open failed"),
                },
            )
        });
        self.backend.cover(
            &req.id,
            policy,
            CoverCachePolicy::DEFAULT.max_side,
            on_cover,
            on_count,
        );
    }

    /// Persist any still-pending prefetched page counts. Call on shutdown or on a web suspend event;
    /// safe to call repeatedly and while the event loop is running (pending counts are drained, not
    /// duplicated). Counts resolved after the last start survive a restart instead of being recomputed
    /// by re-opening every archive.
    pub fn flush_counts(&self, library: &RefCell<Library>, library_store: &LibraryStoreHandle) {
        self.prefetch.flush(library, library_store);
    }

    /// Persist pending counts and start a generation for the freshly built carousel.
    /// Callers must release their library borrow before the pending counts are applied.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        library: &RefCell<Library>,
        library_store: &LibraryStoreHandle,
        requests: Vec<CoverRequest>,
    ) {
        self.prefetch.apply(library, library_store);
        self.backend.cancel_covers();
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;
        if ui_weak.upgrade().is_none() {
            return;
        }

        let started = crate::clock::now();
        let dispatched = requests.len();
        let policy = self.settings.borrow().archive_policy();
        for req in requests {
            self.request_cover(&ui_weak, my_epoch, req, policy);
        }
        tracing::debug!(
            dispatched,
            elapsed_us = started.elapsed().as_micros() as u64,
            "cover loading dispatched"
        );
    }
}

#[cfg(test)]
pub(crate) fn test_window() -> ViewerWindow {
    struct Platform;
    impl slint::platform::Platform for Platform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    slint::platform::set_platform(Box::new(Platform)).unwrap();
    ViewerWindow::new().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::{Call, FakeBackend};
    use gashuu_core::CoreError;
    use slint::{ComponentHandle, SharedString};
    use std::num::NonZeroUsize;

    /// The cover-cache policy values are load-bearing for on-disk compatibility:
    /// `max_side` (512) is a cover-key ingredient, so the
    /// refactor must NOT change it (a different value silently invalidates every
    /// existing cached cover). `max_bytes` (256 MiB) is the prune target. Pin
    /// both so a future edit that drifts them fails loudly here.
    #[test]
    fn default_policy_preserves_on_disk_values() {
        assert_eq!(
            CoverCachePolicy::DEFAULT.max_side,
            512,
            "max_side is a cache-key ingredient; changing it breaks existing covers"
        );
        assert_eq!(
            CoverCachePolicy::DEFAULT.max_bytes,
            256 * 1024 * 1024,
            "max_bytes is the on-disk cover-cache prune target"
        );
    }

    /// The policy is an immutable `Copy` value object (mirrors core's
    /// `CacheConfig`): copying it yields an equal instance.
    #[test]
    fn policy_is_copy_and_eq() {
        let a = CoverCachePolicy::DEFAULT;
        let b = a; // Copy
        assert_eq!(a, b);
    }

    /// A fully-populated row with every flag SET (except `cover_failed`), so the
    /// preserve-other-fields assertion below cannot pass vacuously on defaults.
    fn carousel_item(title: &str) -> CarouselItem {
        CarouselItem {
            cover: slint::Image::default(),
            title: title.into(),
            current: 3,
            total: 10,
            progress: 0.3,
            available: true,
            selected: true,
            bookmarked: true,
            cover_failed: false,
            cover_loaded: false,
        }
    }

    /// Marking a row failed flips ONLY `cover_failed`; every other field (title,
    /// counters, selection, bookmark) survives the read-modify-write untouched,
    /// and sibling rows are not touched at all.
    #[test]
    fn mark_cover_failed_sets_only_the_flag() {
        let vm = VecModel::from(vec![carousel_item("alpha"), carousel_item("beta")]);
        mark_cover_failed(&vm, 1);
        assert!(
            !vm.row_data(0).expect("row 0").cover_failed,
            "sibling row must stay un-failed"
        );
        let failed = vm.row_data(1).expect("row 1");
        assert!(failed.cover_failed, "marked row must read failed");
        assert_eq!(failed.title, "beta");
        assert_eq!(failed.current, 3);
        assert_eq!(failed.total, 10);
        assert!(failed.available);
        assert!(failed.selected);
        assert!(failed.bookmarked);
    }

    /// A row beyond the model (shrunk since the request was scheduled, e.g. a
    /// book removed between dispatch and delivery) is a no-op — the same
    /// tolerance as `apply_loaded_cover`'s bounds check; no panic, no write.
    #[test]
    fn mark_cover_failed_out_of_range_is_noop() {
        let vm = VecModel::from(vec![carousel_item("alpha")]);
        mark_cover_failed(&vm, 5);
        assert!(!vm.row_data(0).expect("row 0").cover_failed);
    }

    fn req(row: usize) -> CoverRequest {
        let id = BookId::new(format!("/manga/{row}.cbz"));
        CoverRequest {
            row,
            path: PathBuf::from(id.as_str()),
            id,
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

    #[test]
    fn start_cancels_covers_then_requests_each_book_in_focus_order() {
        let ui = test_window();
        let fake = FakeBackend::new();
        let controller =
            CoverController::new(fake.clone(), Rc::new(RefCell::new(Settings::default())));
        let mut requests = prioritize_by_focus(vec![req(0), req(1)], 1);
        requests[0].needs_count = true;
        controller.start(
            ui.as_weak(),
            &RefCell::new(Library::new()),
            &Rc::new(None),
            requests,
        );
        assert_eq!(
            fake.calls(),
            [
                Call::CancelCovers,
                Call::Cover {
                    id: req(1).id,
                    policy: ArchivePolicy::default(),
                    max_side: 512,
                    wants_count: true
                },
                Call::Cover {
                    id: req(0).id,
                    policy: ArchivePolicy::default(),
                    max_side: 512,
                    wants_count: false
                },
            ]
        );
    }

    #[test]
    fn start_passes_the_settings_archive_policy_to_every_cover() {
        let ui = test_window();
        let fake = FakeBackend::new();
        let settings = Rc::new(RefCell::new(Settings::default()));
        let controller = CoverController::new(fake.clone(), Rc::clone(&settings));
        settings.borrow_mut().allow_rar_archives = false;
        controller.start(
            ui.as_weak(),
            &RefCell::new(Library::new()),
            &Rc::new(None),
            vec![req(0), req(1)],
        );
        assert_eq!(
            fake.calls(),
            [
                Call::CancelCovers,
                Call::Cover {
                    id: req(0).id,
                    policy: ArchivePolicy { allow_rar: false },
                    max_side: 512,
                    wants_count: false,
                },
                Call::Cover {
                    id: req(1).id,
                    policy: ArchivePolicy { allow_rar: false },
                    max_side: 512,
                    wants_count: false,
                },
            ]
        );
    }

    #[test]
    fn start_persists_previously_resolved_counts_before_cancelling() {
        use gashuu_core::{BlobStore, LibraryStore};

        // The store observes the fake's call log at the instant the library is saved.
        thread_local! {
            static BACKEND: Rc<FakeBackend> = FakeBackend::new();
        }
        #[derive(Debug, Default)]
        struct Saves(AtomicUsize);
        impl BlobStore for Saves {
            fn write_atomic(&self, _: &str, bytes: &[u8]) -> Result<(), CoreError> {
                BACKEND.with(|backend| assert!(backend.calls().is_empty()));
                let saved = Library::from_json(std::str::from_utf8(bytes).unwrap())?;
                assert_eq!(saved.books()[0].page_count_opt(), Some(1));
                self.0.fetch_add(1, Relaxed);
                Ok(())
            }

            fn read(&self, _: &str) -> Result<Option<Vec<u8>>, CoreError> {
                unreachable!("refresh only saves")
            }

            fn quarantine(&self, _: &str, _: u64) -> Result<Option<String>, CoreError> {
                unreachable!("refresh only saves")
            }
        }
        let ui = test_window();
        let fake = BACKEND.with(Rc::clone);
        let controller =
            CoverController::new(fake.clone(), Rc::new(RefCell::new(Settings::default())));
        let saves = Arc::new(Saves::default());
        let store = Rc::new(Some(LibraryStore::with_blob_store(saves.clone())));
        let library = RefCell::new(Library::new());
        let id = req(0).id;
        library.borrow_mut().add(id.clone(), "book".into()).unwrap();
        PageCountPrefetch::record_count(
            &controller.prefetch.queue(),
            &ui,
            0,
            id,
            NonZeroUsize::MIN,
        );
        controller.start(slint::Weak::default(), &library, &store, vec![]);
        assert_eq!(library.borrow().books()[0].page_count_opt(), Some(1));
        assert_eq!(saves.0.load(Relaxed), 1);
        assert_eq!(fake.calls(), [Call::CancelCovers]);
    }

    fn start_cover(ui: &ViewerWindow) -> (Rc<FakeBackend>, CoverController) {
        ui.set_carousel_items(Rc::new(VecModel::from(vec![carousel_item("book")])).into());
        let fake = FakeBackend::new();
        let controller =
            CoverController::new(fake.clone(), Rc::new(RefCell::new(Settings::default())));
        let mut request = req(0);
        request.needs_count = true;
        controller.start(
            ui.as_weak(),
            &RefCell::new(Library::new()),
            &Rc::new(None),
            vec![request],
        );
        (fake, controller)
    }

    fn cover_image() -> Arc<DecodedImage> {
        Arc::new(DecodedImage::new(vec![255; 8], 2, 1).unwrap())
    }

    fn io_error() -> BackendError {
        BackendError::from_core(&CoreError::Io(std::io::Error::other("read failed")))
    }

    #[test]
    fn stale_generation_cover_and_count_are_ignored() {
        let ui = test_window();
        let (fake, controller) = start_cover(&ui);
        let (on_cover, on_count) = fake.take_cover(0);
        let mut replacement = carousel_item("replacement");
        // Slint treats empty images as unequal, even after cloning.
        let image = DecodedImage::new(vec![0; 4], 1, 1).unwrap();
        replacement.cover = to_slint_image(&image);
        ui.set_carousel_items(Rc::new(VecModel::from(vec![replacement.clone()])).into());
        controller.start(
            ui.as_weak(),
            &RefCell::new(Library::new()),
            &Rc::new(None),
            vec![],
        );

        on_cover(Ok(cover_image()));
        on_count.expect("requested count")(Ok(NonZeroUsize::MIN));

        let model = ui.get_carousel_items();
        assert_eq!(model.row_count(), 1);
        assert_eq!(model.row_data(0).unwrap(), replacement);
        assert!(controller.prefetch.queue().lock().unwrap().is_empty());
    }

    #[test]
    fn current_generation_cover_and_count_update_row_and_queue_count() {
        let ui = test_window();
        let (fake, controller) = start_cover(&ui);
        let count = NonZeroUsize::new(7).unwrap();
        fake.complete_cover(0, Ok(cover_image()), Some(Ok(count)));

        let row = ui.get_carousel_items().row_data(0).unwrap();
        assert!(row.cover_loaded);
        assert!(!row.cover_failed);
        assert_eq!(row.cover.size().width, 2);
        assert_eq!(row.cover.size().height, 1);
        assert_eq!(row.total, 7);
        assert_eq!(controller.prefetch.queue().lock().unwrap().len(), 1);

        let id = req(0).id;
        let mut library = Library::new();
        library.add(id, "book".into()).unwrap();
        let library = RefCell::new(library);
        controller.flush_counts(&library, &Rc::new(None));
        assert_eq!(library.borrow().books()[0].page_count_opt(), Some(7));
    }

    fn assert_empty_book_detected(
        cover: Result<Arc<DecodedImage>, BackendError>,
        count: Option<Result<NonZeroUsize, BackendError>>,
    ) {
        let ui = test_window();
        let (fake, controller) = start_cover(&ui);
        let detected = Rc::new(RefCell::new(Vec::new()));
        ui.on_empty_book_detected({
            let detected = detected.clone();
            move |id| detected.borrow_mut().push(id)
        });

        fake.complete_cover(0, cover, count);

        assert_eq!(*detected.borrow(), [SharedString::from(req(0).id.as_str())]);
        assert!(!ui.get_carousel_items().row_data(0).unwrap().cover_failed);
        assert!(controller.prefetch.queue().lock().unwrap().is_empty());
    }

    #[test]
    fn empty_cover_completion_signals_empty_book_once() {
        assert_empty_book_detected(Err(BackendError::EmptyBook), None);
    }

    #[test]
    fn empty_count_completion_signals_empty_book_once() {
        assert_empty_book_detected(Ok(cover_image()), Some(Err(BackendError::EmptyBook)));
    }

    #[test]
    fn format_disabled_cover_marks_row_failed_and_keeps_the_book() {
        let ui = test_window();
        let (fake, _controller) = start_cover(&ui);
        let detected = Rc::new(std::cell::Cell::new(false));
        ui.on_empty_book_detected({
            let detected = Rc::clone(&detected);
            move |_| detected.set(true)
        });

        fake.complete_cover(
            0,
            Err(BackendError::FormatDisabled {
                kind: gashuu_core::ArchiveKind::Rar,
            }),
            None,
        );

        let row = ui.get_carousel_items().row_data(0).unwrap();
        assert!(row.cover_failed);
        assert!(!row.cover_loaded);
        assert!(!detected.get());
    }

    #[test]
    fn cover_io_error_marks_cover_failed() {
        let ui = test_window();
        let (fake, _controller) = start_cover(&ui);
        fake.complete_cover(0, Err(io_error()), None);

        let row = ui.get_carousel_items().row_data(0).unwrap();
        assert!(row.cover_failed);
        assert!(!row.cover_loaded);
    }

    #[test]
    fn count_io_error_leaves_cover_loaded_and_queue_empty() {
        let ui = test_window();
        let (fake, controller) = start_cover(&ui);
        fake.complete_cover(0, Ok(cover_image()), Some(Err(io_error())));

        let row = ui.get_carousel_items().row_data(0).unwrap();
        assert!(row.cover_loaded);
        assert!(!row.cover_failed);
        assert!(controller.prefetch.queue().lock().unwrap().is_empty());
    }
}
