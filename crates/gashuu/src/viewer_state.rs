//! Presentation-layer view state: which folder is open and the current spread.
//! Backed by `ImageCache` (LRU + background prefetch) since PR2; drives a
//! two-page spread (Single/Double, Standalone/Paired cover, LTR/RTL) since PR4.
//!
//! The pure page-pairing math lives in `gashuu_core::spread` and is
//! reading-direction-agnostic: it decides WHICH pages form a spread (in reading
//! order); this layer owns the modes, the cache, and the navigation actions.
//! Placement (which side each page renders on) is resolved in the UI from
//! `reading_direction`; this state only exposes the leading/trailing images.

use crate::keymap::NavAction;
use gashuu_core::{
    next_leading, normalize_leading, prev_leading, spread_at, CoreError, CoverMode, DecodedImage,
    FolderSource, ImageCache, PageSource, ReadingDirection, Settings, SpreadMode, DEFAULT_CAPACITY,
    DEFAULT_PREFETCH_RADIUS,
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

    /// Apply a navigation action a spread at a time. Returns true if the leading
    /// index moved. In Single mode this is the old ±1 clamp; in Double mode it
    /// advances/retreats one spread (skipping the partner page).
    pub fn apply(&mut self, action: NavAction) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let next = match action {
            NavAction::Next => next_leading(
                self.page_count,
                self.spread_mode,
                self.cover_mode,
                self.index,
            ),
            NavAction::Prev => prev_leading(
                self.page_count,
                self.spread_mode,
                self.cover_mode,
                self.index,
            ),
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
            self.spread_mode,
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

    /// Flip Single <-> Double, then re-normalize the index so the currently
    /// visible page stays on screen. Returns `true` when the mode changed; a
    /// 2-variant toggle always flips, so this is currently always `true`. The
    /// bool is retained for forward-compatibility with multi-valued modes (e.g.
    /// a future `SpreadMode::Auto`).
    pub fn toggle_spread(&mut self) -> bool {
        let before = self.spread_mode;
        self.spread_mode = match before {
            SpreadMode::Single => SpreadMode::Double,
            SpreadMode::Double => SpreadMode::Single,
        };
        self.renormalize_index();
        before != self.spread_mode
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
            self.spread_mode,
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
                    self.spread_mode,
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

        // Flip back to Single.
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
        // toggle to Single: the mode flips and the index is unchanged (Single
        // normalize is identity).
        let mut state = ViewerState::from_settings(&Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            ..Default::default()
        });
        state.set_source(mock_with(6));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);

        assert!(state.toggle_spread());
        assert_eq!(state.spread_mode(), SpreadMode::Single);
        assert_eq!(state.index(), 2);
    }
}
