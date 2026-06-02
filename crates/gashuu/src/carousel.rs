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

/// Fetch the thumbnail image for a 0-based page from the strip's existing
/// `VecModel<ThumbnailItem>` (the PR8a model). Returns the cell's image when the
/// row exists and is loaded; otherwise a default (empty) image. UI-thread only —
/// the `Rc` model is never crossed between threads. No new decode is performed.
pub(crate) fn thumb_image_at(model: &slint::ModelRc<ThumbnailItem>, page: usize) -> slint::Image {
    use slint::Model;
    match model.row_data(page) {
        Some(item) if item.loaded => item.image,
        Some(_) => slint::Image::default(), // still loading: normal, stay silent
        None => {
            // `page` is outside the thumbnail model — strip and page_count are out
            // of sync (the model is built to exactly page_count rows). Not fatal:
            // show a blank preview, but log so the desync is diagnosable.
            tracing::warn!(
                page,
                row_count = model.row_count(),
                "thumb_image_at: page outside thumbnail model (strip/page_count desync)"
            );
            slint::Image::default()
        }
    }
}
