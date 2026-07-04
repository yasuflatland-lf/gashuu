//! `Library` → carousel display mapping and Slint model building for the UI.
//!
//! The pure `Library` → `CarouselData` row derivation lives in `library_model`;
//! this module adapts those rows into Slint `CarouselItem`s, builds and binds the
//! backing `VecModel`, and derives the per-book cover requests. UI-thread only
//! (it constructs `slint::Image`s and the `Rc` model, both `!Send`).

use crate::cover_loader;
use crate::library_model::{carousel_data_for_indices, CarouselData};
use crate::{CarouselItem, ThumbnailItem, ViewerWindow};
use gashuu_core::Library;
use slint::{ModelRc, VecModel};
use std::rc::Rc;

/// Adapt a pure `CarouselData` row into the Slint `CarouselItem` the carousel
/// renders. Runs on the UI thread (it builds a `slint::Image`). The cover field
/// starts as a placeholder (`slint::Image::default()`); `CoverController` fills
/// it in asynchronously via `invoke_from_event_loop` (hit and miss both stream
/// in from background workers — a cache hit just arrives sooner).
fn to_carousel_item(data: &CarouselData) -> CarouselItem {
    CarouselItem {
        cover: slint::Image::default(),
        title: data.title.as_str().into(),
        current: data.current,
        total: data.total,
        progress: data.progress,
        available: data.available,
        // Selection is orthogonal to row derivation: the model is built unselected, then
        // `apply_selection_flags` applies bulk selection over visible rows (survives query changes).
        selected: false,
        // Continue-reading: propagated from CarouselData (pure derivation in
        // `carousel_data_for_indices`; drives the BookmarkRibbon overlay).
        bookmarked: data.bookmarked,
        // Cover loading starts (or restarts on rebuild) un-failed: the row shows the neutral
        // loading placeholder until CoverController streams a cover or marks it failed (issue 144).
        cover_failed: false,
        // Starts false so the neutral loading placeholder shows (not the black default image)
        // while the async worker is in flight; set_cover() flips it true once a real image arrives.
        cover_loaded: false,
    }
}

/// Build the carousel's backing model from the given visible library `indices`,
/// in the order of `indices` (the search state's natural-order projection).
/// Pure model construction with NO `ViewerWindow` — headless-testable. The
/// matching [`bind_carousel_model`] performs the UI bind; keeping build and bind
/// separate lets a caller rebuild the model off the visible-index slice and bind
/// it in one place. Centralizes the Library → carousel row adaptation (mirrors
/// `ThumbnailController::new`).
pub(crate) fn build_carousel_model(
    library: &Library,
    indices: &[usize],
) -> Rc<VecModel<CarouselItem>> {
    let items: Vec<CarouselItem> = carousel_data_for_indices(library, indices)
        .iter()
        .map(to_carousel_item)
        .collect();
    Rc::new(VecModel::from(items))
}

/// Bind a freshly built carousel `model` into the UI's `carousel-items` and
/// return it, so callers (PR-V cover streaming; PR-L refresh-after-add) can
/// mutate the SAME model rather than rebuilding/rebinding. The build/bind split
/// keeps the (headless) row construction in [`build_carousel_model`] and the
/// UI-thread bind here.
pub(crate) fn bind_carousel_model(
    ui: &ViewerWindow,
    model: Rc<VecModel<CarouselItem>>,
) -> Rc<VecModel<CarouselItem>> {
    ui.set_carousel_items(ModelRc::from(model.clone()));
    model
}

/// Read-modify-write a single carousel row on the UI thread: re-fetch the `!Send`
/// `VecModel` through `ui` (never moved across threads), bounds-check `row`
/// against the CURRENT row count (tolerating a model that shrank since the
/// request was built — e.g. a book removed between scheduling and delivery),
/// clone the row, apply `f` to mutate exactly one field, and write it back.
/// Single-homes the fetch + downcast + bounds-check + read-modify-write that the
/// per-field carousel writers (`set_cover`, `set_carousel_total`,
/// `set_carousel_selected`) each repeated. A model whose downcast fails or whose
/// `row` is out of range is a silent no-op, matching the previous per-site guards.
pub(crate) fn update_carousel_row(
    ui: &ViewerWindow,
    row: usize,
    f: impl FnOnce(&mut CarouselItem),
) {
    use slint::Model;
    let model = ui.get_carousel_items();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<CarouselItem>>() else {
        return;
    };
    if row < vm.row_count() {
        let mut item = vm.row_data(row).expect("row < row_count checked above");
        f(&mut item);
        vm.set_row_data(row, item);
    }
}

