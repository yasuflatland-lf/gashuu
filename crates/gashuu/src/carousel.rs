//! `Library` → carousel display mapping and Slint model building for the UI.
//!
//! The pure `Library` → `CarouselData` row derivation lives in `library_model`;
//! this module adapts those rows into Slint `CarouselItem`s, builds and binds the
//! backing `VecModel`, and derives the per-book cover requests. UI-thread only
//! (it constructs `slint::Image`s and the `Rc` model, both `!Send`).

use crate::book_path::locate;
use crate::cover_loader;
use crate::library_model::{carousel_data_for_indices, CarouselData};
use crate::{CarouselItem, ThumbnailItem, ViewerWindow};
use gashuu_core::BookId;
use gashuu_core::{Book, Library};
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
        // while the async worker is in flight; apply_loaded_cover() flips it true once a real image arrives.
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
    is_available: &dyn Fn(&Book) -> bool,
) -> Rc<VecModel<CarouselItem>> {
    let items: Vec<CarouselItem> = carousel_data_for_indices(library, indices, is_available)
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

/// Apply availability changes to the corresponding rows of the current model.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) fn patch_availability(
    model: &VecModel<CarouselItem>,
    row_ids: &[BookId],
    changed: &[(BookId, bool)],
) {
    use slint::Model;
    use std::collections::HashMap;

    let changed: HashMap<_, _> = changed
        .iter()
        .map(|(id, available)| (id, *available))
        .collect();
    for (row, id) in row_ids.iter().take(model.row_count()).enumerate() {
        if let Some(&available) = changed.get(id) {
            let mut item = model.row_data(row).expect("row < row_count checked above");
            item.available = available;
            model.set_row_data(row, item);
        }
    }
}

/// Read-modify-write a single carousel row on the UI thread: re-fetch the `!Send`
/// `VecModel` through `ui` (never moved across threads), bounds-check `row`
/// against the CURRENT row count (tolerating a model that shrank since the
/// request was built — e.g. a book removed between scheduling and delivery),
/// clone the row, apply `f` to mutate exactly one field, and write it back.
/// Single-homes the fetch + downcast + bounds-check + read-modify-write that the
/// per-field carousel writers (`apply_loaded_cover`, `set_carousel_total`,
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
/// bulk-selection counterpart of `cover_loader::apply_loaded_cover` — same `!Send`
/// `VecModel`-via-`ui` re-fetch and row-bounds check (tolerating a model that
/// shrank since the click was scheduled), swapping ONLY the row's `selected` so
/// the accent check badge appears/disappears without rebuilding the model or
/// restarting the cover stream.
pub(crate) fn set_carousel_selected(ui: &ViewerWindow, row: usize, selected: bool) {
    update_carousel_row(ui, row, |item| item.selected = selected);
}

