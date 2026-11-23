//! Viewer session, spread navigation and display preferences.
//! The backend owns page data. Navigation determines the leading and trailing
//! indices in reading order, and the UI places them using the reading direction.
use crate::backend::{BookBackend, OpenedBook, SessionId};
use crate::keymap::NavAction;
use crate::viewport::ViewportState;
use gashuu_core::{
    BookId, CacheConfig, CoverMode, DecodedImage, Language, ReadingDirection, ResolvedView,
    Settings, Spread, SpreadMode, SpreadNavigation,
};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

pub use gashuu_core::scrub_fraction_to_page;

/// Discriminates the three content shapes of the viewer status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusKind {
    /// No source has been opened yet.
    NoFolder,
    /// A source is open but contains no displayable images.
    NoImages,
    /// A source is open and has pages; spread info applies.
    Pages,
}

/// Language-free content produced by [`ViewerState::status_content`].
/// Formatted into a localized string by `i18n::dynamic::format_status`.
#[derive(Debug, Clone)]
pub(crate) struct StatusContent {
    /// Page range string, e.g. `"2\u{2013}3 / 6"` or `"1 / 100"`.
    /// Empty when `kind` is `NoFolder` or `NoImages`.
    pub(crate) pages: String,
    pub(crate) spread: SpreadMode,
    pub(crate) direction: ReadingDirection,
    pub(crate) kind: StatusKind,
}

/// Non-blocking cache classification for the current spread.
///
/// Each slot carries its page index plus `Some(decoded)` on a cache HIT or
/// `None` on a cache MISS. MISS never reads or decodes.
pub(crate) struct SpreadCacheState {
    pub(crate) leading: (usize, Option<Arc<DecodedImage>>),
    pub(crate) trailing: Option<(usize, Option<Arc<DecodedImage>>)>,
    pub(crate) single: bool,
}

/// Holds the active session, book metadata, and presentation settings.
/// Spread pairing and its always-valid-leading invariant live in `nav`.
pub struct ViewerState {
    session: Option<SessionId>,
    nav: SpreadNavigation,
    cache_config: CacheConfig,
    reading_direction: ReadingDirection,
    /// UI display language, mirrored from the global `Settings` (the same
    /// dual-write `cache_config` uses: the settings handler updates both).
    /// Retained for the language getter; the status line itself is now language-free
    /// (`status_content`) and localized by `i18n::dynamic::format_status`.
    language: Language,
    /// Entries skipped by the most recent book installed via `install_opened`.
    /// Zero until a book is installed or when the last listing skipped nothing.
    /// A failed open never reaches installation, so the previous value is retained.
    last_open_skipped: usize,
    /// Whether an error cut short the page listing of the most recent book
    /// installed via `install_opened`. False until a book is installed or when
    /// the last listing completed normally. A failed open never reaches
    /// installation, so the previous value is retained.
    last_open_truncated: bool,
    /// Identity and display title installed by a successful open.
    open_book: Option<BookId>,
    open_title: Option<String>,
}

// A 1600x2400 RGBA page takes about 15 MB. The #651 Chrome spike reached
// about 1.5 GB for a 348 MB book with a 50-page cache.
const WEB_CACHE_CAPACITY: usize = 6;
// Six typical pages fit under this cap, which also bounds much larger pages.
const WEB_MAX_CACHE_BYTES: u64 = 128 * 1024 * 1024;

fn platform_cache_config(requested: CacheConfig, is_web: bool) -> CacheConfig {
    if is_web {
        CacheConfig::new(requested.capacity().min(WEB_CACHE_CAPACITY), 0)
            .with_max_bytes(requested.max_bytes().min(WEB_MAX_CACHE_BYTES))
    } else {
        requested
    }
}

impl ViewerState {
    pub fn new() -> Self {
        Self::with_cache_config(CacheConfig::default())
    }

