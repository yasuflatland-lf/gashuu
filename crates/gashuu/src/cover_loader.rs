//! Book-cover loading controller for the library carousel.
//!
//! Each book's cover is the thumbnail of its page 0 (see core `generate_cover`).
//! `start` is DISPATCH-ONLY on the UI thread: every request becomes one
//! fire-and-forget rayon job, and the worker does ALL the per-book I/O — derive
//! `cache_key(path, mtime_secs, max_side)`, try the disk cache (a hit reads and
//! decodes the cached PNG on the worker), or on a miss open the source via
//! `ArchiveLoader`, call `generate_cover`, and store the result with
//! `ThumbnailCache::put`. Either way the cover is marshalled back to the row via
//! `invoke_from_event_loop`. Hit and miss share this single worker path so a
//! large library can never freeze the event loop (a 500-book warm start used to
//! decode 500 cached PNGs inline on the UI thread). A PERMANENT failure (open
//! or decode error) is marshalled too: `marshal_failed` flips the row's
//! `cover_failed` flag so the card renders the shared failed treatment instead
//! of an indefinite loading placeholder (issue 144 — parity with the page
//! strip's failed cell).
//!
//! Correctness across library refreshes is the SAME epoch + cancel double-guard
//! as `ThumbnailController` (see `thumbnail_strip.rs`): `start` bumps an
//! `AtomicUsize` epoch (so a late `invoke_from_event_loop` whose captured
//! `my_epoch` mismatches the current epoch is dropped) AND flips the previous
//! generation's `AtomicBool` cancel flag (so a worker that has not yet started
//! its heavy open+decode stops promptly). Either guard alone is insufficient.
//!
//! Thread-boundary rule (identical to the thumbnail strip): only `Send` values
//! cross into the rayon job and the event-loop closure — `slint::Weak` (Send+Sync),
//! the epoch and cancel `Arc`s, the row `usize`, `my_epoch`, the cache-key
//! `String`, the `PathBuf`, and the resulting `DecodedImage`. The `Rc` `VecModel`
//! and `slint::Image` (both `!Send`) are NEVER moved: the model is re-fetched and
//! the image built INSIDE the event-loop closure.

use crate::carousel::update_carousel_row;
use crate::page_count_prefetch::{self, PageCountPrefetch, ResolvedCount};
use crate::to_slint_image;
use crate::ui_marshal::marshal_to_ui;
use crate::{CarouselItem, ViewerWindow};
use gashuu_core::{
    cache_key, generate_cover, thumbnail_cache::source_mtime_secs, ArchiveLoader, DecodedImage,
    Library, ThumbnailCache,
};
use lru::LruCache;
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Immutable cover-cache retention policy: how big the on-disk cover cache may
/// grow (`max_bytes`) and the longer-edge size of generated covers (`max_side`).
/// Both decisions are I/O retention policy, so they live together in one value
/// object instead of as scattered bare `const`s next to the rayon-dispatch glue
/// — the invariant-owner pattern of core's `CacheConfig`. The single
/// canonical instance is [`CoverCachePolicy::DEFAULT`]; the dispatch helpers
/// (`spawn_cache_prune`, `spawn_load`, `purge_cover`) read its fields rather
/// than free-standing constants.
///
/// `max_side` is also a `cache_key` / `purge_cover_for` ingredient, so its value must
/// stay 512 to keep compatibility with the existing on-disk cache — raising it
/// would transparently invalidate and regenerate every stale cover on the next
/// run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoverCachePolicy {
    /// Size cap, in bytes, for the on-disk cover cache; the startup sweep prunes
    /// down to this. 256 MiB holds roughly 650-1700 covers at `max_side` 512
    /// (PNG covers run ~150-400 KB each), far beyond a typical library, so
    /// eviction only ever bites on key-orphaned covers (source mtime drifted,
    /// see core `purge_cover_for`) and very large collections. The cap POLICY lives
    /// here in the app layer; core's `ThumbnailCache::prune` is only the
    /// mechanism (issue 143's ownership split).
    max_bytes: u64,
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
    /// `max_side` (512) is a `cache_key`/`purge_cover_for` ingredient, so changing it
    /// invalidates the existing on-disk cache; `max_bytes` (256 MiB) is the
    /// prune target.
    pub(crate) const DEFAULT: Self = Self {
        max_bytes: 256 * 1024 * 1024,
        max_side: 512,
    };
}

