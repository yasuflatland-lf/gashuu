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
    next_leading, normalize_leading, prev_leading, spread_at, ArchiveLoader, CoreError, CoverMode,
    DecodedImage, ImageCache, PageSource, ReadingDirection, Settings, SpreadLayout, SpreadMode,
    DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS,
};
use std::path::Path;
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
    cache_size: usize,
    preload_pages: usize,
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
}

impl ViewerState {
    pub fn new() -> Self {
        Self::with_cache_config(DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS)
    }

    /// Construct with explicit cache config and default display modes
    /// (Single / Standalone / Ltr) so callers that only care about cache sizing
    /// get single-page behavior.
    pub fn with_cache_config(cache_size: usize, preload_pages: usize) -> Self {
        Self {
            cache: None,
            source: None,
            page_count: 0,
            index: 0,
            cache_size,
            preload_pages,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Ltr,
            viewport_aspect: 1.0,
            last_open_skipped: 0,
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
            cache_size: settings.cache_size,
            preload_pages: settings.preload_pages,
            spread_mode: settings.spread_mode,
            cover_mode: settings.cover_mode,
            reading_direction: settings.reading_direction,
            viewport_aspect: 1.0,
            last_open_skipped: 0,
        }
    }

    /// Replace the active source (used by `open_folder` and by tests). Wraps the
    /// source in a fresh `ImageCache`, discarding any previously cached pages and
    /// resetting to the first page. Stores a clone of the source so
    /// `current_source()` can return it without going through `ImageCache`.
    pub fn set_source(&mut self, source: Arc<dyn PageSource>) {
        self.source = Some(Arc::clone(&source));
        let cache = ImageCache::new(source, self.cache_size, self.preload_pages);
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

    // Test-only accessors (same #[allow(dead_code)] convention as the existing
    // page_count()/index() accessors: in a binary crate, pub is not a public API
    // surface, so -D warnings flags cfg(test)-only callers as dead code).
    #[allow(dead_code)]
    pub fn cache_size(&self) -> usize {
        self.cache_size
    }

    #[allow(dead_code)]
    pub fn preload_pages(&self) -> usize {
        self.preload_pages
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
        Ok(())
    }

    /// Number of entries skipped during the most recent successful `open_path`
    /// (or `open_folder`) call. Zero until a path has been successfully opened
    /// or when the last open skipped nothing. Only meaningful after `Ok(())`;
    /// an error return leaves the value from the previous successful open.
    pub fn last_open_skipped(&self) -> usize {
        self.last_open_skipped
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
    pub fn set_cache_config(&mut self, cache_size: usize, preload_pages: usize) {
        self.cache_size = cache_size;
        self.preload_pages = preload_pages;
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
mod tests {
    use super::*;
    use gashuu_core::{MockPageSource, PageEntry};
    use std::io::Cursor;

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([9, 9, 9, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn mock_with(pages: usize) -> Arc<dyn PageSource> {
        let mut mock = MockPageSource::new();
        mock.expect_list_pages()
            .returning(move || vec![PageEntry { name: "p".into() }; pages]);
        mock.expect_read_bytes().returning(|_| Ok(tiny_png()));
        Arc::new(mock)
    }

    /// Build a Double-mode state (Standalone cover, Ltr) via `from_settings`,
    /// since the mode fields are private.
    fn double_state() -> ViewerState {
        ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Ltr,
            ..Default::default()
        })
    }

    #[test]
    fn empty_state_shows_nothing() {
        let state = ViewerState::new();
        assert_eq!(state.page_count(), 0);
        assert_eq!(state.index(), 0);
        assert!(state.current_spread().is_none());
        assert_eq!(state.status_text(), "No folder opened");
    }

    #[test]
    fn empty_folder_status_distinguishes_from_no_folder() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(0));
        assert_eq!(state.status_text(), "Folder contains no images");
        assert!(state.current_spread().is_none());
    }

    #[test]
    fn next_advances_and_clamps_at_last_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);
        assert!(!state.apply(NavAction::Next)); // clamped, no move
        assert_eq!(state.index(), 2);
    }

