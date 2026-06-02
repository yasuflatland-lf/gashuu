//! `Library` → carousel display mapping and Slint model building for the UI.
//!
//! The pure `Library` → `CarouselData` row derivation lives in `library_model`;
//! this module adapts those rows into Slint `CarouselItem`s, builds and binds the
//! backing `VecModel`, and derives the per-book cover requests. UI-thread only
//! (it constructs `slint::Image`s and the `Rc` model, both `!Send`).

use crate::cover_loader;
use crate::library_model::{carousel_data, CarouselData};
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

/// Build the carousel's backing model from the current `Library`, in shelf
/// order, and bind it to the UI's `carousel-items`. Returns the `Rc<VecModel>`
/// so callers (PR-V cover streaming; PR-L refresh-after-add) can mutate the
/// SAME model rather than rebuilding/rebinding. Centralizes the build + bind so
/// the Library → carousel surface lives in ONE place (mirrors
/// `ThumbnailController::new`).
pub(crate) fn build_carousel_model(
    ui: &ViewerWindow,
    library: &Library,
) -> Rc<VecModel<CarouselItem>> {
    let items: Vec<CarouselItem> = carousel_data(library)
        .iter()
        .map(to_carousel_item)
        .collect();
    let model = Rc::new(VecModel::from(items));
    ui.set_carousel_items(ModelRc::from(model.clone()));
    model
}

/// Build one `CoverRequest` per book, row index == carousel order (insertion
/// order, per the `Library` contract). The cover controller resolves each book's
/// cache key from its path + mtime and either serves a cached cover or generates
/// one in the background.
pub(crate) fn cover_requests(library: &Library) -> Vec<cover_loader::CoverRequest> {
    library
        .books()
        .iter()
        .enumerate()
        .map(|(row, book)| cover_loader::CoverRequest {
            row,
            path: book.path().to_path_buf(),
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