/// Byte budget for the in-memory decoded-cover LRU ([`CoverMemCache`]). A cover
/// is at most `max_side` (512) on its longer edge, so a decoded RGBA buffer is at
/// most 512×512×4 ≈ 1 MiB; 64 MiB therefore holds ~64 covers, a generous bound
/// for the rows a carousel sweeps back and forth over. This is a pure RAM/latency
/// tunable (it never touches the on-disk cache or any cache key), so it can be
/// raised or lowered freely without invalidating anything on disk.
const COVER_MEM_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// Process-lifetime, byte-budgeted in-memory LRU of decoded covers, checked
/// BEFORE the disk read in `spawn_load`. The on-disk cover cache HIT path still
/// `fs::read`s and PNG-decodes `<cache>/<key>.png` on every call
/// (`ThumbnailCache::get`), so a carousel scrolled back and forth re-decodes the
/// same covers repeatedly; serving an already-decoded cover from here removes
/// that repeat work. Covers are held as `Arc<DecodedImage>` so a mem hit is a
/// cheap refcount bump rather than an RGBA buffer copy.
///
/// Eviction is by running byte total, not entry count: each `insert` adds the new
/// buffer's bytes and pops the least-recently-used entry until the total is back
/// within `budget`, always keeping at least one entry so a single oversized cover
/// is still served from memory (the floor). Shared across rayon workers behind an
/// `Arc<Mutex<_>>`; the lock is held only around `get`/`insert`, never across a
/// decode or any I/O.
struct CoverMemCache {
    /// Unbounded by entry count — the byte budget is the only cap, enforced in
    /// `insert`. Values are shared so a hit clones the `Arc`, not the bytes.
    lru: LruCache<String, Arc<DecodedImage>>,
    /// Running sum of `rgba().len()` over every cached cover; the eviction key.
    total_bytes: u64,
    /// Soft cap in bytes. `insert` evicts down to it (floor of one entry).
    budget: u64,
}

impl CoverMemCache {
    /// Build an empty cache with the default [`COVER_MEM_BUDGET_BYTES`] budget.
    fn new() -> Self {
        Self {
            lru: LruCache::unbounded(),
            total_bytes: 0,
            budget: COVER_MEM_BUDGET_BYTES,
        }
    }

    /// Build an empty cache with an explicit byte budget. Test-only so unit tests
    /// can exercise eviction with tiny buffers instead of allocating 64 MiB.
    #[cfg(test)]
    fn with_budget(budget: u64) -> Self {
        Self {
            lru: LruCache::unbounded(),
            total_bytes: 0,
            budget,
        }
    }

    /// Fetch a cached cover by `key`, promoting it to most-recently-used (so a
    /// cover touched on this carousel pass survives a later eviction). Returns a
    /// cheap `Arc` clone, never a copy of the RGBA bytes.
    fn get(&mut self, key: &str) -> Option<Arc<DecodedImage>> {
        self.lru.get(key).map(Arc::clone)
    }

    /// Insert (or replace) a cover, then evict least-recently-used entries until
    /// the running byte total is within `budget`. Replacing an existing key first
    /// subtracts the old buffer's bytes so the total stays exact. At least one
    /// entry is always kept, so a single cover larger than the whole budget is
    /// still served from memory rather than thrashing in and out.
    fn insert(&mut self, key: String, img: Arc<DecodedImage>) {
        let added = img.rgba().len() as u64;
        if let Some(prev) = self.lru.put(key, img) {
            self.total_bytes = self.total_bytes.saturating_sub(prev.rgba().len() as u64);
        }
        self.total_bytes = self.total_bytes.saturating_add(added);
        while self.total_bytes > self.budget && self.lru.len() > 1 {
            let Some((_, evicted)) = self.lru.pop_lru() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.rgba().len() as u64);
        }
    }
}