/// Set the `selected` flag of carousel row `row`, on the UI thread. The
/// bulk-selection counterpart of `cover_loader::set_cover` — same `!Send`
/// `VecModel`-via-`ui` re-fetch and row-bounds check (tolerating a model that
/// shrank since the click was scheduled), swapping ONLY the row's `selected` so
/// the accent check badge appears/disappears without rebuilding the model or
/// restarting the cover stream.
pub(crate) fn set_carousel_selected(ui: &ViewerWindow, row: usize, selected: bool) {
    update_carousel_row(ui, row, |item| item.selected = selected);
}

/// Re-apply the bulk-selection flags over the CURRENTLY VISIBLE carousel rows:
/// for each visible row, set its `selected` flag from whether the underlying
/// book's path is in `is_selected`. Called after a model rebuild (the rebuilt
/// model starts unselected) so a selection survives a search-query change — the
/// projection recomputes, but the selection (keyed by path) is reprojected onto
/// the new rows. `indices` is the search state's visible projection, in the SAME
/// order the model was built from, so visible row `r` maps to `indices[r]`.
pub(crate) fn apply_selection_flags<F>(
    ui: &ViewerWindow,
    library: &Library,
    indices: &[usize],
    is_selected: F,
) where
    F: Fn(&std::path::Path) -> bool,
{
    for (row, &library_index) in indices.iter().enumerate() {
        if let Some(book) = library.books().get(library_index) {
            set_carousel_selected(ui, row, is_selected(book.path()));
        }
    }
}

/// Build one `CoverRequest` per visible library `index`, with `row` re-based to
/// the ENUMERATED position in the filtered slice (0, 1, 2, …) so cover targets
/// align with the filtered carousel model built by [`build_carousel_model`] from
/// the same `indices`. Out-of-range indices are skipped via
/// `Library::books().get(index)`. The cover controller resolves each book's
/// cache key from its path + mtime on a background worker and either serves the
/// cached cover or generates one — both off the UI thread.
///
/// `needs_count` is set for a book whose page count is still unknown
/// (`Book::page_count_opt() == None` — never opened, no persisted total). The
/// cover controller resolves the real total for those rows in the background so
/// the carousel shows "1 / 200" instead of "1 / 0"; a book already carrying a
/// count (opened before, or back-filled on a prior run) leaves it `false` and is
/// not re-opened just to count.
pub(crate) fn cover_requests(
    library: &Library,
    indices: &[usize],
) -> Vec<cover_loader::CoverRequest> {
    indices
        .iter()
        .filter_map(|&library_index| library.books().get(library_index))
        .enumerate()
        .map(|(row, book)| cover_loader::CoverRequest {
            row,
            path: book.path().to_path_buf(),
            needs_count: book.page_count_opt().is_none(),
        })
        .collect()
}

/// The mutually-constrained visual state of a thumbnail cell, as returned by
/// [`thumb_state_at`], mirroring the three states `ThumbnailCell` renders:
///   - loaded — `image` holds the decoded thumbnail; `failed` is always false.
///   - loading — blank `image`, `failed` false (still decoding).
///   - failed — blank `image`, `failed` true (decode failed).
///
/// `loaded` and `failed` are mutually exclusive: the constructors never produce
/// `loaded == true && failed == true` (the strip model enforces the same
/// invariant through its single `ThumbnailItem` constructor, `thumbnail_item`,
/// which all strip writes go through).
pub(crate) struct ThumbState {
    pub(crate) image: slint::Image,
    pub(crate) loaded: bool,
    pub(crate) failed: bool,
}

impl ThumbState {
    pub(crate) fn loaded(image: slint::Image) -> Self {
        Self {
            image,
            loaded: true,
            failed: false,
        }
    }

    pub(crate) fn loading() -> Self {
        Self {
            image: slint::Image::default(),
            loaded: false,
            failed: false,
        }
    }

    pub(crate) fn failed() -> Self {
        Self {
            image: slint::Image::default(),
            loaded: false,
            failed: true,
        }
    }
}

