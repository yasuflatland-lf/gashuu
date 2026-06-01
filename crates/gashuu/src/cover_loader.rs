//! Book-cover loading controller for the library carousel.
//!
//! Each book's cover is the thumbnail of its page 0 (see core `generate_cover`).
//! For every book in the carousel this controller derives `cache_key(path,
//! mtime_secs, max_side)`, tries the disk cache, and on a hit sets the row's
//! `cover` immediately (on the UI thread). On a miss it fires a fire-and-forget
//! rayon job that opens the source via `ArchiveLoader`, calls `generate_cover`,
//! stores the result with `ThumbnailCache::put`, then marshals the cover back to
//! the row via `invoke_from_event_loop`.
//!
//! Correctness across library refreshes is the SAME epoch + cancel double-guard
//! as `ThumbnailController` (see `thumbnail_strip.rs`): `start` bumps an
//! `AtomicUsize` epoch (so a late `invoke_from_event_loop` whose captured
//! `my_epoch` mismatches the current epoch is dropped) AND flips the previous
//! generation's `AtomicBool` cancel flag (so a worker that has not yet started
//! its heavy decode stops promptly). Either guard alone is insufficient.
//!
//! Thread-boundary rule (identical to the thumbnail strip): only `Send` values
//! cross into the rayon job and the event-loop closure — `slint::Weak` (Send+Sync),
//! the epoch and cancel `Arc`s, the row `usize`, `my_epoch`, the cache-key
//! `String`, the `PathBuf`, and the resulting `DecodedImage`. The `Rc` `VecModel`
//! and `slint::Image` (both `!Send`) are NEVER moved: the model is re-fetched and
//! the image built INSIDE the event-loop closure.

use crate::to_slint_image;
use crate::{CarouselItem, ViewerWindow};
use gashuu_core::{
    cache_key, generate_cover, ArchiveLoader, DecodedImage, ThumbnailCache, DEFAULT_THUMB_MAX_SIDE,
};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

/// Longer-edge size for generated covers. Reuses the strip thumbnail size so a
/// page-0 thumbnail and a cover share cache geometry; if the carousel later wants
/// larger covers, change this single constant (it is part of the cache key, so
/// changing it transparently invalidates old entries).
const COVER_MAX_SIDE: u32 = DEFAULT_THUMB_MAX_SIDE;

/// One book the controller must load a cover for: its carousel row index and its
/// canonical filesystem path. Built on the UI thread from the `Library`, then the
/// `PathBuf` (Send) is what crosses into the worker.
pub struct CoverRequest {
    pub row: usize,
    pub path: PathBuf,
}

/// Owns the carousel-cover generation bookkeeping (epoch + cancel double-guard).
/// It does NOT own the `VecModel` — that lives in `main.rs` and is bound into the
/// UI by the carousel builder; this controller re-fetches it through the
/// `Weak<ViewerWindow>` inside each event-loop closure (the model is `!Send`).
pub struct CoverController {
    epoch: Arc<AtomicUsize>,
    cancel: RefCell<Arc<AtomicBool>>,
}

impl Default for CoverController {
    fn default() -> Self {
        Self::new()
    }
}