/// Re-apply the bulk-selection flags over the CURRENTLY VISIBLE carousel rows:
/// for each visible row, set its `selected` flag from whether the underlying
/// book's ID is in `is_selected`. Called after a model rebuild (the rebuilt
/// model starts unselected) so a selection survives a search-query change — the
/// projection recomputes, but the selection (keyed by ID) is reprojected onto
/// the new rows. `indices` is the search state's visible projection, in the SAME
/// order the model was built from, so visible row `r` maps to `indices[r]`.
pub(crate) fn apply_selection_flags<F>(
    ui: &ViewerWindow,
    library: &Library,
    indices: &[usize],
    is_selected: F,
) where
    F: Fn(&BookId) -> bool,
{
    for (row, &library_index) in indices.iter().enumerate() {
        if let Some(book) = library.books().get(library_index) {
            set_carousel_selected(ui, row, is_selected(book.id()));
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
            id: book.id().clone(),
            path: locate(book.id()),
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

    #[test]
    fn patch_availability_updates_only_changed_rows() {
        use slint::Model;

        let row_ids = [BookId::new("a"), BookId::new("b"), BookId::new("c")];
        let model = VecModel::from(vec![
            CarouselItem {
                available: true,
                ..Default::default()
            },
            CarouselItem::default(),
            CarouselItem {
                available: true,
                ..Default::default()
            },
        ]);
        let changed = [
            (row_ids[1].clone(), true),
            (BookId::new("other"), false),
            (row_ids[0].clone(), false),
        ];

        patch_availability(&model, &row_ids, &changed);

        assert!(!model.row_data(0).unwrap().available);
        assert!(model.row_data(1).unwrap().available);
        assert!(model.row_data(2).unwrap().available);
    }

    #[test]
    fn patch_availability_tolerates_a_shrunk_model() {
        use slint::Model;

        let row_ids = [BookId::new("a"), BookId::new("b")];
        let model = VecModel::from(vec![CarouselItem::default(); 2]);
        model.remove(1);
        let changed = [(row_ids[0].clone(), true), (row_ids[1].clone(), true)];

        patch_availability(&model, &row_ids, &changed);

        assert_eq!(model.row_count(), 1);
        assert!(model.row_data(0).unwrap().available);
    }

    #[test]
    fn patch_availability_preserves_cover_and_selection_fields() {
        use slint::Model;

        let row_ids = [BookId::new("a"), BookId::new("b")];
        let loaded = CarouselItem {
            cover: slint::Image::from_rgba8(slint::SharedPixelBuffer::new(2, 3)),
            cover_loaded: true,
            selected: true,
            bookmarked: true,
            ..Default::default()
        };
        let failed = CarouselItem {
            cover_failed: true,
            selected: true,
            ..Default::default()
        };
        let model = VecModel::from(vec![loaded.clone(), failed.clone()]);
        let changed = [(row_ids[0].clone(), true), (row_ids[1].clone(), true)];

        patch_availability(&model, &row_ids, &changed);

        assert_eq!(
            model.row_data(0).unwrap(),
            CarouselItem {
                available: true,
                ..loaded
            }
        );
        assert_eq!(
            model.row_data(1).unwrap(),
            CarouselItem {
                available: true,
                ..failed
            }
        );
    }

    proptest::proptest! {
        #[test]
        fn cover_requests_compact_valid_indices_in_input_order(
            known_counts in proptest::collection::vec(proptest::bool::ANY, 1..=8),
            indices in proptest::collection::vec(0usize..16, 0..=20),
            endpoints in (0usize..8, 0usize..8),
        ) {
            let mut library = Library::new();
            let n = known_counts.len();
            for (index, known) in known_counts.into_iter().enumerate() {
                let id = BookId::new(format!("/manga/{index}.cbz"));
                assert!(library.add(id.clone(), format!("Book {index}")).is_some());
                if known {
                    assert!(library.set_page_count(&id, NonZeroUsize::new(10).unwrap()));
                }
            }
            // A guaranteed invalid index between two valid ones catches gaps in rebased rows.
            let indices: Vec<_> = [endpoints.0 % n, n].into_iter()
                .chain(indices).chain([endpoints.1 % n]).collect();
            let expected: Vec<_> = indices.iter().copied()
                .filter(|&index| index < n).collect();

            let actual = cover_requests(&library, &indices);
            proptest::prop_assert_eq!(actual.len(), expected.len());
            for (row, (request, index)) in actual.iter().zip(expected).enumerate() {
                let book = &library.books()[index];
                proptest::prop_assert_eq!(request.row, row);
                proptest::prop_assert_eq!(&request.id, book.id());
                proptest::prop_assert_eq!(&request.path, &locate(book.id()));
                proptest::prop_assert_eq!(request.needs_count, book.page_count_opt().is_none());
            }
        }
    }

    /// The model is built strictly in the order of the supplied visible indices,
    /// not in natural `Library::books()` order: passing `[1, 0]` yields beta then
    /// alpha. This is what lets the carousel render the filtered/projected slice.
    #[test]
    fn build_carousel_model_uses_visible_indices_order() {
        use slint::Model;

        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/beta.cbz"), "beta".into())
            .is_some());

        let model =
            build_carousel_model(&lib, &[1, 0], &crate::book_availability::book_path_exists);

        assert_eq!(model.row_count(), 2);
        assert_eq!(model.row_data(0).unwrap().title, "beta");
        assert_eq!(model.row_data(1).unwrap().title, "alpha");
    }

    /// An empty visible-indices slice yields a model with no rows even when the
    /// library has books (an active search matching nothing).
    #[test]
    fn build_carousel_model_empty_indices_yields_no_rows() {
        use slint::Model;

        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());

        let model = build_carousel_model(&lib, &[], &crate::book_availability::book_path_exists);
        assert_eq!(model.row_count(), 0);
    }

    /// `build_carousel_model` propagates `bookmarked` from `CarouselData` to each
    /// `CarouselItem`: the last-opened row has `bookmarked == true`, all others false.
    /// Natural sort: "alpha" < "beta", so alpha is row 0, beta is row 1.
    #[test]
    fn build_carousel_model_propagates_bookmarked_per_visible_row() {
        use slint::Model;

        let mut lib = Library::new();
        let alpha = BookId::new("/manga/alpha.cbz");
        let beta = BookId::new("/manga/beta.cbz");
        lib.register_opened(&alpha, "alpha", None); // adds alpha + marks it last_opened
        lib.add(beta, "beta".into()); // adds beta (not last_opened)

        let model =
            build_carousel_model(&lib, &[0, 1], &crate::book_availability::book_path_exists);
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
    /// A non-existent path is used intentionally: it is already the result of
    /// `book_path::identify` because resolution fails, so the book lands in the
    /// shelf as unavailable from the start.
    #[test]
    fn build_carousel_model_unavailable_book_still_bookmarked() {
        use slint::Model;

        let mut lib = Library::new();
        // A path that never existed is its own caller-supplied identity.
        let path = BookId::new("/manga/gone.cbz");
        lib.register_opened(&path, "gone", None);

        let model = build_carousel_model(&lib, &[0], &crate::book_availability::book_path_exists);
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
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());

        let model = build_carousel_model(&lib, &[0], &crate::book_availability::book_path_exists);
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
    fn thumb_state_at_covers_all_flag_combinations_and_missing_rows() {
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(1, 1);
        let image = slint::Image::from_rgba8(pixels);
        let blank = slint::Image::default();
        for (page, loaded, failed, expected) in [
            (0, false, false, (false, false, &blank)),
            (0, false, true, (false, true, &blank)),
            (0, true, false, (true, false, &image)),
            (0, true, true, (true, false, &image)),
            (5, true, false, (false, false, &blank)),
        ] {
            let mut thumbnail = item(0, loaded, failed);
            thumbnail.image = image.clone();
            let state = thumb_state_at(&model(vec![thumbnail]), page);
            assert_eq!((state.loaded, state.failed, &state.image), expected);
        }
    }
}
