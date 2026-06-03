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
/// it in asynchronously via `invoke_from_event_loop` (a cache hit paints
/// immediately; a miss streams in after a background decode).
fn to_carousel_item(data: &CarouselData) -> CarouselItem {
    CarouselItem {
        cover: slint::Image::default(),
        title: data.title.as_str().into(),
        current: data.current,
        total: data.total,
        progress: data.progress,
        available: data.available,
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

/// Build one `CoverRequest` per visible library `index`, with `row` re-based to
/// the ENUMERATED position in the filtered slice (0, 1, 2, …) so cover targets
/// align with the filtered carousel model built by [`build_carousel_model`] from
/// the same `indices`. Out-of-range indices are skipped via
/// `Library::books().get(index)`. The cover controller resolves each book's
/// cache key from its path + mtime and either serves a cached cover or generates
/// one in the background.
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
            // `page` is outside the thumbnail model — strip and page_count are
            // out of sync (the model is built to exactly page_count rows). This
            // is a desync, not a real in-progress load: fall back to the neutral
            // loading placeholder, but log so the desync is diagnosable.
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
    /// what is already stored). Row index tracks natural `Library::books()`
    /// order.
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
        // A (loaded && failed) item cannot occur by construction, but the loaded
        // arm must still report failed=false so a future regression cannot
        // surface a loaded-yet-failed preview.
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