    /// Construct with explicit cache config and fixed display modes
    /// (Single / Standalone / Ltr — deliberate pins, NOT the `Settings` defaults)
    /// so callers that only care about cache sizing get single-page behavior.
    pub fn with_cache_config(cache_config: CacheConfig) -> Self {
        Self {
            session: None,
            nav: SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone),
            cache_config: platform_cache_config(cache_config, cfg!(target_arch = "wasm32")),
            reading_direction: ReadingDirection::Ltr,
            language: Language::default(),
            last_open_skipped: 0,
            last_open_truncated: false,
            open_book: None,
            open_title: None,
        }
    }

    /// Construct from persisted `Settings`, copying both the cache config and the
    /// display modes.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            session: None,
            nav: SpreadNavigation::new(settings.spread_mode, settings.cover_mode),
            cache_config: platform_cache_config(
                settings.cache_config(),
                cfg!(target_arch = "wasm32"),
            ),
            reading_direction: settings.reading_direction,
            language: settings.language,
            last_open_skipped: 0,
            last_open_truncated: false,
            open_book: None,
            open_title: None,
        }
    }

    /// Close the current book, preserving the configured display and cache modes.
    #[must_use]
    pub fn close(&mut self) -> Option<SessionId> {
        self.nav.set_total(0);
        self.last_open_skipped = 0;
        self.last_open_truncated = false;
        self.open_book = None;
        self.open_title = None;
        self.session.take()
    }

    pub(crate) fn session(&self) -> Option<SessionId> {
        self.session
    }

    /// Jump to the spread containing `page`. The target is normalized to a valid
    /// spread leading for the current modes (clicking a trailing member lands on
    /// its spread start), so `index` stays a valid leading. No-op (returns
    /// `false`) when no source is loaded or the resolved leading equals the
    /// current index. Out-of-range `page` is clamped.
    pub fn jump_to(&mut self, page: usize) -> bool {
        self.nav.jump_to(page)
    }

    /// The spread the scrubber preview/commit would land on for `page`: clamps,
    /// normalizes to the containing spread for the CURRENT modes, and returns it.
    /// `None` when no source is loaded. Does NOT move the index.
    pub fn preview_spread(&self, page: usize) -> Option<Spread> {
        self.nav.preview_spread(page)
    }

    /// Whether a hypothetical jump to `page` would land on a double-page spread
    /// (used by the scrubber preview to show 1 vs 2 thumbnails WITHOUT changing
    /// the current index). Returns `false` when no source is loaded.
    pub fn preview_is_double(&self, page: usize) -> bool {
        self.nav.preview_is_double(page)
    }

    pub(crate) fn cache_config(&self) -> CacheConfig {
        self.cache_config
    }

    /// Install the book metadata and return the session its owner must close.
    #[must_use]
    pub(crate) fn install_opened(&mut self, opened: &OpenedBook) -> Option<SessionId> {
        self.last_open_skipped = opened.skipped;
        self.last_open_truncated = opened.listing_truncated;
        self.nav.set_total(opened.page_count);
        self.open_book = Some(opened.id.clone());
        self.open_title = Some(gashuu_core::display_title(
            Path::new(opened.id.as_str()),
            opened.is_dir,
        ));
        self.session.replace(opened.session)
    }

    /// Entries skipped by the most recent book installed via `install_opened`.
    /// Zero until a book is installed or when the last listing skipped nothing.
    /// A failed open never reaches installation, so the previous value is retained.
    pub fn last_open_skipped(&self) -> usize {
        self.last_open_skipped
    }

    /// Whether an error cut short the page listing of the most recent book
    /// installed via `install_opened`. False until a book is installed or when
    /// the last listing completed normally. A failed open never reaches
    /// installation, so the previous value is retained.
    pub fn last_open_truncated(&self) -> bool {
        self.last_open_truncated
    }

    /// The ID installed by the most recent successful open, if still active.
    pub fn open_book(&self) -> Option<&BookId> {
        self.open_book.as_ref()
    }

    pub fn open_title(&self) -> Option<&str> {
        self.open_title.as_deref()
    }

    #[cfg(test)]
    pub fn open_path(&mut self, path: &Path) -> Result<(), gashuu_core::CoreError> {
        let source = gashuu_core::ArchiveLoader::open_with_policy(path, Default::default())?;
        let id = BookId::from_canonical_path(&gashuu_core::canonical_identity(path))?;
        let _ = self.install_opened(&OpenedBook {
            session: Self::test_session(),
            id,
            page_count: source.list_pages().len(),
            skipped: source.skipped_count(),
            listing_truncated: source.listing_truncated(),
            is_dir: path.is_dir(),
        });
        Ok(())
    }

    #[cfg(test)]
    fn test_session() -> SessionId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(u64::MAX);
        SessionId::for_test(NEXT.fetch_sub(1, Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn install_test_book(&mut self, page_count: usize) -> Option<SessionId> {
        self.nav.set_total(page_count);
        self.open_book = None;
        self.open_title = None;
        self.session.replace(Self::test_session())
    }

    /// Total page count of the current source (0 when none is open). Used by the
    /// thumbnail-strip wiring in `app.rs` to size the placeholder model, and by
    /// the unit tests below.
    pub fn page_count(&self) -> usize {
        self.nav.total()
    }

    /// The current source's page count as a domain-typed `Option`, mirroring the
    /// core's `Book::page_count_opt` convention: the `0` "no book / no pages"
    /// sentinel maps to `None`, any positive count to `Some(NonZeroUsize)`. This
    /// types the rule the methods below guard on so callers never re-interpret the
    /// raw `0`. (`page_count()` is kept for the Slint boundary, which needs the
    /// plain count.)
    pub(crate) fn page_count_opt(&self) -> Option<NonZeroUsize> {
        self.nav.total_nonzero()
    }

    /// The current spread's leading page index. Used by the highlight wiring in
    /// `main.rs` (`refresh` pushes it into `current-index`), and by the unit
    /// tests below.
    pub fn index(&self) -> usize {
        self.nav.index()
    }

    /// The page index to persist as the resume position: the last page index when
    /// the CURRENT spread contains the final page (so a finished book reads 100%),
    /// else the spread leading. `index()` when no source.
    pub fn resume_index_to_persist(&self) -> usize {
        self.nav.resume_index_to_persist()
    }

    /// Apply a navigation action a spread at a time. Returns true if the leading
    /// index moved. In Single mode this is the old ±1 clamp; in Double mode it
    /// advances/retreats one spread (skipping the partner page). Auto first
    /// resolves to Single or Double from the current viewport aspect before the
    /// navigation step.
    pub fn apply(&mut self, action: NavAction) -> bool {
        match action {
            NavAction::Next => self.nav.next(),
            NavAction::Prev => self.nav.prev(),
        }
    }

    /// Return a non-blocking classification of the current spread.
    ///
    /// `None` means no source or an empty source. `Some` never decodes on cache
    /// MISS; callers must dispatch missing slots separately.
    pub(crate) fn classify_spread(&self, backend: &dyn BookBackend) -> Option<SpreadCacheState> {
        let session = self.session?;
        let s = self.nav.spread()?;
        let leading = (s.leading, backend.cached_page(session, s.leading));
        let trailing = s.trailing.map(|t| (t, backend.cached_page(session, t)));
        Some(SpreadCacheState {
            leading,
            trailing,
            single: s.trailing.is_none(),
        })
    }

    #[cfg(test)]
    pub(crate) fn current_spread(&self) -> Option<Spread> {
        self.nav.spread()
    }

    /// Cycle Single -> Double -> Auto -> Single, then re-normalizes the index
    /// (via the effective layout) so the currently visible page stays on screen.
    /// Always returns `true` (a 3-cycle always changes the configured mode).
    pub fn toggle_spread(&mut self) -> bool {
        let next = match self.nav.spread_mode() {
            SpreadMode::Single => SpreadMode::Double,
            SpreadMode::Double => SpreadMode::Auto,
            SpreadMode::Auto => SpreadMode::Single,
        };
        self.nav.set_spread_mode(next)
    }

    /// Update the window aspect ratio used to resolve `SpreadMode::Auto`. Returns
    /// `true` iff the EFFECTIVE layout changed (only possible in Auto, crossing
    /// the square boundary). On a change, re-normalizes the index so the
    /// currently visible page stays on screen. A degenerate window size — any
    /// `width`/`height` whose ratio is non-finite or non-positive — falls back to
    /// aspect `1.0`, so the navigation value object always holds a valid ratio.
    /// No-op-safe with no folder open (index stays 0).
    pub fn set_viewport_size(&mut self, width: f32, height: f32) -> bool {
        self.nav.set_viewport_size(width, height)
    }

    /// Flip Standalone <-> Paired cover, then re-normalize the index so the
    /// currently visible page stays on screen. Returns `true` when the cover
    /// changed; a 2-variant toggle always flips, so this is currently always
    /// `true`. The bool is retained for forward-compatibility with multi-valued
    /// modes (e.g. a future `CoverMode` variant).
    pub fn toggle_cover(&mut self) -> bool {
        let next = match self.nav.cover_mode() {
            CoverMode::Standalone => CoverMode::Paired,
            CoverMode::Paired => CoverMode::Standalone,
        };
        self.nav.set_cover_mode(next)
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
    /// Re-anchors the index inside `SpreadNavigation` so the visible page stays
    /// on screen. Idempotent: returns `false` (no change) when already set.
    pub fn set_spread_mode(&mut self, mode: SpreadMode) -> bool {
        self.nav.set_spread_mode(mode)
    }

    /// Set the cover mode to an exact value (vs. `toggle_cover`'s flip).
    /// Re-anchors the index inside `SpreadNavigation` so the visible page stays
    /// on screen. Idempotent: returns `false` (no change) when already set.
    pub fn set_cover_mode(&mut self, mode: CoverMode) -> bool {
        self.nav.set_cover_mode(mode)
    }

    /// Update the cache configuration used the NEXT time a book is opened.
    ///
    /// This deliberately does NOT rebuild the current book's cache — the new
    /// values are consumed by the backend on the next open, matching the
    /// "applies to newly opened books" contract surfaced in the settings
    /// dialog. (Immediate runtime rebuild of the current cache is deferred.)
    pub fn set_cache_config(&mut self, cache_config: CacheConfig) {
        self.cache_config = platform_cache_config(cache_config, cfg!(target_arch = "wasm32"));
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

    /// Mirror the UI display language from the global `Settings` (set by the
    /// settings-dialog language handler, never per-book). The status line is now
    /// localized externally via `i18n::dynamic`, so this only keeps the getter in
    /// step. Idempotent: returns `false` when already set — the dropdown's
    /// selection self-fire is absorbed by this guard.
    pub fn set_language(&mut self, lang: Language) -> bool {
        if self.language == lang {
            return false;
        }
        self.language = lang;
        true
    }

    /// The UI display language currently mirrored into this state.
    ///
    /// The status line and Rust-composed strings are now localized externally
    /// via `i18n::dynamic` (driven by the `Localizer`'s `FluentLanguageLoader`),
    /// so this getter has no production caller after the Fluent migration. It is
    /// retained as the read side of the mirrored `language` field — written by
    /// `set_language` whose "changed" return still gates the language dropdown's
    /// self-fire in `handlers/settings.rs`. `#[allow(dead_code)]` follows the same convention
    /// as the other currently-unwired accessors in this module.
    #[allow(dead_code)]
    pub fn language(&self) -> Language {
        self.language
    }

    /// Apply a fully resolved per-book view to the runtime: the three
    /// ViewerState-owned modes AND the viewport-owned fit. Fit stays owned by
    /// ViewportState; this method is the one place the pairing happens.
    pub fn apply_resolved_view(&mut self, view: ResolvedView, viewport: &mut ViewportState) {
        self.set_reading_direction(view.reading_direction);
        self.set_spread_mode(view.spread_mode);
        self.set_cover_mode(view.cover_mode);
        viewport.set_fit(view.fit_mode);
    }

    pub fn reading_direction(&self) -> ReadingDirection {
        self.reading_direction
    }

    pub fn spread_mode(&self) -> SpreadMode {
        self.nav.spread_mode()
    }

    pub fn cover_mode(&self) -> CoverMode {
        self.nav.cover_mode()
    }

    /// Return a language-free description of the current status line.
    /// Format it via `i18n::dynamic::format_status(loader, &content)`.
    pub fn status_content(&self) -> StatusContent {
        // `spread`/`direction` are the same in every arm; only `pages`/`kind`
        // vary, so compute that pair per-arm and build the struct once.
        let (pages, kind) = match (self.session, self.nav.spread()) {
            (None, _) => (String::new(), StatusKind::NoFolder),
            (Some(_), None) => (String::new(), StatusKind::NoImages),
            (Some(_), Some(s)) => {
                let pages = if let Some(t) = s.trailing {
                    format!("{}\u{2013}{} / {}", s.leading + 1, t + 1, self.nav.total())
                } else {
                    format!("{} / {}", s.leading + 1, self.nav.total())
                };
                (pages, StatusKind::Pages)
            }
        };
        StatusContent {
            pages,
            spread: self.nav.spread_mode(),
            direction: self.reading_direction,
            kind,
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
