//! Presentation-layer view state: which folder is open and the current spread.
//! Backed by `ImageCache` (LRU + background prefetch) since PR2; drives a
//! two-page spread (Single/Double/Auto, Standalone/Paired cover, LTR/RTL) since PR4/PR4a.
//!
//! The pure page-pairing math lives in `gashuu_core::spread` and is
//! reading-direction-agnostic: it decides WHICH pages form a spread (in reading
//! order); this layer owns the modes, the cache, and the navigation actions.
//! Placement (which side each page renders on) is resolved in the UI from
//! `reading_direction`; this state only exposes the leading/trailing images.

use crate::keymap::NavAction;
use gashuu_core::{
    next_leading, normalize_leading, prev_leading, spread_at, ArchiveLoader, CacheConfig,
    CoreError, CoverMode, DecodedImage, ImageCache, PageSource, ReadingDirection, Settings,
    SpreadLayout, SpreadMode,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One displayed unit: the leading image and, in two-page modes, an optional
/// trailing image. Both are `Arc<DecodedImage>` so a cache hit never copies the
/// multi-MB RGBA buffer.
pub struct SpreadImages {
    pub leading: Arc<DecodedImage>,
    pub trailing: Option<Arc<DecodedImage>>,
    /// `Some(page_index)` when the trailing page failed to decode and the view
    /// degraded to leading-only; `None` for a normal single- or two-page spread.
    pub trailing_failed: Option<usize>,
}

/// Map a scrub fraction (knob position along the track, 0.0 = screen-left edge,
/// 1.0 = screen-right edge) to a 0-based RAW page index. Pure and total: clamps
/// the fraction to `[0, 1]` (a non-finite input clamps to 0), guards
/// `page_count == 0` (returns 0), and rounds to the nearest page using the last
/// index (`page_count - 1`) as the span so BOTH ends are reachable.
///
/// `rtl` inverts the fraction: in right-to-left (manga) reading, dragging the
/// knob LEFT advances the page, so the screen-left edge maps to the LAST page
/// and the screen-right edge to the FIRST (spec §5 / decision 11). The returned
/// index is RAW; the caller normalizes it to a valid spread leading via
/// `ViewerState::jump_to` (which respects single/double + cover mode), so this
/// helper carries NO layout awareness — only direction and clamping.
//
// Test-only in this PR: the Slint scrubber resolves the knob fraction to a page
// in the UI and passes an already-resolved page to `on_scrub_commit`, so this
// pure helper is currently exercised only by the unit tests below. Same
// `#[allow(dead_code)]` convention as the test-only accessors above (in a binary
// crate, `pub` is not a public API surface, so `-D warnings` flags it).
#[allow(dead_code)]
pub fn scrub_fraction_to_page(fraction: f32, page_count: usize, rtl: bool) -> usize {
    if page_count == 0 {
        return 0;
    }
    // Clamp; a non-finite fraction (NaN/inf) collapses to 0 first.
    let f = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let f = if rtl { 1.0 - f } else { f };
    let last = page_count - 1;
    // Round half-up: +0.5 then floor (via `as usize` truncation on a non-negative
    // value). `last` fits f32 exactly for any realistic page count.
    let scaled = (f * last as f32 + 0.5) as usize;
    scaled.min(last)
}

/// Holds the active image cache, the current spread's leading page index, and
/// the display modes. `index` is ALWAYS a valid leading page for the current
/// `(spread_mode, cover_mode)`: it is reset to 0 on `set_source`, advanced only
/// via `next_leading`/`prev_leading`, and re-normalized after a mode toggle.
pub struct ViewerState {
    cache: Option<ImageCache>,
    /// The currently open `PageSource`, retained separately from `ImageCache`
    /// (which does not expose its source) so the UI can launch background
    /// thumbnail generation. `None` until a source is successfully opened.
    source: Option<Arc<dyn PageSource>>,
    page_count: usize,
    index: usize,
    cache_config: CacheConfig,
    spread_mode: SpreadMode,
    cover_mode: CoverMode,
    reading_direction: ReadingDirection,
    /// Window aspect ratio (width / height) used to resolve `SpreadMode::Auto`
    /// into a concrete `SpreadLayout`. Ignored by Single/Double. Defaults to
    /// `1.0` until the UI pushes the real window size via `set_viewport_size`.
    viewport_aspect: f32,
    /// Number of entries skipped during the most recent successful `open_path`
    /// call (unreadable or filtered-out entries). Zero when no path has been
    /// opened yet or when the last open skipped nothing. Only updated on
    /// `Ok(())` returns; an error return leaves the value from the previous
    /// successful open unchanged.
    last_open_skipped: usize,
    /// Canonical path of the most recently successfully opened source. `None`
    /// until `open_path` completes `Ok(())`; reset to `None` only by a
    /// subsequent `set_source` call (including the one the next successful
    /// `open_path` makes before re-setting it). A failed `open_path` returns
    /// early via `?` before `set_source`, so it leaves this field unchanged.
    /// Used by `main.rs` to form the write-back tuple `(path, state.index())`
    /// at every leave point without holding a concurrent borrow on both
    /// `state` and `library`.
    open_file: Option<PathBuf>,
}

impl ViewerState {
    pub fn new() -> Self {
        Self::with_cache_config(CacheConfig::default())
    }

    /// Construct with explicit cache config and default display modes
    /// (Single / Standalone / Ltr) so callers that only care about cache sizing
    /// get single-page behavior.
    pub fn with_cache_config(cache_config: CacheConfig) -> Self {
        Self {
            cache: None,
            source: None,
            page_count: 0,
            index: 0,
            cache_config,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Ltr,
            viewport_aspect: 1.0,
            last_open_skipped: 0,
            open_file: None,
        }
    }

    /// Construct from persisted `Settings`, copying both the cache config and the
    /// display modes.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            cache: None,
            source: None,
            page_count: 0,
            index: 0,
            cache_config: settings.cache_config(),
            spread_mode: settings.spread_mode,
            cover_mode: settings.cover_mode,
            reading_direction: settings.reading_direction,
            viewport_aspect: 1.0,
            last_open_skipped: 0,
            open_file: None,
        }
    }

    /// Replace the active source (used by `open_folder` and by tests). Wraps the
    /// source in a fresh `ImageCache`, discarding any previously cached pages and
    /// resetting to the first page. Stores a clone of the source so
    /// `current_source()` can return it without going through `ImageCache`.
    pub fn set_source(&mut self, source: Arc<dyn PageSource>) {
        self.source = Some(Arc::clone(&source));
        self.open_file = None;
        let cache = ImageCache::new(source, self.cache_config);
        self.page_count = cache.len();
        self.cache = Some(cache);
        self.index = 0;
    }

    /// Returns the currently opened page source, if any. Used by the UI to
    /// launch background thumbnail generation (`ImageCache` does not expose its
    /// source). Returns `None` before a source is successfully opened.
    pub fn current_source(&self) -> Option<Arc<dyn PageSource>> {
        self.source.clone()
    }

    /// Jump to the spread containing `page`. The target is normalized to a valid
    /// spread leading for the current modes (clicking a trailing member lands on
    /// its spread start), so `index` stays a valid leading. No-op (returns
    /// `false`) when no source is loaded or the resolved leading equals the
    /// current index. Out-of-range `page` is clamped.
    pub fn jump_to(&mut self, page: usize) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let target = normalize_leading(
            self.page_count,
            self.effective_layout(),
            self.cover_mode,
            page.min(self.page_count - 1),
        );
        let moved = target != self.index;
        self.index = target;
        moved
    }

    /// Whether a hypothetical jump to `page` would land on a double-page spread
    /// (used by the scrubber preview to show 1 vs 2 thumbnails WITHOUT changing
    /// the current index). Normalizes `page` to its spread leading for the
    /// current modes, then asks the pure `spread_at` whether that spread has a
    /// trailing page. Returns `false` when no source is loaded.
    pub fn preview_is_double(&self, page: usize) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let layout = self.effective_layout();
        let lead = normalize_leading(
            self.page_count,
            layout,
            self.cover_mode,
            page.min(self.page_count - 1),
        );
        spread_at(self.page_count, layout, self.cover_mode, lead)
            .trailing
            .is_some()
    }

    // Test-only accessors (same #[allow(dead_code)] convention as the existing
    // page_count()/index() accessors: in a binary crate, pub is not a public API
    // surface, so -D warnings flags cfg(test)-only callers as dead code).
    /// The cache configuration applied to newly opened books.
    #[allow(dead_code)]
    pub fn cache_config(&self) -> CacheConfig {
        self.cache_config
    }

    /// Open any supported path (directory or CBZ/ZIP archive) as the active
    /// source, resetting to the first page. Dispatches to the appropriate
    /// `PageSource` implementation via `ArchiveLoader`. Unreadable entries are
    /// counted and logged at WARN level; the caller receives `Ok(())` as long
    /// as the source itself opened successfully.
    pub fn open_path(&mut self, path: &Path) -> Result<(), CoreError> {
        let source = ArchiveLoader::open(path)?;
        let skipped = source.skipped_count();
        if skipped > 0 {
            tracing::warn!(skipped, path = %path.display(), "entries skipped while opening path");
        }
        self.last_open_skipped = skipped;
        self.set_source(source);
        // Canonicalize best-effort; fall back to the verbatim path on error
        // (same policy as Library::add — identity is the canonical form when
        // available, verbatim otherwise).
        self.open_file = Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        Ok(())
    }

    /// Number of entries skipped during the most recent successful `open_path`
    /// (or `open_folder`) call. Zero until a path has been successfully opened
    /// or when the last open skipped nothing. Only meaningful after `Ok(())`;
    /// an error return leaves the value from the previous successful open.
    pub fn last_open_skipped(&self) -> usize {
        self.last_open_skipped
    }

    /// The canonical path of the currently open source, or `None` after
    /// construction or a direct `set_source` call. Set on a successful
    /// `open_path`; a failed `open_path` leaves the previous value unchanged
    /// (it returns early before `set_source`). Used by `main.rs` to write the
    /// reading position back to the `Library` at leave points.
    pub fn open_file(&self) -> Option<&Path> {
        self.open_file.as_deref()
    }

    /// Open a folder as the active source, resetting to the first page.
    /// Delegates to `open_path` so both directories and archives share a single
    /// dispatch + skipped-warn path.
    pub fn open_folder(&mut self, path: &Path) -> Result<(), CoreError> {
        self.open_path(path)
    }

    /// Total page count of the current source (0 when none is open). Used by the
    /// thumbnail-strip wiring in `main.rs` to size the placeholder model, and by
    /// the unit tests below.
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// The current spread's leading page index. Used by the highlight wiring in
    /// `main.rs` (`refresh` pushes it into `current-index`), and by the unit
    /// tests below.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Resolve the configured `spread_mode` against the current viewport aspect
    /// into the concrete `SpreadLayout` the pure `spread::*` math operates on.
    /// Single/Double are identity; Auto picks Single/Double from the aspect.
    fn effective_layout(&self) -> SpreadLayout {
        self.spread_mode.resolve(self.viewport_aspect)
    }

    /// Apply a navigation action a spread at a time. Returns true if the leading
    /// index moved. In Single mode this is the old ±1 clamp; in Double mode it
    /// advances/retreats one spread (skipping the partner page). Auto first
    /// resolves to Single or Double from the current viewport aspect (via
    /// `effective_layout()`) before the navigation step.
    pub fn apply(&mut self, action: NavAction) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let layout = self.effective_layout();
        let next = match action {
            NavAction::Next => next_leading(self.page_count, layout, self.cover_mode, self.index),
            NavAction::Prev => prev_leading(self.page_count, layout, self.cover_mode, self.index),
        };
        let moved = next != self.index;
        self.index = next;
        moved
    }

    /// Return the current spread from the cache (decoding on a miss and
    /// triggering background prefetch). `None` when no folder is open or it has
    /// no pages.
    ///
    /// A leading-page decode error fails the whole call. A trailing-page decode
    /// error does NOT: it is logged and the spread degrades gracefully to a
    /// single page (showing the leading alone) rather than blanking the view.
    pub fn current_spread(&self) -> Option<Result<SpreadImages, CoreError>> {
        let cache = self.cache.as_ref()?;
        if self.page_count == 0 {
            return None;
        }
        let s = spread_at(
            self.page_count,
            self.effective_layout(),
            self.cover_mode,
            self.index,
        );
        let leading = match cache.get(s.leading) {
            Ok(img) => img,
            Err(e) => return Some(Err(e)),
        };
        let (trailing, trailing_failed) = match s.trailing {
            Some(t) => match cache.get(t) {
                Ok(img) => (Some(img), None),
                Err(e) => {
                    tracing::warn!(page = t, error = %e, "failed to decode trailing page; showing leading alone");
                    (None, Some(t))
                }
            },
            None => (None, None),
        };
        Some(Ok(SpreadImages {
            leading,
            trailing,
            trailing_failed,
        }))
    }

    /// Cycle Single -> Double -> Auto -> Single, then re-normalizes the index
    /// (via the effective layout) so the currently visible page stays on screen.
    /// Always returns `true` (a 3-cycle always changes the configured mode).
    pub fn toggle_spread(&mut self) -> bool {
        let before = self.spread_mode;
        self.spread_mode = match before {
            SpreadMode::Single => SpreadMode::Double,
            SpreadMode::Double => SpreadMode::Auto,
            SpreadMode::Auto => SpreadMode::Single,
        };
        self.renormalize_index();
        before != self.spread_mode
    }

    /// Update the window aspect ratio used to resolve `SpreadMode::Auto`. Returns
    /// `true` iff the EFFECTIVE layout changed (only possible in Auto, crossing
    /// the square boundary). On a change, re-normalizes the index so the
    /// currently visible page stays on screen. A degenerate window size — any
    /// `width`/`height` whose ratio is non-finite or non-positive — falls back to
    /// aspect `1.0`, so `viewport_aspect` always holds a valid ratio
    /// (`SpreadMode::resolve` is the standalone safety net but the field itself
    /// stays sane). No-op-safe with no folder open (index stays 0).
    pub fn set_viewport_size(&mut self, width: f32, height: f32) -> bool {
        let before = self.effective_layout();
        let aspect = width / height;
        self.viewport_aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let after = self.effective_layout();
        if before != after {
            self.renormalize_index();
            true
        } else {
            false
        }
    }

    /// Flip Standalone <-> Paired cover, then re-normalize the index so the
    /// currently visible page stays on screen. Returns `true` when the cover
    /// changed; a 2-variant toggle always flips, so this is currently always
    /// `true`. The bool is retained for forward-compatibility with multi-valued
    /// modes (e.g. a future `CoverMode` variant).
    pub fn toggle_cover(&mut self) -> bool {
        let before = self.cover_mode;
        self.cover_mode = match before {
            CoverMode::Standalone => CoverMode::Paired,
            CoverMode::Paired => CoverMode::Standalone,
        };
        self.renormalize_index();
        before != self.cover_mode
    }

    /// Flip Ltr <-> Rtl. Reading direction only affects placement, not pairing,
    /// so the index is left untouched. Returns `true` when the direction changed;
    /// a 2-variant toggle always flips, so this is currently always `true`. The
    /// bool is retained for forward-compatibility with multi-valued modes (e.g.
    /// a future `ReadingDirection` variant).
    pub fn toggle_reading_direction(&mut self) -> bool {
        let before = self.reading_direction;
        self.reading_direction = match before {
            ReadingDirection::Ltr => ReadingDirection::Rtl,
            ReadingDirection::Rtl => ReadingDirection::Ltr,
        };
        before != self.reading_direction
    }

    /// Set the spread mode to an exact value (vs. `toggle_spread`'s cycle).
    /// Re-anchors the index via `renormalize_index` so the visible page stays on
    /// screen. Idempotent: returns `false` (no change) when already set.
    pub fn set_spread_mode(&mut self, mode: SpreadMode) -> bool {
        if self.spread_mode == mode {
            return false;
        }
        self.spread_mode = mode;
        self.renormalize_index();
        true
    }

    /// Set the cover mode to an exact value (vs. `toggle_cover`'s flip).
    /// Re-anchors the index via `renormalize_index` so the visible page stays on
    /// screen. Idempotent: returns `false` (no change) when already set.
    pub fn set_cover_mode(&mut self, mode: CoverMode) -> bool {
        if self.cover_mode == mode {
            return false;
        }
        self.cover_mode = mode;
        self.renormalize_index();
        true
    }

    /// Update the cache configuration used the NEXT time a book is opened.
    ///
    /// This deliberately does NOT rebuild the current book's cache — the new
    /// values are consumed by `set_source` on the next open, matching the
    /// "applies to newly opened books" contract surfaced in the settings
    /// dialog. (Immediate runtime rebuild of the current cache is deferred.)
    pub fn set_cache_config(&mut self, cache_config: CacheConfig) {
        self.cache_config = cache_config;
    }

    /// Set the reading direction to an exact value (vs. `toggle_reading_direction`'s
    /// flip). Reading direction only affects placement, not pairing, so the index
    /// is left untouched. Idempotent: returns `false` (no change) when already set.
    pub fn set_reading_direction(&mut self, dir: ReadingDirection) -> bool {
        if self.reading_direction == dir {
            return false;
        }
        self.reading_direction = dir;
        true
    }

    /// Re-anchor `index` onto a valid leading for the current modes after a
    /// pairing-affecting toggle. No-op when no pages are loaded (keeps index 0).
    fn renormalize_index(&mut self) {
        if self.page_count == 0 {
            self.index = 0;
            return;
        }
        self.index = normalize_leading(
            self.page_count,
            self.effective_layout(),
            self.cover_mode,
            self.index,
        );
    }

    pub fn reading_direction(&self) -> ReadingDirection {
        self.reading_direction
    }

    pub fn spread_mode(&self) -> SpreadMode {
        self.spread_mode
    }

    pub fn cover_mode(&self) -> CoverMode {
        self.cover_mode
    }

    /// Status line: "No folder opened", "Folder contains no images", or a page
    /// range plus a mode label, e.g. "2\u{2013}3 / 6  [double \u{00b7} LTR]" or
    /// "1 / 100  [single \u{00b7} LTR]".
    pub fn status_text(&self) -> String {
        match (&self.cache, self.page_count) {
            (None, _) => "No folder opened".to_string(),
            (Some(_), 0) => "Folder contains no images".to_string(),
            (Some(_), _) => {
                let s = spread_at(
                    self.page_count,
                    self.effective_layout(),
                    self.cover_mode,
                    self.index,
                );
                let pages = if let Some(t) = s.trailing {
                    format!("{}\u{2013}{} / {}", s.leading + 1, t + 1, self.page_count)
                } else {
                    format!("{} / {}", s.leading + 1, self.page_count)
                };
                let mode_label = match self.spread_mode {
                    SpreadMode::Single => "single",
                    SpreadMode::Double => "double",
                    SpreadMode::Auto => "auto",
                };
                let dir_label = match self.reading_direction {
                    ReadingDirection::Ltr => "LTR",
                    ReadingDirection::Rtl => "RTL",
                };
                format!("{pages}  [{mode_label} \u{00b7} {dir_label}]")
            }
        }
    }
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "viewer_state/tests.rs"]
mod tests;