/// Sweep the cover cache down to the policy's `max_bytes` on a rayon worker.
/// Fire-and-forget, called ONCE at startup right after the initial cover
/// dispatch — visible covers grab workers first, and the sweep's per-entry
/// stat/unlink I/O never touches the UI thread. Cap overflow created later in
/// the session is reclaimed on the next launch (issue 143 keep-it-simple).
///
/// Safe against the concurrent cover workers by construction: a just-written
/// PNG is the newest entry (last in eviction order), an in-flight `.tmp` is
/// protected by core's staleness age guard, and a cover pruned mid-read is a
/// plain cache miss that regenerates. Core stays log-free — the returned
/// `PruneReport` is logged here.
pub(crate) fn spawn_cache_prune() {
    rayon::spawn(|| {
        // `start` already warned once about the no-cache-dir degraded state.
        let Ok(cache) = ThumbnailCache::new() else {
            return;
        };
        let started = std::time::Instant::now();
        let report = cache.prune(CoverCachePolicy::DEFAULT.max_bytes);
        tracing::debug!(
            removed_files = report.removed_files,
            removed_bytes = report.removed_bytes,
            retained_bytes = report.retained_bytes,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "cover cache pruned"
        );
    });
}

/// One book the controller must load a cover for: its carousel row index and its
/// canonical filesystem path. Built on the UI thread from the `Library`, then the
/// `PathBuf` (Send) is what crosses into the worker.
pub struct CoverRequest {
    pub row: usize,
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

/// Owns the carousel-cover generation bookkeeping (epoch + cancel double-guard).
/// It does NOT own the `VecModel` — that is built and bound into the UI by
/// `carousel::build_carousel_model`; this controller re-fetches it through the
/// `Weak<ViewerWindow>` inside each event-loop closure (the model is `!Send`).
pub struct CoverController {
    epoch: Arc<AtomicUsize>,
    cancel: RefCell<Arc<AtomicBool>>,
    /// Page-count prefetch concern: the pending queue of counts resolved by
    /// background workers plus its UI-thread persistence lifecycle. The live
    /// carousel `total` is updated separately and immediately by `marshal_total`
    /// (display vs persist are independent — the queue is only the persistence
    /// bridge). See [`PageCountPrefetch`].
    prefetch: PageCountPrefetch,
    /// Process-lifetime, thread-shared, byte-budgeted LRU of decoded covers,
    /// checked before the disk read in `spawn_load` so a cover decoded once this
    /// session is not re-read + re-PNG-decoded on a later carousel pass. Behind an
    /// `Arc<Mutex<_>>` because rayon workers share it; the lock is held only around
    /// `get`/`insert`, never across a decode or any I/O. See [`CoverMemCache`].
    mem: Arc<Mutex<CoverMemCache>>,
}

impl Default for CoverController {
    fn default() -> Self {
        Self::new()
    }
}

/// Best-effort removal of `path`'s persistent cover — the single home of the
/// app's cover purge policy (`purge_cover_for(path,
/// &[CoverCachePolicy::DEFAULT.max_side])`). Core owns the mtime recipe, so the key
/// ingredients never leak to callers. A zero purge count is EXPECTED (missing
/// file, mtime drift, unwritable cache entry) and only warned: the orphan is
/// harmless and the startup prune sweep reclaims it later (issue 143).
pub(crate) fn purge_cover(cache: &ThumbnailCache, path: &std::path::Path) {
    let removed = cache.purge_cover_for(path, &[CoverCachePolicy::DEFAULT.max_side]);
    if removed == 0 {
        tracing::warn!(
            path = %path.display(),
            "no persistent cover purged for removed book (missing, mtime drift, or unwritable cache)"
        );
    }
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

/// Marshal one cover image onto the UI thread for carousel row `row`: the
/// event-loop closure applies the epoch-guard + upgrade preamble, then writes
/// the row via `apply_loaded_cover`. The `slint::Image` is built INSIDE this
/// closure (it is `!Send`); the captured `img` is the Send `Arc<DecodedImage>`
/// (an `Arc` so a memory-cached cover is shared, not buffer-copied, into the
/// closure — `to_slint_image` reads it through the `Arc`'s `Deref`).
fn marshal_cover(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
    img: Arc<DecodedImage>,
) {
    marshal_to_ui(weak, epoch, my_epoch, "cover", move |ui| {
        // Build the `!Send` slint::Image here, on the UI thread, then write the row.
        apply_loaded_cover(ui, row, to_slint_image(&img));
    });
}

/// Marshal a FAILED cover load onto the UI thread for carousel row `row`
/// (issue 144): the event-loop closure applies the same epoch-guard + upgrade
/// preamble as `marshal_cover`, then flips the row's `cover_failed` flag via
/// `set_cover_failed`. Fired by a worker whose source open or cover decode
/// failed permanently, so the card renders the shared failed treatment instead
/// of an indefinite loading placeholder — the cover counterpart of the strip's
/// failed cell (docs/patterns.md "Per-page thumbnail failure → distinct FAILED
/// cell"). A model rebuild resets the flag, so the next generation retries.
fn marshal_failed(
    weak: slint::Weak<ViewerWindow>,
    epoch: Arc<AtomicUsize>,
    my_epoch: usize,
    row: usize,
) {
    marshal_to_ui(weak, epoch, my_epoch, "cover-failed", move |ui| {
        set_cover_failed(ui, row);
    });
}

/// Marshal an "empty book detected" signal onto the UI thread for the book at
/// `path`. Fired by a worker that opened a book's source CLEANLY but found ZERO
/// pages (see `should_signal_empty`): the `on_empty_book_detected` handler in
/// `handlers/library.rs` removes the book, purges its cover cache, rebuilds the
/// carousel, and shows a notice. The counterpart of `marshal_total`: same epoch-guard + upgrade
/// preamble, so a signal from a superseded generation (library refreshed since)
/// is dropped rather than acting on a stale row.
///
/// Dropping a stale generation's signal cannot lose the detection: if this
/// generation is dropped, the still-present empty book is re-detected by the next
/// generation's worker (the open is re-issued for every book lacking a count);
/// once the book is removed it is absent from the next generation's requests, so
/// there is no removal loop. The `path` is the canonical removal key (a `Send`
/// `PathBuf`); the `!Send` `SharedString` is built inside the closure.
fn marshal_empty_book(
    weak: &slint::Weak<ViewerWindow>,
    epoch: &Arc<AtomicUsize>,
    my_epoch: usize,
    path: PathBuf,
) {
    marshal_to_ui(
        weak.clone(),
        Arc::clone(epoch),
        my_epoch,
        "empty-book",
        move |ui| {
            ui.invoke_empty_book_detected(path.to_string_lossy().as_ref().into());
        },
    );
}

/// Worker-side decision: should opening a book's source and counting its pages
/// raise the "empty book" signal? `true` only when the source opened CLEANLY
/// (`Ok`) AND counts no pages (`None` — the raw probe count converts to the
/// emptiness vocabulary at the call-site boundary) — an open ERROR is
/// unreadable, NOT empty, and keeps the placeholder + log behavior with no
/// signal. Pure and `Send`-free so the decision is unit-testable without a
/// Slint/UI event loop. The generic `E` avoids a dependency on
/// `ArchiveLoader`'s concrete error type in tests.
fn should_signal_empty<S, E>(open_result: &Result<S, E>, page_count: Option<NonZeroUsize>) -> bool {
    open_result.is_ok() && page_count.is_none()
}

/// After a cover HIT (in-memory OR on-disk), resolve a still-unknown page count
/// with one archive open and stream it to the row. Shared by both hit branches of
/// `spawn_load`: the cover is already marshalled, so this only fixes the "1 / 0"
/// display. An opened-but-empty book raises the empty-book signal; a count-open
/// FAILURE is logged, not fatal — the row keeps the cover already shown (no failed
/// marking). Runs on the rayon worker, touching only `Send` state; cancel is
/// re-checked after the open before crossing to the UI thread.
fn resolve_count_after_hit(
    pending: &Arc<Mutex<Vec<ResolvedCount>>>,
    weak: &slint::Weak<ViewerWindow>,
    epoch: &Arc<AtomicUsize>,
    my_epoch: usize,
    cancel: &Arc<AtomicBool>,
    row: usize,
    path: PathBuf,
) {
    // TODO(#175-followup): use open_with_policy once ArchivePolicy is threaded
    // through CoverRequest / CarouselRefresh.
    let open_result = ArchiveLoader::open(&path);
    match &open_result {
        Ok(source) => {
            let count = source.list_pages().len();
            // Post-open cancel re-check before crossing to the UI thread.
            if cancel.load(Relaxed) {
                return;
            }
            // Opened cleanly but zero pages: signal the empty book (count == 0 also means
            // push_and_marshal_count below queues nothing — NonZeroUsize::new(0) is None).
            if should_signal_empty(&open_result, NonZeroUsize::new(count)) {
                marshal_empty_book(weak, epoch, my_epoch, path.clone());
            }
            page_count_prefetch::push_and_marshal_count(
                pending, weak, epoch, my_epoch, row, path, count,
            );
        }
        Err(e) => {
            // A count-open failure is NOT a cover failure: the cover was already
            // marshalled, so the row keeps it (no failed marking, log only).
            tracing::warn!(path = %path.display(), error = %e, "cover: count open failed");
        }
    }
}

impl CoverController {
    /// Build the controller. Call once during UI setup.
    pub fn new() -> Self {
        Self {
            epoch: Arc::new(AtomicUsize::new(0)),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
            prefetch: PageCountPrefetch::new(),
            mem: Arc::new(Mutex::new(CoverMemCache::new())),
        }
    }

    /// Supersede the previous generation's cancel flag and install a fresh one,
    /// returning a clone of the new flag for the just-started generation. Delegates
    /// to the shared [`crate::ui_marshal::rotate_cancel`], which single-homes the
    /// one-borrow-per-statement discipline; shared shape with `ThumbnailController`.
    fn rotate_cancel(&self) -> Arc<AtomicBool> {
        crate::ui_marshal::rotate_cancel(&self.cancel)
    }

    /// Persist any still-pending prefetched page counts. Call once on shutdown
    /// (after the event loop ends) so counts resolved after the last `start`
    /// survive a restart instead of being recomputed by re-opening every archive.
    pub fn flush_counts(&self, library: &Rc<RefCell<Library>>) {
        self.prefetch.flush(library);
    }

    /// Load one cover on a rayon worker — the SINGLE per-book path for cache hit
    /// and miss alike, so no per-book I/O (mtime `fs::metadata`, cached-PNG read +
    /// decode, archive open, page-0 decode) ever runs on the UI thread.
    ///
    /// Worker flow: derive `cache_key` (reads the mtime HERE, not at dispatch) →
    /// try the in-memory cover LRU FIRST. MEM HIT → marshal the shared
    /// `Arc<DecodedImage>` straight to the row (no `fs::read`, no PNG decode),
    /// then resolve a still-unknown count. Otherwise try the disk cache. DISK HIT
    /// → the read + decode happens once here, the result is interned into the mem
    /// LRU (so the next carousel pass is a mem hit), then marshalled. MISS → open
    /// the source, capture the count from the same open (when needed), generate +
    /// store the cover (disk + mem), then marshal count and cover; an open or
    /// decode FAILURE marshals the row failed instead (issue 144). Cancel is
    /// polled before each heavy step and re-checked before crossing to the UI
    /// thread; every crossing goes through the epoch-guarded marshal helpers. The
    /// mem `Mutex` is held only around `get`/`insert`, never across a decode or
    /// any I/O. Capture rule: only `Send` values enter the closure.
    fn spawn_load(
        &self,
        ui_weak: &slint::Weak<ViewerWindow>,
        cancel_flag: &Arc<AtomicBool>,
        my_epoch: usize,
        req: CoverRequest,
    ) {
        let weak = ui_weak.clone();
        let epoch = Arc::clone(&self.epoch);
        let cancel = Arc::clone(cancel_flag);
        let pending = self.prefetch.queue();
        let mem = Arc::clone(&self.mem);
        rayon::spawn(move || {
            // Bail before the heavy work if a newer generation superseded us.
            if cancel.load(Relaxed) {
                return;
            }
            // Reconstruct the cache on this thread: ThumbnailCache isn't Clone and holds only a
            // dir, so this is cheap and side-steps a !Send/!Clone capture.
            let Ok(cache) = ThumbnailCache::new() else {
                return;
            };
            let key = cache_key(
                &req.path,
                source_mtime_secs(&req.path),
                CoverCachePolicy::DEFAULT.max_side,
            );
            // MEM HIT: cover decoded earlier this session — serve the shared Arc straight to the
            // row (no fs::read/decode). lock().ok() degrades a poisoned mutex to a disk read.
            let cached = mem.lock().ok().and_then(|mut c| c.get(&key));
            if let Some(decoded) = cached {
                marshal_cover(weak.clone(), Arc::clone(&epoch), my_epoch, req.row, decoded);
                // Cover is shown but the page count may still be unknown —
                // resolve it with one archive open so the row shows "1 / N".
                if req.needs_count && !cancel.load(Relaxed) {
                    resolve_count_after_hit(
                        &pending, &weak, &epoch, my_epoch, &cancel, req.row, req.path,
                    );
                }
                return;
            }
            // DISK HIT: read + decode on THIS worker, intern into the mem LRU so the next
            // carousel pass is a mem hit, then stream to the row first (the count open may be slow).
            if let Some(decoded) = cache.get(&key) {
                let decoded = Arc::new(decoded);
                if let Ok(mut c) = mem.lock() {
                    c.insert(key.clone(), Arc::clone(&decoded));
                }
                marshal_cover(weak.clone(), Arc::clone(&epoch), my_epoch, req.row, decoded);
                // Cover is cached but the page count may still be unknown —
                // resolve it with one archive open so the row shows "1 / N".
                if req.needs_count && !cancel.load(Relaxed) {
                    resolve_count_after_hit(
                        &pending, &weak, &epoch, my_epoch, &cancel, req.row, req.path,
                    );
                }
                return;
            }
            // MISS: open the source. An unreadable file marks the row FAILED (issue 144) so a
            // permanent failure is distinct from a loading placeholder; cancel-recheck before crossing.
            // TODO(#175-followup): use open_with_policy once ArchivePolicy
            // is threaded through CoverRequest / CarouselRefresh.
            let open_result = ArchiveLoader::open(&req.path);
            let source = match open_result {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %req.path.display(), error = %e, "cover: open failed");
                    if !cancel.load(Relaxed) {
                        marshal_failed(weak, epoch, my_epoch, req.row);
                    }
                    return;
                }
            };
            // The same open yields the page count for free; capture it BEFORE `source` moves into
            // generate_cover. None = opened cleanly but zero pages → signal empty and stop.
            let Some(page_count) = NonZeroUsize::new(source.list_pages().len()) else {
                if !cancel.load(Relaxed) {
                    marshal_empty_book(&weak, &epoch, my_epoch, req.path);
                }
                return;
            };
            let count = req.needs_count.then_some(page_count.get());
            let decoded = match generate_cover(source, CoverCachePolicy::DEFAULT.max_side) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(path = %req.path.display(), error = %e, "cover: generate failed");
                    // Decode failure: same FAILED marking as the open-error arm.
                    if !cancel.load(Relaxed) {
                        marshal_failed(weak, epoch, my_epoch, req.row);
                    }
                    return;
                }
            };
            // Store to the disk cache so the next launch is a hit. A cache write
            // failure is non-fatal (we already have the image to show).
            if let Err(e) = cache.put(&key, &decoded) {
                tracing::warn!(path = %req.path.display(), error = %e, "cover: cache put failed");
            }
            // Intern into the mem LRU so a later carousel pass over this row is a mem hit
            // (no disk read + decode). Wrap in Arc so the marshal below shares, not copies.
            let decoded = Arc::new(decoded);
            if let Ok(mut c) = mem.lock() {
                c.insert(key, Arc::clone(&decoded));
            }
            // Post-generate cancel re-check before crossing to the UI thread
            // (mirrors generate_thumbnails' second cancel poll).
            if cancel.load(Relaxed) {
                return;
            }
            // Stream the resolved count to the row and queue it for persistence.
            if let Some(c) = count {
                page_count_prefetch::push_and_marshal_count(
                    &pending, &weak, &epoch, my_epoch, req.row, req.path, c,
                );
            }
            marshal_cover(weak, epoch, my_epoch, req.row, decoded);
        });
    }

    /// (Re)load covers for the given books. First persists any page counts resolved
    /// since the previous generation (UI thread), then cancels any in-flight
    /// generation, bumps the epoch, and dispatches EVERY request to a rayon worker
    /// (`spawn_load` — hit and miss share one off-thread path; results stream back
    /// via `invoke_from_event_loop`). The UI-thread work here is the count
    /// persistence, a cheap cache-dir probe, and the dispatch loop — no per-book
    /// I/O, regardless of cache state or library size. Call after the carousel
    /// model is built or refreshed (initial library load + every add/remove). The
    /// caller MUST build `requests` BEFORE the call (releasing its own
    /// `library.borrow()`) so the `prefetch.apply` `borrow_mut` here cannot
    /// double-borrow.
    pub fn start(
        &self,
        ui_weak: slint::Weak<ViewerWindow>,
        library: &Rc<RefCell<Library>>,
        requests: Vec<CoverRequest>,
    ) {
        // 0. Persist counts the PREVIOUS generation's workers resolved (UI thread).
        self.prefetch.apply(library);

        // 1. Supersede the previous generation and take this one's fresh cancel flag.
        let cancel_flag = self.rotate_cancel();

        // 2. Tag this generation so superseded callbacks are dropped.
        let my_epoch = self.epoch.fetch_add(1, Relaxed) + 1;

        // Probe the disk cache once so the no-cache-dir degraded state is logged ONCE here,
        // not by every worker (workers rebuild their own handle — it only holds a dir path).
        if let Err(e) = ThumbnailCache::new() {
            tracing::warn!(error = %e, "no cover cache available; covers stay placeholders");
            return;
        }

        // Window already gone (shutdown race): don't queue useless workers.
        if ui_weak.upgrade().is_none() {
            return;
        }

        // 3. Dispatch. The elapsed log pins the UI-thread cost of `start` itself
        // (dispatch only — all per-book I/O happens on the workers).
        let started = std::time::Instant::now();
        let dispatched = requests.len();
        for req in requests {
            self.spawn_load(&ui_weak, &cancel_flag, my_epoch, req);
        }
        tracing::debug!(
            dispatched,
            elapsed_us = started.elapsed().as_micros() as u64,
            "cover loading dispatched"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `Arc<DecodedImage>` whose RGBA buffer is exactly `bytes` long, so
    /// `CoverMemCache`'s byte accounting can be driven with tiny, predictable
    /// allocations. `bytes` must be a multiple of 4 (RGBA), enforced by the
    /// `DecodedImage::new` invariant (`rgba.len() == width * height * 4`); we make
    /// a `bytes/4`-wide, 1-tall image.
    fn cover_of(bytes: usize) -> Arc<DecodedImage> {
        assert_eq!(bytes % 4, 0, "test cover bytes must be a multiple of 4");
        let width = (bytes / 4) as u32;
        Arc::new(
            DecodedImage::new(vec![0u8; bytes], width, 1).expect("valid RGBA dimensions for test"),
        )
    }

    /// Inserting past the budget evicts least-recently-used entries so the running
    /// total never exceeds the budget. Budget = 12 bytes; three 8-byte covers can
    /// hold at most one (8 ≤ 12 < 16), so each insert evicts the prior one.
    #[test]
    fn insert_past_budget_evicts_lru_and_caps_total() {
        let mut c = CoverMemCache::with_budget(12);
        c.insert("a".into(), cover_of(8));
        c.insert("b".into(), cover_of(8));
        c.insert("c".into(), cover_of(8));
        assert!(
            c.total_bytes <= 12,
            "running total {} must stay within the budget",
            c.total_bytes
        );
        assert_eq!(c.lru.len(), 1, "only the most-recent cover survives");
        assert!(c.get("c").is_some(), "the most-recently inserted survives");
        assert!(c.get("a").is_none(), "the oldest was evicted");
        assert!(c.get("b").is_none(), "the middle was evicted");
    }

    /// `get` promotes an entry to most-recently-used, so a promoted entry survives
    /// a later eviction while the now-oldest untouched entry is dropped. Budget 16
    /// holds two 8-byte covers; touching "a" then inserting "c" must evict "b".
    #[test]
    fn get_promotes_mru_so_touched_entry_survives_eviction() {
        let mut c = CoverMemCache::with_budget(16);
        c.insert("a".into(), cover_of(8));
        c.insert("b".into(), cover_of(8));
        assert!(c.get("a").is_some(), "promote a to MRU (b is now the LRU)");
        c.insert("c".into(), cover_of(8));
        assert!(c.get("a").is_some(), "promoted entry survived eviction");
        assert!(c.get("c").is_some(), "newly inserted entry is present");
        assert!(
            c.get("b").is_none(),
            "the un-promoted LRU entry was evicted"
        );
        assert!(c.total_bytes <= 16, "total stays within the budget");
    }

    /// A single entry larger than the whole budget is retained (floor = 1): the
    /// eviction loop stops at one entry rather than dropping the only cover, so an
    /// oversized cover is still served from memory instead of thrashing.
    #[test]
    fn single_oversized_entry_is_retained_as_floor() {
        let mut c = CoverMemCache::with_budget(8);
        c.insert("big".into(), cover_of(64));
        assert_eq!(c.lru.len(), 1, "the oversized entry is kept as the floor");
        assert!(c.get("big").is_some(), "oversized cover is still served");
        // A second oversized insert still leaves exactly one (the newest) entry.
        c.insert("big2".into(), cover_of(64));
        assert_eq!(c.lru.len(), 1, "floor stays at one even when over budget");
        assert!(c.get("big2").is_some(), "the newest oversized cover wins");
        assert!(
            c.get("big").is_none(),
            "the prior oversized cover was evicted"
        );
    }

    /// Re-inserting an existing key replaces its value and adjusts the running
    /// total by the byte delta (old subtracted, new added) rather than
    /// double-counting — so a replace never inflates `total_bytes`.
    #[test]
    fn reinsert_same_key_replaces_without_double_counting() {
        let mut c = CoverMemCache::with_budget(64);
        c.insert("k".into(), cover_of(8));
        c.insert("k".into(), cover_of(16));
        assert_eq!(c.lru.len(), 1, "same key replaces, not appends");
        assert_eq!(c.total_bytes, 16, "total reflects only the new buffer");
        let got = c.get("k").expect("key present");
        assert_eq!(got.rgba().len(), 16, "the replacement value is served");
    }

    /// The default constructor uses the documented 64 MiB budget tunable.
    #[test]
    fn default_budget_is_the_documented_constant() {
        let c = CoverMemCache::new();
        assert_eq!(c.budget, COVER_MEM_BUDGET_BYTES);
        assert_eq!(c.budget, 64 * 1024 * 1024);
        assert_eq!(c.total_bytes, 0, "a fresh cache holds nothing");
    }

    /// The cover-cache policy values are load-bearing for on-disk compatibility:
    /// `max_side` (512) is a `cache_key` / `purge_cover_for` ingredient, so the
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
        CoverRequest {
            row,
            path: PathBuf::from(format!("/manga/{row}.cbz")),
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

    /// A unit type and error stand in for `ArchiveLoader::open`'s `Ok`/`Err`,
    /// since `should_signal_empty` is generic over the source and error types —
    /// the decision is "opened cleanly AND no pages (`None`)".
    type OpenResult = Result<(), &'static str>;

    /// Opened cleanly with zero pages (`None`) IS an empty book: signal.
    #[test]
    fn signal_empty_on_open_success_zero_pages() {
        let open: OpenResult = Ok(());
        assert!(should_signal_empty(&open, None));
    }

    /// Opened cleanly with pages is a normal book: no signal.
    #[test]
    fn no_signal_on_open_success_with_pages() {
        let open: OpenResult = Ok(());
        assert!(!should_signal_empty(&open, NonZeroUsize::new(5)));
        assert!(!should_signal_empty(&open, NonZeroUsize::new(1)));
    }

    /// An open ERROR is unreadable, NOT empty — even if the (unused) count is
    /// zero, the error keeps the placeholder + log behavior and raises no signal.
    #[test]
    fn no_signal_on_open_error() {
        let open: OpenResult = Err("unreadable");
        assert!(!should_signal_empty(&open, None));
        assert!(!should_signal_empty(&open, NonZeroUsize::new(5)));
    }
}