/// Filesystem mtime of `path` as whole seconds since the Unix epoch, or `0` when
/// the file is missing / has no readable mtime (an unavailable book still gets a
/// stable, if degenerate, cache key). This is one of the three `cache_key` inputs
/// so a modified file regenerates its cover automatically (the cache owns hashing).
fn mtime_secs(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Set the `cover` image of carousel row `row` to `img`, on the UI thread.
/// Re-fetches the `!Send` `VecModel` through `ui` (never moved across threads),
/// reads the existing row, swaps only its `cover`, and writes it back. A
/// row-count bound check tolerates a model that shrank since the request was
/// built (e.g. a book removed between scheduling and delivery).
fn set_cover(ui: &ViewerWindow, row: usize, img: slint::Image) {
    let model = ui.get_carousel_items();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<CarouselItem>>() else {
        return;
    };
    if row < vm.row_count() {
        let mut item = vm.row_data(row).expect("row < row_count checked above");
        item.cover = img;
        vm.set_row_data(row, item);
    }
}

/// Marshal one cover image onto the UI thread for carousel row `row`, applying
/// the epoch-guard + upgrade + model-row preamble shared with the worker. The
/// `slint::Image` is built INSIDE this closure (it is `!Send`); the captured
/// `img` is the Send `DecodedImage`.
fn marshal_cover(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    img: DecodedImage,
) {
    if let Err(e) = slint::invoke_from_event_loop(move || {
        // Drop results from a superseded generation (library refreshed since).
        if epoch.load(Relaxed) != my_epoch {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Build the `!Send` slint::Image here, on the UI thread, then write the row.
        set_cover(&ui, row, to_slint_image(&img));
    }) {
        tracing::debug!(row, error = %e, "dropped cover update; event loop gone");
    }
}

impl CoverController {
    /// Build the controller. Call once during UI setup.
    pub fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        }
    }

    /// (Re)load covers for the given books. Cancels any in-flight generation,
    /// bumps the epoch, then for each request tries the cache (hit → set the row
    /// now) or fires a rayon worker (miss). Call after the carousel model is built
    /// or refreshed (initial library load + every add/remove).
    pub fn start(&self, ui_weak: slint::Weak<ViewerWindow>, requests: Vec<CoverRequest>) {
        // 1. Cancel the previous generation, install a fresh flag. Each borrow is
        //    confined to its own statement and drops at the `;` — mirrors
        //    ThumbnailController to avoid a double-borrow panic.
        self.cancel.borrow().store(true, Relaxed);
        *self.cancel.borrow_mut() = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&self.cancel.borrow());

        // 2. Tag this generation so superseded callbacks are dropped.
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;

        // Open the disk cache once for this generation. If the OS gave us no cache
        // dir, skip cover loading entirely (covers stay placeholders — a non-fatal
        // degraded state, logged once).
        let cache = match ThumbnailCache::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "no cover cache available; covers stay placeholders");
                return;
            }
        };

        let Some(ui) = ui_weak.upgrade() else {
            return;
        };

        for req in requests {
            let key = cache_key(&req.path, mtime_secs(&req.path), COVER_MAX_SIDE);
            // HIT: decode is already done on disk; set the row now (UI thread).
            if let Some(decoded) = cache.get(&key) {
                set_cover(&ui, req.row, to_slint_image(&decoded));
                continue;
            }
            // MISS: generate the cover on a rayon worker, store it, and stream it
            // back. Fire-and-forget (no join handle). Capture only Send values.
            let weak = ui_weak.clone();
            let epoch = Arc::clone(&self.epoch);
            let cancel = Arc::clone(&cancel_flag);
            let row = req.row;
            let path = req.path.clone();
            rayon::spawn(move || {
                // Bail before the heavy work if a newer generation superseded us.
                if cancel.load(Relaxed) {
                    return;
                }
                // Open the source for THIS book. A missing/unsupported file leaves
                // the row a placeholder — log and return, never panic.
                let source = match ArchiveLoader::open(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cover: open failed");
                        return;
                    }
                };
                let decoded = match generate_cover(source, COVER_MAX_SIDE) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cover: generate failed");
                        return;
                    }
                };
                // Store to the disk cache so the next launch is a hit. A cache write
                // failure is non-fatal (we already have the image to show).
                // Reconstruct the cache on this thread (ThumbnailCache is not Clone
                // and holds only a dir, so this is cheap and side-steps a !Send/!Clone
                // capture).
                if let Ok(cache) = ThumbnailCache::new() {
                    if let Err(e) = cache.put(&key, &decoded) {
                        tracing::warn!(path = %path.display(), error = %e, "cover: cache put failed");
                    }
                }
                // Post-generate cancel re-check before crossing to the UI thread
                // (mirrors generate_thumbnails' second cancel poll).
                if cancel.load(Relaxed) {
                    return;
                }
                marshal_cover(weak, epoch, my_epoch, row, decoded);
            });
        }
    }
}