    #[test]
    fn prev_clamps_at_first_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        state.apply(NavAction::Next);
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 0);
        assert!(!state.apply(NavAction::Prev)); // clamped at 0
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn single_page_clamps_both_directions() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(1));
        assert!(!state.apply(NavAction::Next));
        assert_eq!(state.index(), 0);
        assert!(!state.apply(NavAction::Prev));
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn set_source_resets_index_to_zero() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(5));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        state.set_source(mock_with(3));
        assert_eq!(state.index(), 0);
        assert_eq!(state.page_count(), 3);
    }

    #[test]
    fn current_spread_decodes_current_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(2));
        let spread = state.current_spread().unwrap().unwrap();
        let leading = spread.leading;
        assert_eq!((leading.width(), leading.height()), (2, 3));
        assert_eq!(leading.rgba().len(), 2 * 3 * 4);
        // Single mode: no trailing page.
        assert!(spread.trailing.is_none());
    }

    #[test]
    fn current_spread_propagates_source_error() {
        let mut state = ViewerState::new();
        let mut mock = MockPageSource::new();
        mock.expect_list_pages()
            .returning(|| vec![PageEntry { name: "p".into() }; 1]);
        mock.expect_read_bytes()
            .returning(|_| Err(CoreError::IndexOutOfRange { index: 0, len: 0 }));
        state.set_source(Arc::new(mock));
        assert!(matches!(state.current_spread(), Some(Err(_))));
    }

    #[test]
    fn status_text_is_one_based() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(100));
        assert_eq!(state.status_text(), "1 / 100  [single \u{00b7} LTR]");
        state.apply(NavAction::Next);
        assert_eq!(state.status_text(), "2 / 100  [single \u{00b7} LTR]");
    }

    #[test]
    fn status_text_at_last_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.status_text(), "3 / 3  [single \u{00b7} LTR]");
    }

    #[test]
    fn with_cache_config_stores_values() {
        let state = ViewerState::with_cache_config(7, 1);
        assert_eq!(state.cache_size(), 7);
        assert_eq!(state.preload_pages(), 1);
    }

    #[test]
    fn with_cache_config_defaults_to_single_standalone_ltr() {
        let state = ViewerState::with_cache_config(7, 1);
        assert_eq!(state.spread_mode(), SpreadMode::Single);
        assert_eq!(state.cover_mode(), CoverMode::Standalone);
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
    }

    #[test]
    fn from_settings_copies_all_modes_and_cache_config() {
        let state = ViewerState::from_settings(&Settings {
            cache_size: 11,
            preload_pages: 2,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            reading_direction: ReadingDirection::Rtl,
            ..Default::default()
        });
        assert_eq!(state.cache_size(), 11);
        assert_eq!(state.preload_pages(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Double);
        assert_eq!(state.cover_mode(), CoverMode::Paired);
        assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
    }

    // ---- Double-mode (Standalone cover) navigation -------------------------

    #[test]
    fn double_standalone_navigation_advances_by_spread() {
        // 6 pages, Standalone cover: {0}{1,2}{3,4}{5}.
        let mut state = double_state();
        state.set_source(mock_with(6));
        assert_eq!(state.index(), 0);

        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 3);
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 5);
        assert!(!state.apply(NavAction::Next)); // clamp at last
        assert_eq!(state.index(), 5);

        // And back down the spreads.
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 3);
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 1);
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 0);
        assert!(!state.apply(NavAction::Prev)); // clamp at start
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn double_standalone_spread_has_trailing_for_pairs_only() {
        // 6 pages, Standalone cover: cover (0) and last odd (5) stand alone;
        // {1,2} and {3,4} have trailing pages.
        let mut state = double_state();
        state.set_source(mock_with(6));

        // Cover page 0: no trailing.
        let cover = state.current_spread().unwrap().unwrap();
        assert!(cover.trailing.is_none());

        // {1,2}: trailing present.
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());

        // {3,4}: trailing present.
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 3);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());

        // {5}: last odd page stands alone, no trailing.
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 5);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());
    }

    // ---- Toggles -----------------------------------------------------------

    #[test]
    fn toggle_spread_flips_mode_and_normalizes_index() {
        // Single mode at the last page (index 5 of 6).
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        for _ in 0..5 {
            state.apply(NavAction::Next);
        }
        assert_eq!(state.index(), 5);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        // Flip to Double: default cover is Standalone, so index 5 (last odd) is
        // already a valid leading and stays put.
        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Double);
        assert_eq!(state.index(), 5);

        // Cycle to Auto: default viewport aspect is 1.0 -> resolves to Double, so
        // index 5 is still a valid leading and stays put.
        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        assert_eq!(state.index(), 5);

        // Cycle back to Single.
        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Single);
        assert_eq!(state.index(), 5);
    }

    #[test]
    fn toggle_cover_flips_and_renormalizes() {
        // Double / Standalone at index 5 of 6.
        let mut state = double_state();
        state.set_source(mock_with(6));
        for _ in 0..3 {
            state.apply(NavAction::Next);
        }
        assert_eq!(state.index(), 5);
        assert_eq!(state.cover_mode(), CoverMode::Standalone);

        // Standalone -> Paired: pairs start even, so index 5 normalizes down to
        // the even pair start 4 ({4,5}).
        assert!(state.toggle_cover());
        assert_eq!(state.cover_mode(), CoverMode::Paired);
        assert_eq!(state.index(), 4);

        // Paired -> Standalone again: page 4 (even>0) normalizes to its pair
        // start 3 ({3,4}).
        assert!(state.toggle_cover());
        assert_eq!(state.cover_mode(), CoverMode::Standalone);
        assert_eq!(state.index(), 3);
    }

    #[test]
    fn toggle_reading_direction_flips_and_leaves_index() {
        let mut state = double_state();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

        assert!(state.toggle_reading_direction());
        assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
        assert_eq!(state.index(), 1); // pairing unaffected

        assert!(state.toggle_reading_direction());
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn toggles_are_noop_safe_with_no_folder() {
        // Toggling with no source must not panic and must leave index at 0.
        let mut state = ViewerState::new();
        assert!(state.toggle_spread());
        assert_eq!(state.index(), 0);
        assert!(state.toggle_cover());
        assert_eq!(state.index(), 0);
        assert!(state.toggle_reading_direction());
        assert_eq!(state.index(), 0);
    }

    // ---- status_text double form -------------------------------------------

    #[test]
    fn status_text_double_form_shows_range_and_label() {
        // Double / Standalone at index 1 of 6: {1,2} -> "2-3 / 6".
        let mut state = double_state();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.status_text(), "2\u{2013}3 / 6  [double \u{00b7} LTR]");
    }

    #[test]
    fn status_text_double_standalone_cover_is_single_form() {
        // Cover page in Double mode renders as a single page number.
        let mut state = double_state();
        state.set_source(mock_with(6));
        assert_eq!(state.status_text(), "1 / 6  [double \u{00b7} LTR]");
    }

    #[test]
    fn status_text_reflects_rtl_label() {
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Rtl,
            ..Default::default()
        });
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.status_text(), "2\u{2013}3 / 6  [double \u{00b7} RTL]");
    }

    // ---- Trailing-page decode failure fallback (FIX 4/5) --------------------

    #[test]
    fn current_spread_degrades_to_leading_on_trailing_decode_error() {
        // 3 pages, Double / Standalone: {0}{1,2}. Advancing once lands on the
        // {1,2} spread, whose trailing index is page 2. Make page 2 fail to
        // decode and confirm the spread degrades to leading-only with a marker.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            ..Default::default()
        });
        let mut mock = MockPageSource::new();
        mock.expect_list_pages()
            .returning(|| vec![PageEntry { name: "p".into() }; 3]);
        mock.expect_read_bytes().returning(|idx| {
            if idx == 2 {
                Err(CoreError::IndexOutOfRange { index: 2, len: 3 })
            } else {
                Ok(tiny_png())
            }
        });
        state.set_source(Arc::new(mock));

        // Advance to the {1,2} spread (leading = 1).
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);

        let images = state.current_spread().unwrap().unwrap();
        assert!(images.trailing.is_none(), "trailing should drop on error");
        assert_eq!(images.trailing_failed, Some(2));
        assert_eq!(
            (images.leading.width(), images.leading.height()),
            (2, 3),
            "leading page must still decode"
        );
    }

    // ---- Double/Paired navigation honors stored cover_mode (FIX 6) ---------

    #[test]
    fn double_paired_navigation_steps_by_two_and_clamps() {
        // 5 pages, Paired cover: {0,1}{2,3}{4}. apply() must honor the stored
        // cover_mode, stepping leading 0->2->4 forward and 4->2->0 back.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            ..Default::default()
        });
        state.set_source(mock_with(5));
        assert_eq!(state.index(), 0);

        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 4);
        assert!(!state.apply(NavAction::Next)); // clamp at last even
        assert_eq!(state.index(), 4);

        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 2);
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 0);
        assert!(!state.apply(NavAction::Prev)); // clamp at 0
        assert_eq!(state.index(), 0);
    }

    // ---- toggle_spread from Double/Paired preserves the visible page (FIX 7)

    #[test]
    fn toggle_spread_from_double_paired_keeps_index() {
        // 6 pages, Paired cover. Advance to the {2,3} spread (index 2), then
        // toggle to Auto: default viewport aspect 1.0 resolves Auto to Double, so
        // index 2 ({2,3}) is still a valid Paired leading and stays unchanged.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            ..Default::default()
        });
        state.set_source(mock_with(6));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        assert_eq!(state.index(), 2);
    }

    // ---- Auto spread mode (PR4a): resolved from viewport aspect -------------

    /// Build an Auto-mode state (Standalone cover, Ltr) via `from_settings`.
    fn auto_state() -> ViewerState {
        ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Ltr,
            ..Default::default()
        })
    }

    #[test]
    fn auto_portrait_navigates_single() {
        // Portrait viewport (aspect < 1) => Auto resolves to Single: every page
        // stands alone and navigation steps by 1.
        let mut state = auto_state();
        state.set_viewport_size(900.0, 1200.0);
        state.set_source(mock_with(5));

        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());

        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());
    }

    #[test]
    fn auto_landscape_navigates_double() {
        // Landscape viewport (aspect > 1) => Auto resolves to Double. Default
        // Standalone cover: {0}{1,2}{3,4}{5}; navigation steps 0->1->3->5.
        let mut state = auto_state();
        state.set_viewport_size(1600.0, 900.0);
        state.set_source(mock_with(6));

        // Cover (page 0) stands alone.
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());

        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 3);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 5);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());
    }

    #[test]
    fn set_viewport_size_reports_flip_and_renormalizes() {
        // Auto, landscape (Double). Advance to the {1,2} spread (index 1), then
        // go portrait (Single): the layout flips, set_viewport_size returns true,
        // and the visible page stays on screen (index 1 is a valid Single
        // leading). Going back landscape flips again and re-anchors to a valid
        // Double leading.
        let mut state = auto_state();
        // Default aspect 1.0 already resolves Auto to Double; widening to
        // landscape stays Double, so this reports no flip.
        assert!(!state.set_viewport_size(1600.0, 900.0));
        state.set_source(mock_with(6));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);

        // Landscape -> portrait: Double -> Single flips.
        assert!(state.set_viewport_size(900.0, 1200.0));
        assert_eq!(state.index(), 1); // valid Single leading, page stays visible

        // Portrait -> landscape: Single -> Double flips again; index 1 ({1,2}) is
        // a valid Standalone Double leading.
        assert!(state.set_viewport_size(1600.0, 900.0));
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn set_viewport_size_no_flip_when_not_auto() {
        // Fixed Double mode ignores the viewport aspect: a large aspect change
        // never flips the effective layout, so set_viewport_size returns false.
        let mut state = double_state();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        let before = state.index();

        assert!(!state.set_viewport_size(900.0, 1200.0)); // portrait, but mode is Double
        assert_eq!(state.index(), before);
        assert!(!state.set_viewport_size(1600.0, 900.0)); // landscape, still Double
        assert_eq!(state.index(), before);
    }

    #[test]
    fn toggle_spread_cycles_single_double_auto() {
        // Single -> Double -> Auto -> Single, each transition keeps the visible
        // page on screen (index normalized). Default viewport 1.0 => Auto=Double.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Double);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Auto);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Single);
    }

    #[test]
    fn toggle_into_auto_resolves_with_current_viewport() {
        // Portrait viewport then cycle into Auto: spread resolves to Single.
        let mut state = ViewerState::new();
        state.set_viewport_size(900.0, 1200.0);
        state.set_source(mock_with(6));
        state.toggle_spread(); // Single -> Double
        state.toggle_spread(); // Double -> Auto
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());

        // Landscape viewport then cycle into Auto: spread resolves to Double.
        let mut state = ViewerState::new();
        state.set_viewport_size(1600.0, 900.0);
        state.set_source(mock_with(6));
        state.apply(NavAction::Next); // index 1 ({1,2} once Double)
        state.toggle_spread(); // Single -> Double
        state.toggle_spread(); // Double -> Auto
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());
    }

    #[test]
    fn status_text_auto_label() {
        // Auto + landscape => "auto" label and a page RANGE (resolved Double).
        let mut state = auto_state();
        state.set_viewport_size(1600.0, 900.0);
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.status_text(), "2\u{2013}3 / 6  [auto \u{00b7} LTR]");

        // Auto + portrait => "auto" label and a single page number (Single).
        let mut state = auto_state();
        state.set_viewport_size(900.0, 1200.0);
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.status_text(), "2 / 6  [auto \u{00b7} LTR]");
    }

    #[test]
    fn set_viewport_size_flip_moves_index_via_normalize() {
        // Auto + Paired cover. Portrait resolves Auto to Single, where index 1 is a
        // valid leading; flipping to landscape (Double/Paired) makes pairs start
        // even, so normalize_leading(.., Double, Paired, 1) rounds 1 down to 0 — the
        // renormalize on flip must MOVE the index, not no-op.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Paired,
            ..Default::default()
        });
        // Default aspect 1.0 resolves Auto to Double; go portrait => Single (flips).
        assert!(state.set_viewport_size(900.0, 1200.0));
        state.set_source(mock_with(6));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);

        // Portrait -> landscape: Single -> Double/Paired flips; index 1 normalizes
        // to the even pair start 0.
        assert!(state.set_viewport_size(1600.0, 900.0));
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn toggle_spread_renormalize_moves_index() {
        // Standalone, Single mode, navigated to index 2. Toggling to Double makes
        // index 2 (even > 0) an invalid Standalone leading, so normalize_leading
        // re-anchors it down to the pair start 1 ({1,2}) — the renormalize must
        // MOVE the index.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Double);
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn auto_landscape_with_paired_cover_navigates_double() {
        // Auto + Paired + landscape => Double. 5 pages Paired: {0,1}{2,3}{4};
        // navigation steps leading 0->2->4.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Paired,
            ..Default::default()
        });
        state.set_viewport_size(1600.0, 900.0);
        state.set_source(mock_with(5));

        // Cover paired with page 1: trailing present.
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());

        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 4);
        // Last page (4) stands alone.
        assert!(state.current_spread().unwrap().unwrap().trailing.is_none());
        assert!(!state.apply(NavAction::Next)); // clamp at last
        assert_eq!(state.index(), 4);
    }

    #[test]
    fn set_viewport_size_degenerate_inputs_do_not_panic() {
        // Degenerate sizes must not panic and must not flip from the default 1.0
        // aspect (=> Double); after sanitizing, the stored aspect stays 1.0.
        let mut state = auto_state();
        state.set_source(mock_with(6));

        assert!(!state.set_viewport_size(0.0, 0.0));
        assert!(!state.set_viewport_size(f32::NAN, f32::NAN));
        // Still resolves to Double (aspect stayed 1.0): a non-cover spread pairs.
        state.apply(NavAction::Next);
        assert!(state.current_spread().unwrap().unwrap().trailing.is_some());
    }

    // ---- open_path / open_folder dispatch (PR6) -----------------------------

    #[test]
    fn open_path_nonexistent_returns_err() {
        // ArchiveLoader::open must propagate an Err for a path that does not
        // exist. This exercises the dispatch pathway and error propagation
        // without requiring tempfile or zip dev-dependencies in this crate.
        // CBZ/ZipSource correctness is covered by gashuu-core's archive_loader
        // and zip source tests.
        let mut state = ViewerState::new();
        let result = state.open_path(std::path::Path::new("/nonexistent_path_pr6_test"));
        assert!(
            result.is_err(),
            "open_path must return Err for a missing path"
        );
        // State must stay clean (no source installed) when open_path errors.
        assert_eq!(state.page_count(), 0);
        assert_eq!(state.index(), 0);
        assert!(state.current_spread().is_none());
    }

    #[test]
    fn open_folder_delegates_to_open_path() {
        // open_folder is a thin delegation wrapper; it must behave identically
        // to open_path on the same input. A nonexistent path should return Err
        // from both, leaving the state unchanged.
        let mut state_a = ViewerState::new();
        let mut state_b = ViewerState::new();
        let bad = std::path::Path::new("/nonexistent_path_pr6_delegation");
        let r_a = state_a.open_path(bad);
        let r_b = state_b.open_folder(bad);
        assert!(r_a.is_err());
        assert!(r_b.is_err());
        assert_eq!(state_a.page_count(), state_b.page_count());
    }

    #[test]
    fn last_open_skipped_is_zero_on_fresh_state() {
        // A freshly constructed ViewerState has no open in progress, so
        // last_open_skipped must start at zero.
        assert_eq!(ViewerState::new().last_open_skipped(), 0);
        assert_eq!(ViewerState::with_cache_config(10, 2).last_open_skipped(), 0);
        assert_eq!(
            ViewerState::from_settings(&Settings::default()).last_open_skipped(),
            0
        );
    }

    #[test]
    fn last_open_skipped_stays_zero_on_open_error() {
        // An open_path error must not update last_open_skipped; it stays 0.
        let mut state = ViewerState::new();
        let _ = state.open_path(std::path::Path::new("/nonexistent_path_pr6_skip"));
        assert_eq!(state.last_open_skipped(), 0);
    }

    // ---- open_path CBR/RAR dispatch (PR7) -----------------------------------

    #[test]
    fn open_path_nonexistent_cbr_returns_err_and_leaves_clean_state() {
        // A .cbr path that does not exist must propagate an error through
        // ArchiveLoader::open and leave ViewerState in its default (no-source)
        // state — no panic, no partial initialization. This locks in that the
        // .cbr extension routes through the same graceful error-handling path as
        // .cbz/.zip (tested above). Real CBR/RarSource extraction correctness is
        // owned by gashuu-core's rar.rs/archive_loader.rs tests; this crate
        // deliberately carries no tempfile/zip/rar dev-dependency.
        let mut state = ViewerState::new();
        let result = state.open_path(std::path::Path::new("/nonexistent_path_pr7_cbr_test.cbr"));
        assert!(
            result.is_err(),
            "open_path must return Err for a missing .cbr path"
        );
        assert_eq!(state.page_count(), 0, "page_count must stay 0 after error");
        assert_eq!(state.index(), 0, "index must stay 0 after error");
        assert!(
            state.current_spread().is_none(),
            "current_spread must be None after error"
        );
        assert_eq!(
            state.last_open_skipped(),
            0,
            "last_open_skipped must not update on error"
        );
    }

    // ---- current_source() (PR8a) ---------------------------------------------

    #[test]
    fn current_source_is_none_before_open() {
        // A freshly constructed ViewerState has no source installed yet.
        let state = ViewerState::new();
        assert!(
            state.current_source().is_none(),
            "current_source must be None before any open"
        );
    }

    #[test]
    fn current_source_is_some_after_set_source() {
        // After set_source the Arc is retained and current_source returns Some.
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        assert!(
            state.current_source().is_some(),
            "current_source must be Some after set_source"
        );
    }

    #[test]
    fn current_source_is_none_after_failed_open_path() {
        // A failed open_path must NOT install a source; current_source stays None.
        let mut state = ViewerState::new();
        let _ = state.open_path(std::path::Path::new("/nonexistent_pr8a_source"));
        assert!(
            state.current_source().is_none(),
            "current_source must remain None after a failed open_path"
        );
    }

    // ---- jump_to() (PR8a) ---------------------------------------------------

    #[test]
    fn jump_to_no_source_returns_false() {
        // With no source loaded jump_to must be a no-op.
        let mut state = ViewerState::new();
        assert!(
            !state.jump_to(0),
            "jump_to must return false with no source"
        );
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn jump_to_current_leading_returns_false() {
        // Jumping to the page already at the current leading is a no-op.
        let mut state = ViewerState::new();
        state.set_source(mock_with(5));
        // Default: Single mode, index 0 is the leading.
        assert!(
            !state.jump_to(0),
            "jump_to current leading must return false"
        );
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn jump_to_out_of_range_clamps() {
        // An out-of-range page is clamped to a valid leading without panic.
        let mut state = ViewerState::new();
        state.set_source(mock_with(4));
        // Single mode: every page is its own leading; page_count - 1 = 3.
        // Jumping to page_count + 5 = 9 must clamp to the last page (3).
        let moved = state.jump_to(9);
        assert!(moved, "jump_to out-of-range must move when index differs");
        assert_eq!(state.index(), 3, "clamped to last valid single leading");
    }

    #[test]
    fn jump_to_single_mode_lands_on_exact_page() {
        // In Single mode every page is its own leading; jump_to should land there.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        assert!(state.jump_to(4));
        assert_eq!(state.index(), 4);
    }

    #[test]
    fn jump_to_double_standalone_trailing_normalizes_to_leading() {
        // Double / Standalone: {0}{1,2}{3,4}{5}.
        // Clicking page 2 (trailing of the {1,2} spread) should land on leading 1.
        let mut state = double_state();
        state.set_source(mock_with(6));
        let moved = state.jump_to(2);
        assert!(moved, "jump_to trailing must move from cover (index 0)");
        assert_eq!(
            state.index(),
            1,
            "trailing page 2 must normalize to leading 1"
        );
    }

    #[test]
    fn jump_to_double_standalone_trailing_page4_normalizes_to_leading3() {
        // Double / Standalone: {0}{1,2}{3,4}{5}.
        // Clicking page 4 (trailing of {3,4}) should land on leading 3.
        let mut state = double_state();
        state.set_source(mock_with(6));
        assert!(state.jump_to(4));
        assert_eq!(state.index(), 3);
    }

    #[test]
    fn jump_to_double_paired_trailing_normalizes_to_leading() {
        // Double / Paired cover: {0,1}{2,3}{4,5}.
        // Clicking page 1 (trailing of {0,1}) should land on leading 0.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            reading_direction: ReadingDirection::Ltr,
            ..Default::default()
        });
        state.set_source(mock_with(6));
        // index starts at 0; jump_to(1) should normalize to 0 => no move.
        assert!(
            !state.jump_to(1),
            "trailing page 1 normalizes to leading 0, no move from current 0"
        );
        assert_eq!(state.index(), 0);

        // Now jump_to the trailing page 3 of spread {2,3} -> leading 2.
        assert!(state.jump_to(3));
        assert_eq!(state.index(), 2);
    }

    #[test]
    fn jump_to_double_paired_trailing_page5_normalizes_to_leading4() {
        // Double / Paired cover on 6 pages: {0,1}{2,3}{4,5}.
        // Clicking page 5 (trailing of {4,5}) should land on leading 4.
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            reading_direction: ReadingDirection::Ltr,
            ..Default::default()
        });
        state.set_source(mock_with(6));
        assert!(state.jump_to(5));
        assert_eq!(state.index(), 4);
    }

    // ---- set_spread_mode (PR8b) ---------------------------------------------

    #[test]
    fn set_spread_mode_to_double_renormalizes_index() {
        // Single mode at index 2 of 6. Switching to Double / Standalone makes
        // index 2 (even > 0) an invalid Standalone leading, so renormalize_index
        // re-anchors it to the pair start 1 ({1,2}). set_spread_mode returns true.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        assert!(state.set_spread_mode(SpreadMode::Double));
        assert_eq!(state.spread_mode(), SpreadMode::Double);
        // index 2 normalized to valid Standalone Double leading 1.
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn set_spread_mode_same_value_is_noop() {
        // Calling set_spread_mode with the already-active mode must return false
        // and leave index unchanged.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        assert!(!state.set_spread_mode(SpreadMode::Single));
        assert_eq!(state.spread_mode(), SpreadMode::Single);
        assert_eq!(state.index(), 2);
    }

    // ---- set_cover_mode (PR8b) ----------------------------------------------

    #[test]
    fn set_cover_mode_flips_and_renormalizes() {
        // Double / Standalone at index 5 of 6. Switching to Paired makes pairs
        // start even, so index 5 normalizes down to the even pair start 4 ({4,5}).
        let mut state = double_state();
        state.set_source(mock_with(6));
        for _ in 0..3 {
            state.apply(NavAction::Next);
        }
        assert_eq!(state.index(), 5);
        assert_eq!(state.cover_mode(), CoverMode::Standalone);

        assert!(state.set_cover_mode(CoverMode::Paired));
        assert_eq!(state.cover_mode(), CoverMode::Paired);
        assert_eq!(state.index(), 4);

        // Setting it back to Standalone: page 4 (even>0) normalizes to pair
        // start 3 ({3,4}) in Standalone Double.
        assert!(state.set_cover_mode(CoverMode::Standalone));
        assert_eq!(state.cover_mode(), CoverMode::Standalone);
        assert_eq!(state.index(), 3);
    }

    #[test]
    fn set_cover_mode_same_value_is_noop() {
        // Calling set_cover_mode with the already-active mode must return false
        // and leave index unchanged.
        let mut state = double_state();
        state.set_source(mock_with(6));
        for _ in 0..3 {
            state.apply(NavAction::Next);
        }
        assert_eq!(state.index(), 5);
        assert_eq!(state.cover_mode(), CoverMode::Standalone);

        assert!(!state.set_cover_mode(CoverMode::Standalone));
        assert_eq!(state.cover_mode(), CoverMode::Standalone);
        assert_eq!(state.index(), 5);
    }

    // ---- set_reading_direction (PR8b) ----------------------------------------

    #[test]
    fn set_reading_direction_flips_and_leaves_index() {
        // Double / Standalone, Ltr. Switching to Rtl returns true; pairing is
        // direction-agnostic so index must remain unchanged.
        let mut state = double_state();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

        assert!(state.set_reading_direction(ReadingDirection::Rtl));
        assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
        assert_eq!(state.index(), 1);

        assert!(state.set_reading_direction(ReadingDirection::Ltr));
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn set_reading_direction_same_value_is_noop() {
        // Calling set_reading_direction with the already-active direction must
        // return false and leave index unchanged.
        let mut state = double_state();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 1);
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

        assert!(!state.set_reading_direction(ReadingDirection::Ltr));
        assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
        assert_eq!(state.index(), 1);
    }

    // ---- set_cache_config (PR8b) ---------------------------------------------

    #[test]
    fn set_spread_mode_to_auto_landscape_renormalizes_like_double() {
        // Single mode at index 2 of 6. Landscape viewport (aspect > 1) means
        // Auto resolves to Double. Switching from Single to Auto triggers
        // renormalize_index under Double/Standalone semantics: index 2 (even > 0)
        // is NOT a valid Standalone Double leading, so it re-anchors to the pair
        // start 1 ({1,2}). set_spread_mode returns true (Single != Auto).
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        // Landscape viewport: Auto resolves to Double.
        state.set_viewport_size(1600.0, 900.0);

        assert!(state.set_spread_mode(SpreadMode::Auto));
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        // Index 2 normalized to valid Standalone Double leading 1 ({1,2}).
        assert_eq!(state.index(), 1);
    }

    #[test]
    fn set_spread_mode_to_auto_portrait_preserves_index() {
        // Single mode at index 2 of 6. Portrait viewport (aspect < 1) means
        // Auto resolves to Single, where every page is its own valid leading.
        // Switching from Single to Auto still returns true (different enum
        // values), triggers renormalize_index, but index 2 is already a valid
        // Single leading so it stays unchanged.
        let mut state = ViewerState::new();
        state.set_source(mock_with(6));
        state.apply(NavAction::Next);
        state.apply(NavAction::Next);
        assert_eq!(state.index(), 2);
        assert_eq!(state.spread_mode(), SpreadMode::Single);

        // Portrait viewport: Auto resolves to Single.
        state.set_viewport_size(900.0, 1200.0);

        assert!(state.set_spread_mode(SpreadMode::Auto));
        assert_eq!(state.spread_mode(), SpreadMode::Auto);
        // Index 2 is a valid Single leading; renormalize is idempotent here.
        assert_eq!(state.index(), 2);
    }

    #[test]
    fn set_cover_mode_preserves_valid_leading() {
        // Double / Standalone at index 0 (the cover). Switching to Paired: in
        // Paired, pairs are {0,1}{2,3}{4,5}, so index 0 is already a valid
        // Paired leading. set_cover_mode returns true (mode changed) but index
        // stays 0 (renormalize is idempotent on an already-valid leading).
        // This complements the existing cover test that shows index DOES move
        // when the old index is not a valid leading under the new mode.
        let mut state = double_state();
        state.set_source(mock_with(6));
        assert_eq!(state.index(), 0);
        assert_eq!(state.cover_mode(), CoverMode::Standalone);

        assert!(state.set_cover_mode(CoverMode::Paired));
        assert_eq!(state.cover_mode(), CoverMode::Paired);
        // Index 0 is a valid Paired leading; renormalize must leave it at 0.
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn set_cache_config_updates_fields() {
        // Calling set_cache_config must update the fields that set_source reads
        // on the next open. This pins that a settings dialog can store new
        // cache/preload values in the ViewerState so a subsequently opened book
        // picks them up without requiring an app relaunch.
        let mut state = ViewerState::new();
        state.set_cache_config(99, 7);
        assert_eq!(state.cache_size(), 99);
        assert_eq!(state.preload_pages(), 7);
    }
}
