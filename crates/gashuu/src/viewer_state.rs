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
    next_leading, normalize_leading, prev_leading, spread_at, CoreError, CoverMode, DecodedImage,
    FolderSource, ImageCache, PageSource, ReadingDirection, Settings, SpreadLayout, SpreadMode,
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
            page_count: 0,
            index: 0,
            cache_size,
            preload_pages,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            reading_direction: ReadingDirection::Ltr,
            viewport_aspect: 1.0,
        }
    }

    /// Construct from persisted `Settings`, copying both the cache config and the
    /// display modes.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            cache: None,
            page_count: 0,
            index: 0,
            cache_size: settings.cache_size,
            preload_pages: settings.preload_pages,
            spread_mode: settings.spread_mode,
            cover_mode: settings.cover_mode,
            reading_direction: settings.reading_direction,
            viewport_aspect: 1.0,
        }
    }

    /// Replace the active source (used by `open_folder` and by tests). Wraps the
    /// source in a fresh `ImageCache`, discarding any previously cached pages and
    /// resetting to the first page.
    pub fn set_source(&mut self, source: Arc<dyn PageSource>) {
        let cache = ImageCache::new(source, self.cache_size, self.preload_pages);
        self.page_count = cache.len();
        self.cache = Some(cache);
        self.index = 0;
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

    /// Open a folder as the active source, resetting to the first page.
    pub fn open_folder(&mut self, path: &Path) -> Result<(), CoreError> {
        let source = FolderSource::open(path)?;
        let skipped = source.skipped_count();
        if skipped > 0 {
            tracing::warn!(skipped, "skipped unreadable entries while opening folder");
        }
        self.set_source(Arc::new(source));
        Ok(())
    }

    // Exercised by the unit tests below; dead_code fires only because #[cfg(test)]
    // callers are invisible to the lint. The UI layer will use these in a later PR.
    #[allow(dead_code)]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    #[allow(dead_code)]
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
        mock.expect_list_pages().returning(move || {
            vec![
                PageEntry {
                    path: "p".into(),
                    name: "p".into()
                };
                pages
            ]
        });
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
        mock.expect_list_pages().returning(|| {
            vec![
                PageEntry {
                    path: "p".into(),
                    name: "p".into()
                };
                1
            ]
        });
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
        mock.expect_list_pages().returning(|| {
            vec![
                PageEntry {
                    path: "p".into(),
                    name: "p".into()
                };
                3
            ]
        });
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
}