/// Returns the [`ThumbState`] for `page` from the strip model (image + the
/// loaded/failed flags `ThumbnailCell` renders). No decode happens here — it
/// only reflects the flags the strip already carries.
pub(crate) fn thumb_state_at(model: &slint::ModelRc<ThumbnailItem>, page: usize) -> ThumbState {
    use slint::Model;
    match model.row_data(page) {
        // `loaded` wins over `failed`: a loaded item shows its real image. The
        // model never sets both, so dropping `item.failed` here is safe.
        Some(item) if item.loaded => ThumbState::loaded(item.image),
        Some(item) if item.failed => ThumbState::failed(),
        Some(_) => ThumbState::loading(),
        None => {
            // `page` is outside the thumbnail model (strip/page_count desync): fall back to
            // the neutral loading placeholder, but log so the desync is diagnosable.
            tracing::warn!(
                page,
                row_count = model.row_count(),
                "thumb_state_at: page outside thumbnail model (strip/page_count desync)"
            );
            ThumbState::loading()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{ModelRc, VecModel};
    use std::num::NonZeroUsize;

    fn item(page: i32, loaded: bool, failed: bool) -> ThumbnailItem {
        ThumbnailItem {
            image: slint::Image::default(),
            page,
            loaded,
            failed,
        }
    }

    fn model(items: Vec<ThumbnailItem>) -> ModelRc<ThumbnailItem> {
        ModelRc::new(VecModel::from(items))
    }

    /// A fresh book (never opened, no persisted total) is flagged `needs_count`,
    /// so the cover controller resolves its real page count in the background; a
    /// book with a known count is NOT (it would only re-open the archive to learn
    /// what is already stored). Row index is the enumerated position in the
    /// supplied visible-indices slice (0, 1, ...), not the full-library position.
    #[test]
    fn cover_requests_flags_only_books_with_unknown_count() {
        let root = tempfile::tempdir().expect("tempdir");
        let unknown = root.path().join("unknown");
        let known = root.path().join("known");
        std::fs::create_dir(&unknown).expect("create unknown");
        std::fs::create_dir(&known).expect("create known");

        let mut lib = Library::new();
        assert!(lib.add(unknown.clone()).is_some());
        assert!(lib.add(known.clone()).is_some());
        // Give the naturally first-sorted book a persisted page count (as an open would back-fill).
        let known_path = lib.books()[0].path().to_path_buf();
        assert!(lib.set_page_count(&known_path, NonZeroUsize::new(10).unwrap()));

        let reqs = cover_requests(&lib, &[0, 1]);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].row, 0);
        assert_eq!(reqs[0].path, known_path);
        assert!(
            !reqs[0].needs_count,
            "book with a known total must not be re-opened just to count"
        );
        assert_eq!(reqs[1].row, 1);
        let unknown_path = lib.books()[1].path().to_path_buf();
        assert_eq!(reqs[1].path, unknown_path);
        assert!(
            reqs[1].needs_count,
            "fresh book with no persisted total needs its count resolved"
        );
    }

    /// The model is built strictly in the order of the supplied visible indices,
    /// not in natural `Library::books()` order: passing `[1, 0]` yields beta then
    /// alpha. This is what lets the carousel render the filtered/projected slice.
    #[test]
    fn build_carousel_model_uses_visible_indices_order() {
        use slint::Model;

        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/beta.cbz"))
            .is_some());

        let model = build_carousel_model(&lib, &[1, 0]);

        assert_eq!(model.row_count(), 2);
        assert_eq!(model.row_data(0).unwrap().title, "beta");
        assert_eq!(model.row_data(1).unwrap().title, "alpha");
    }

    /// `cover_requests` re-bases each request's `row` to its enumerated position
    /// in the filtered slice, so a single-element filter on library index 1 emits
    /// `row == 0` (the only row in the filtered model) carrying beta's path.
    #[test]
    fn cover_requests_rebase_rows_to_filtered_model() {
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/beta.cbz"))
            .is_some());

        let beta_path = lib.books()[1].path().to_path_buf();
        let reqs = cover_requests(&lib, &[1]);

        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].row, 0);
        assert_eq!(reqs[0].path, beta_path);
    }

    /// An out-of-range library index is dropped (via `Library::books().get`)
    /// rather than panicking; the surviving request keeps row 0 from the
    /// enumerated position.
    #[test]
    fn cover_requests_skips_out_of_range_index() {
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());

        let reqs = cover_requests(&lib, &[0, 99]);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].row, 0);
    }

    /// An empty visible-indices slice yields a model with no rows even when the
    /// library has books (an active search matching nothing).
    #[test]
    fn build_carousel_model_empty_indices_yields_no_rows() {
        use slint::Model;

        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());

        let model = build_carousel_model(&lib, &[]);
        assert_eq!(model.row_count(), 0);
    }

    /// `build_carousel_model` propagates `bookmarked` from `CarouselData` to each
    /// `CarouselItem`: the last-opened row has `bookmarked == true`, all others false.
    /// Natural sort: "alpha" < "beta", so alpha is row 0, beta is row 1.
    #[test]
    fn build_carousel_model_propagates_bookmarked_per_visible_row() {
        use slint::Model;

        let mut lib = Library::new();
        let alpha = std::path::PathBuf::from("/manga/alpha.cbz");
        let beta = std::path::PathBuf::from("/manga/beta.cbz");
        lib.register_opened(&alpha, None); // adds alpha + marks it last_opened
        lib.add(beta); // adds beta (not last_opened)

        let model = build_carousel_model(&lib, &[0, 1]);
        assert_eq!(model.row_count(), 2);
        assert!(
            model.row_data(0).unwrap().bookmarked,
            "alpha is the last-opened book and must be bookmarked"
        );
        assert!(
            !model.row_data(1).unwrap().bookmarked,
            "beta is not last-opened and must not be bookmarked"
        );
    }

    /// An unavailable (file-gone) book that is also the last-opened book must
    /// still carry `bookmarked == true` — the ribbon renders regardless of
    /// `available` (edge case 4 of the design).
    ///
    /// A non-existent path is used intentionally: `Library::add` falls back to
    /// the raw path when `canonicalize` fails (the file does not exist), so the
    /// book lands in the shelf as unavailable from the start. `register_opened`
    /// uses the same raw path, so the post-add lookup succeeds.
    #[test]
    fn build_carousel_model_unavailable_book_still_bookmarked() {
        use slint::Model;

        let mut lib = Library::new();
        // A path that never existed: `add` falls back to the raw path (no canonicalize),
        // so the book is stored unavailable; `register_opened` finds it via that same path.
        let path = std::path::PathBuf::from("/manga/gone.cbz");
        lib.register_opened(&path, None);

        let model = build_carousel_model(&lib, &[0]);
        assert_eq!(model.row_count(), 1);
        let item = model.row_data(0).unwrap();
        assert!(!item.available, "non-existent path is unavailable");
        assert!(
            item.bookmarked,
            "unavailable last-opened book must still be bookmarked"
        );
    }

    /// Rows are built with `cover_failed == false` and `cover_loaded == false`:
    /// a fresh model row shows the neutral loading placeholder (not the black
    /// default image) until the cover controller either streams in a cover
    /// (`cover_loaded = true`) or marks the row failed (`cover_failed = true`).
    /// A model rebuild resets any prior generation's flags so a refresh retries
    /// the load cleanly (issue 144).
    #[test]
    fn build_carousel_model_rows_start_in_loading_state() {
        use slint::Model;

        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());

        let model = build_carousel_model(&lib, &[0]);
        assert_eq!(model.row_count(), 1);
        let row = model.row_data(0).unwrap();
        assert!(
            !row.cover_failed,
            "fresh rows must start un-failed (neutral loading placeholder)"
        );
        assert!(
            !row.cover_loaded,
            "fresh rows must start un-loaded so the placeholder is shown, not the black default image"
        );
    }

    #[test]
    fn loaded_page_reports_loaded_not_failed() {
        let s = thumb_state_at(&model(vec![item(0, true, false)]), 0);
        assert!(s.loaded);
        assert!(!s.failed);
    }

    #[test]
    fn still_loading_page_reports_neither() {
        let s = thumb_state_at(&model(vec![item(0, false, false)]), 0);
        assert!(!s.loaded);
        assert!(!s.failed);
    }

    #[test]
    fn failed_page_reports_failed() {
        let s = thumb_state_at(&model(vec![item(0, false, true)]), 0);
        assert!(!s.loaded);
        assert!(s.failed);
    }

    #[test]
    fn loaded_page_overrides_failed_flag() {
        // A (loaded && failed) item can't occur by construction, but the loaded arm must
        // still report failed=false so a regression can't surface a loaded-yet-failed preview.
        let s = thumb_state_at(&model(vec![item(0, true, true)]), 0);
        assert!(s.loaded);
        assert!(!s.failed);
    }

    #[test]
    fn out_of_range_page_reports_blank() {
        let s = thumb_state_at(&model(vec![item(0, true, false)]), 5);
        assert!(!s.loaded);
        assert!(!s.failed);
    }
}
