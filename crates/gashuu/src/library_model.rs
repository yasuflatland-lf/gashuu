//! Pure (Slint-free) mapping from the headless `Library` to the carousel's
//! per-book display rows.
//!
//! The Slint `CarouselItem` carries an `image` (a `!Send`, backend-dependent
//! `slint::Image`) and is awkward to build in a headless unit test, so the
//! derivable display data lives in this plain `CarouselData` struct, table-
//! tested here. `carousel.rs`'s `to_carousel_item` adapter turns each row into a
//! `CarouselItem` on the UI thread (placeholder cover for PR-C; real covers
//! stream in via PR-V). This is the SINGLE place the Library → carousel
//! display mapping lives (mirrors the "one chokepoint maps domain → display
//! row" discipline of the private `thumbnail_item` fn in `thumbnail_strip.rs`).
//!
//! Progress is derived from `Book::progress()` which returns a `ReadingProgress`
//! value object. `ReadingProgress::current()` is 1-based (`reached + 1`,
//! saturating, >= 1); `ReadingProgress::fraction()` guards `total == 0` to
//! `0.0` (no NaN/inf); `ReadingProgress::total()` is the persisted page count.

use gashuu_core::{Book, Library};

/// One carousel row's display data, derived from a `Book` in the `Library`.
/// Plain data only (no `slint::Image`) so the derivation is unit-testable
/// without a display backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CarouselData {
    /// Book display title (file stem / directory name; from `Book::title`).
    pub title: String,
    /// 1-based current page for display = `ReadingProgress::current()` (`reached + 1`,
    /// saturating). A fresh book (`reached == 0`) shows `1`.
    pub current: i32,
    /// Total page count for display = `ReadingProgress::total() -> Option<usize>` mapped
    /// through `Book::page_count_opt()`. `None` (unknown) is displayed as `0` until the
    /// book has been opened at least once; back-filled and saved on open (see
    /// `set_page_count` in the open path), so an opened book shows its real total
    /// and a `ReadingProgress::fraction()`-based progress bar.
    pub total: i32,
    /// Reading progress in `0.0..=1.0` = `ReadingProgress::fraction()` (`0.0` when
    /// `total == 0`, never NaN/inf). Ambient per-cover bar; accent fill, green when `>= 1.0`.
    pub progress: f32,
    /// Derived availability (`Library::is_available`): false when the book's
    /// path no longer resolves. Unavailable books stay in the shelf, rendered
    /// grayed with a broken-cover placeholder.
    pub available: bool,
    /// True when this book is the last-opened book (`book.path() == library.last_opened()`).
    /// Drives the BookmarkRibbon overlay on the cover card. Pure derivation — no I/O.
    pub bookmarked: bool,
}

/// Case-insensitive substring match of `query` against a book's display title
/// and its filesystem path. An empty query matches every book (no filter).
///
/// The query is matched verbatim (not trimmed): callers pass the exact debounced
/// search text, so leading/trailing whitespace is significant by design.
pub(crate) fn book_matches(book: &Book, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let needle = query.to_lowercase();
    let title = book.title().to_lowercase();
    let path = book.path().to_string_lossy().to_lowercase();

    title.contains(&needle) || path.contains(&needle)
}

/// Derive one carousel display row from a single `Book`.
///
/// Each row is derived from `Book::progress()`, which returns a
/// `ReadingProgress` value object. `current = progress.current()` (1-based,
/// `>= 1`, saturating); `progress = progress.fraction()` is guarded so
/// `total == 0` yields `0.0` (never NaN/inf); `total` comes from
/// `ReadingProgress::total() -> Option<usize>` via `Book::page_count_opt()` —
/// `None` (unknown) is mapped to `0` when the book has never been opened,
/// the real persisted count once it has been opened. `bookmarked` is the
/// caller-supplied flag (`book.path() == library.last_opened()`), derived at
/// the `carousel_data_for_indices` call site where the `Library` is available.
/// This is the single construction site for a `CarouselData`, shared by both
/// the full-library and filtered-index mappings.
fn carousel_data_for_book(book: &Book, bookmarked: bool) -> CarouselData {
    let progress = book.progress();
    // 1-based display page; saturate the i32 cast for a pathological value.
    let current = clamp_to_i32(progress.current());
    let total = progress.total().map_or(0, clamp_to_i32);
    let fraction = progress.fraction();
    // Pin the documented invariants at the single construction site (debug-only).
    debug_assert!(current >= 1, "current is 1-based and must be >= 1");
    debug_assert!(
        (0.0..=1.0).contains(&fraction),
        "progress fraction must be in 0.0..=1.0"
    );
    CarouselData {
        title: book.title().to_string(),
        current,
        total,
        progress: fraction,
        available: Library::is_available(book),
        bookmarked,
    }
}

/// Return the indices (in natural `Library::books()` order) of all books that
/// match `query` via [`book_matches`]. An empty query returns every index.
///
/// This is the single place the "which rows are visible?" projection lives for
/// the common (no forced-visible) case. [`LibrarySearchState::recompute`]
/// delegates here when no forced paths are active so the logic is exercised
/// in production and there is no duplicate filter loop.
pub(crate) fn matching_indices(library: &Library, query: &str) -> Vec<usize> {
    library
        .books()
        .iter()
        .enumerate()
        .filter_map(|(index, book)| book_matches(book, query).then_some(index))
        .collect()
}

/// Map the given library row `indices` to carousel display rows, preserving the
/// order of `indices`. Out-of-range indices are skipped via
/// `Library::books().get(index)` rather than panicking; all production indices
/// come from the shared [`LibrarySearchState`] projection, so they are valid.
///
/// `bookmarked` for each row is derived here: `book.path() == library.last_opened()`.
/// The `Library` is available at this call site, so the comparison is pure and
/// avoids threading a per-row flag through `carousel_data_for_book`'s callers.
pub fn carousel_data_for_indices(library: &Library, indices: &[usize]) -> Vec<CarouselData> {
    let last_opened = library.last_opened();
    indices
        .iter()
        .filter_map(|&index| library.books().get(index))
        .map(|book| {
            let bookmarked = last_opened.is_some_and(|p| p == book.path());
            carousel_data_for_book(book, bookmarked)
        })
        .collect()
}

/// Saturating `usize -> i32` for display counts (Slint ints are `i32`); a value
/// beyond `i32::MAX` clamps rather than wrapping negative. `pub(crate)` so the
/// cover controller's background page-count prefetch (`cover_loader`) maps a
/// resolved page count into a carousel row's `total` through the SAME saturating
/// rule used here, instead of duplicating the conversion.
pub(crate) fn clamp_to_i32(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Shared, crate-visible search state so both the live-query callback and the
/// add/open backfill paths preserve the same filter.
///
/// `forced_visible_paths` keeps freshly-added books visible even when the active
/// query would not match them; they stay forced until the next user query change
/// (see [`set_query`](LibrarySearchState::set_query), which clears them).
///
/// `visible_indices` is kept consistent with `(query, forced_visible_paths)`
/// after EVERY mutation: each mutator ([`set_query`](LibrarySearchState::set_query),
/// [`force_visible`](LibrarySearchState::force_visible)) owns its invariant and
/// recomputes the projection internally, so callers never see a stale visible
/// set (the natural-ordering "a mutator owns its invariant" rule —
/// docs/patterns.md). [`recompute`](LibrarySearchState::recompute) is the
/// re-filtering entry point for the cases where the LIBRARY itself changed but
/// neither the query nor the forced set did (startup seed + open-time backfill).
#[derive(Debug, Default)]
pub(crate) struct LibrarySearchState {
    query: String,
    forced_visible_paths: Vec<std::path::PathBuf>,
    visible_indices: Vec<usize>,
}

impl LibrarySearchState {
    /// The library row indices currently visible, in natural `Library::books()`
    /// order, as of the last [`recompute`](LibrarySearchState::recompute).
    pub(crate) fn visible_indices(&self) -> &[usize] {
        &self.visible_indices
    }

    /// Replace the query and clear any forced-visible paths, then recompute the
    /// visible set so it stays consistent with the new query. A user-driven query
    /// change supersedes the temporary "keep just-added books visible" override.
    pub(crate) fn set_query(&mut self, query: String, library: &Library) {
        self.query = query;
        self.forced_visible_paths.clear();
        self.recompute(library);
    }

    /// Force the given paths to stay visible regardless of the current query,
    /// until the next [`set_query`](LibrarySearchState::set_query), then recompute
    /// the visible set so the forced books appear immediately. Paths already
    /// forced are skipped (dedup), so a repeated add never grows the forced set
    /// with duplicates.
    pub(crate) fn force_visible<I>(&mut self, paths: I, library: &Library)
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            if !self.forced_visible_paths.contains(&path) {
                self.forced_visible_paths.push(path);
            }
        }
        self.recompute(library);
    }

    /// Recompute `visible_indices` from `library`: a book is visible if it
    /// matches the query or its path is forced visible. Indices are emitted in
    /// natural `Library::books()` order.
    ///
    /// The mutators above recompute internally, so this is the entry point for
    /// the LIBRARY-changed-only cases (startup seed + open-time backfill) where
    /// neither the query nor the forced set moved but the books did.
    ///
    /// When no paths are forced visible the common fast-path delegates to
    /// [`matching_indices`] so that function is always exercised in production
    /// (preventing a dead-code warning while keeping a single filter site).
    pub(crate) fn recompute(&mut self, library: &Library) {
        if self.forced_visible_paths.is_empty() {
            self.visible_indices = matching_indices(library, &self.query);
            return;
        }
        self.visible_indices = library
            .books()
            .iter()
            .enumerate()
            .filter_map(|(index, book)| {
                let forced = self
                    .forced_visible_paths
                    .iter()
                    .any(|path| path == book.path());
                (book_matches(book, &self.query) || forced).then_some(index)
            })
            .collect();
    }
}

/// Crate-visible bulk-selection state for the Library carousel, sitting next to
/// [`LibrarySearchState`]. Tracks which books the user has selected in selection
/// mode (`x`) so PR-4 can bulk-delete them.
///
/// Keyed by `PathBuf`, NOT by carousel index: indices shift whenever the
/// projection recomputes (a query change re-filters; a deletion compacts the
/// shelf), so an index-keyed set would silently point at the wrong books after
/// any such change. A path is stable identity. A `BTreeSet` keeps the selection
/// in a deterministic (sorted-by-path) order, so [`selected`](Self::selected)
/// yields a stable iteration order for the eventual bulk-removal call.
///
/// Selection is ORTHOGONAL to the search query: it lives here, not in
/// `LibrarySearchState`, so changing the query (which recomputes the visible
/// projection) never drops a selected book — a book filtered out of view stays
/// selected and reappears selected when the query clears.
#[derive(Debug, Default)]
pub(crate) struct LibrarySelectionState {
    selected: std::collections::BTreeSet<std::path::PathBuf>,
}

impl LibrarySelectionState {
    /// Toggle `path`'s membership: select it if absent, deselect if present.
    pub(crate) fn toggle(&mut self, path: std::path::PathBuf) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    /// Whether `path` is currently selected. Used by the carousel rebuild path to
    /// set each `CarouselItem.selected` flag (so the accent check badge renders).
    pub(crate) fn contains(&self, path: &std::path::Path) -> bool {
        self.selected.contains(path)
    }

    /// Drop every selection (e.g. on Esc / leaving selection mode).
    pub(crate) fn clear(&mut self) {
        self.selected.clear();
    }

    /// How many books are currently selected (across the WHOLE library, not just
    /// the visible slice). Drives the count-aware selection UI state.
    pub(crate) fn count(&self) -> usize {
        self.selected.len()
    }

    /// Iterate the selected paths in deterministic (`BTreeSet`, path-sorted) order.
    /// Consumed by `RemoveBooksUseCase::run` (the bulk-removal path snapshots these
    /// paths) and `confirm_delete_content` (the dialog title list) — PR-5 (#129).
    pub(crate) fn selected(&self) -> impl Iterator<Item = &std::path::Path> {
        self.selected.iter().map(std::path::PathBuf::as_path)
    }

    /// Select every CURRENTLY VISIBLE book (the search state's projection),
    /// leaving any already-selected non-visible books untouched.
    pub(crate) fn select_visible(&mut self, search: &LibrarySearchState, library: &Library) {
        for &index in search.visible_indices() {
            if let Some(book) = library.books().get(index) {
                self.selected.insert(book.path().to_path_buf());
            }
        }
    }

    /// Deselect every CURRENTLY VISIBLE book (the search state's projection),
    /// leaving any already-selected non-visible books untouched.
    ///
    /// Selection is ORTHOGONAL to the search query (invariant): only paths in the
    /// visible projection are removed; paths that are selected but outside the
    /// current visible set are never touched.
    pub(crate) fn deselect_visible(&mut self, search: &LibrarySearchState, library: &Library) {
        for &index in search.visible_indices() {
            if let Some(book) = library.books().get(index) {
                self.selected.remove(book.path());
            }
        }
    }

    /// Whether every currently visible book is selected. `false` when there are no
    /// visible books (an empty projection has nothing to consider "all selected").
    pub(crate) fn all_visible_selected(
        &self,
        search: &LibrarySearchState,
        library: &Library,
    ) -> bool {
        let visible = search.visible_indices();
        if visible.is_empty() {
            return false;
        }
        visible.iter().all(|&index| {
            library
                .books()
                .get(index)
                .is_some_and(|book| self.selected.contains(book.path()))
        })
    }

    /// How many of the currently visible books are selected.
    pub(crate) fn visible_selected_count(
        &self,
        search: &LibrarySearchState,
        library: &Library,
    ) -> usize {
        search
            .visible_indices()
            .iter()
            .filter(|&&index| {
                library
                    .books()
                    .get(index)
                    .is_some_and(|book| self.selected.contains(book.path()))
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::Library;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};

    /// Test helper: derive carousel rows for the WHOLE library in natural
    /// `Library::books()` order, exercising `carousel_data_for_book` (the single
    /// row-derivation site) through `carousel_data_for_indices` over every index.
    /// Production code always projects through the search state's visible indices,
    /// so there is no production all-library mapping to call here.
    fn carousel_data(library: &Library) -> Vec<CarouselData> {
        let all: Vec<usize> = (0..library.books().len()).collect();
        carousel_data_for_indices(library, &all)
    }

    #[test]
    fn empty_library_yields_no_rows() {
        let lib = Library::new();
        assert!(carousel_data(&lib).is_empty());
    }

    #[test]
    fn book_row_derives_title_current_total_progress() {
        // A real on-disk directory so `add` canonicalizes/derives a title and
        // `is_available` is true (the path resolves). `last_page` defaults to 0
        // for a freshly-added book (no position recorded yet), so this row is
        // the "unread, total unknown" case: current = 1, total = 0,
        // progress = 0.0, available = true.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        assert!(lib.add(dir.path().to_path_buf()).is_some());

        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        // Title is derived from the directory name (Book::title).
        let expected_title = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(row.title, expected_title);
        assert_eq!(row.current, 1); // last_page 0 -> 1-based 1
        assert_eq!(row.total, 0); // total unknown until opened
        assert_eq!(row.progress, 0.0); // total == 0 guard
        assert!(row.available); // the temp dir exists
    }

    #[test]
    fn unavailable_book_marked_unavailable() {
        // Add a real directory (so `add` succeeds + canonicalizes), then delete
        // it so the stored path no longer resolves: the book STAYS in the shelf
        // and the row is marked unavailable (no auto-prune — spec §9).
        let dir = tempfile::tempdir().expect("tempdir");
        let path: PathBuf = dir.path().to_path_buf();
        let mut lib = Library::new();
        assert!(lib.add(path.clone()).is_some());
        drop(dir); // remove the directory from disk

        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 1, "unavailable book is NOT auto-removed");
        assert!(!rows[0].available);
    }

    #[test]
    fn clamp_to_i32_saturates_at_max() {
        assert_eq!(clamp_to_i32(0), 0);
        assert_eq!(clamp_to_i32(i32::MAX as usize), i32::MAX);
        assert_eq!(clamp_to_i32((i32::MAX as usize) + 1), i32::MAX);
        assert_eq!(clamp_to_i32(usize::MAX), i32::MAX);
    }

    #[test]
    fn carousel_data_uses_library_natural_order() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        for name in ["vol 10", "vol 1", "vol 2"] {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).expect("create subdir");
            assert!(lib.add(dir).is_some());
        }
        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].title, "vol 1");
        assert_eq!(rows[1].title, "vol 2");
        assert_eq!(rows[2].title, "vol 10");
    }

    #[test]
    fn carousel_data_mixed_availability_per_book() {
        let root = tempfile::tempdir().expect("tempdir");
        let keep = root.path().join("keep");
        let gone = root.path().join("gone");
        std::fs::create_dir(&keep).expect("create keep");
        std::fs::create_dir(&gone).expect("create gone");
        let mut lib = Library::new();
        assert!(lib.add(keep).is_some());
        assert!(lib.add(gone.clone()).is_some());
        std::fs::remove_dir_all(&gone).expect("remove gone"); // now unresolvable
        let rows = carousel_data(&lib);
        assert_eq!(
            rows.len(),
            2,
            "both books stay in the shelf (no auto-prune)"
        );
        assert!(!rows[0].available, "gone dir no longer resolves");
        assert!(rows[1].available, "keep dir still resolves");
    }

    #[test]
    fn carousel_data_current_reflects_last_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        assert!(lib.add(dir.path().to_path_buf()).is_some());
        let path = lib.books()[0].path().to_path_buf();
        assert!(lib.set_last_page(&path, 4));
        let rows = carousel_data(&lib);
        assert_eq!(rows[0].current, 5); // 1-based: last_page 4 -> display 5
    }

    #[test]
    fn carousel_data_total_and_progress_from_page_count() {
        // An opened book has a persisted page count; the row must surface it as
        // the real `total` and compute `progress = ReadingProgress::fraction()`
        // (reached=4, total=10 → 0.4), with `current` the 1-based display page.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        assert!(lib.add(dir.path().to_path_buf()).is_some());
        let path = lib.books()[0].path().to_path_buf();
        assert!(lib.set_last_page(&path, 4));
        assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total, 10); // real persisted count
        assert_eq!(rows[0].current, 5); // 1-based: last_page 4 -> display 5
        assert_eq!(rows[0].progress, 0.4); // 4 / 10
    }

    #[test]
    fn book_matches_title_and_path_case_insensitively() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/Akira/Vol 01.cbz")).is_some());
        let book = &lib.books()[0];

        assert!(book_matches(book, "vol 01"));
        assert!(book_matches(book, "VOL 01"));
        assert!(book_matches(book, "akira"));
        assert!(book_matches(book, "/MANGA/AKIRA"));
        assert!(!book_matches(book, "banana"));
    }

    #[test]
    fn book_matches_empty_query_matches_everything() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/One Piece.cbz")).is_some());
        assert!(book_matches(&lib.books()[0], ""));
    }

    #[test]
    fn search_state_keeps_forced_added_paths_visible_until_query_changes() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut state = LibrarySearchState::default();
        state.set_query("alpha".to_string(), &lib);
        state.force_visible([beta_path], &lib);
        assert_eq!(state.visible_indices(), &[0, 1]);

        state.set_query("alpha".to_string(), &lib);
        assert_eq!(state.visible_indices(), &[0]);
    }

    #[test]
    fn recompute_forced_branch_excludes_non_matching_non_forced_books() {
        // The forced-visible branch keeps the query match (alpha) AND the forced
        // path (beta) visible, but a book matching NEITHER the query nor the
        // forced set (gamma) must stay hidden.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/gamma.cbz")).is_some());
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut state = LibrarySearchState::default();
        state.set_query("alpha".to_string(), &lib);
        state.force_visible([beta_path], &lib);
        // alpha (query match) + beta (forced) only; gamma is absent.
        assert_eq!(state.visible_indices(), &[0, 1]);
    }

    #[test]
    fn carousel_data_for_indices_skips_out_of_range_index() {
        // An out-of-range index is dropped rather than panicking; only the valid
        // row is mapped.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());

        let rows = carousel_data_for_indices(&lib, &[0, 99]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "alpha");
    }

    #[test]
    fn matching_indices_return_library_natural_order_rows() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/vol 10.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/vol 1.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/bonus.cbz")).is_some());

        // Natural sort order: "bonus" < "vol 1" < "vol 10"
        // so index 0 = bonus, 1 = vol 1, 2 = vol 10.
        assert_eq!(matching_indices(&lib, "vol"), vec![1, 2]);
        assert_eq!(matching_indices(&lib, "BONUS"), vec![0]);
        assert_eq!(matching_indices(&lib, ""), vec![0, 1, 2]);
        assert!(matching_indices(&lib, "missing").is_empty());
    }

    #[test]
    fn selection_toggle_adds_then_removes() {
        let mut sel = LibrarySelectionState::default();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(!sel.contains(&path));
        assert_eq!(sel.count(), 0);

        sel.toggle(path.clone());
        assert!(sel.contains(&path), "first toggle selects");
        assert_eq!(sel.count(), 1);

        sel.toggle(path.clone());
        assert!(!sel.contains(&path), "second toggle deselects");
        assert_eq!(sel.count(), 0);
    }

    #[test]
    fn selection_toggle_on_path_not_in_library_still_selects_and_deselects() {
        // Selection is path-keyed, not library-keyed: a path that was never added
        // to any Library still toggles cleanly. The selection state owns the set
        // of selected paths independently of what books exist.
        let mut sel = LibrarySelectionState::default();
        let path = PathBuf::from("/manga/never-added.cbz");
        assert!(!sel.contains(&path));

        sel.toggle(path.clone());
        assert!(
            sel.contains(&path),
            "toggling an absent path still selects it"
        );

        sel.toggle(path.clone());
        assert!(!sel.contains(&path), "a second toggle deselects it");
    }

    #[test]
    fn selection_clear_drops_everything() {
        let mut sel = LibrarySelectionState::default();
        sel.toggle(PathBuf::from("/manga/a.cbz"));
        sel.toggle(PathBuf::from("/manga/b.cbz"));
        assert_eq!(sel.count(), 2);
        sel.clear();
        assert_eq!(sel.count(), 0);
        assert!(!sel.contains(Path::new("/manga/a.cbz")));
    }

    #[test]
    fn selected_iterates_in_deterministic_path_order() {
        // BTreeSet ⇒ path-sorted iteration regardless of insertion order.
        let mut sel = LibrarySelectionState::default();
        sel.toggle(PathBuf::from("/manga/c.cbz"));
        sel.toggle(PathBuf::from("/manga/a.cbz"));
        sel.toggle(PathBuf::from("/manga/b.cbz"));
        let paths: Vec<&Path> = sel.selected().collect();
        assert_eq!(
            paths,
            vec![
                Path::new("/manga/a.cbz"),
                Path::new("/manga/b.cbz"),
                Path::new("/manga/c.cbz"),
            ]
        );
    }

    #[test]
    fn selection_is_orthogonal_to_search_query() {
        // Selecting a book then narrowing the query so it is filtered OUT of the
        // visible set must NOT drop the selection: it stays selected and reappears
        // when the query clears (selection lives independently of the projection).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // everything visible
        let mut sel = LibrarySelectionState::default();
        sel.toggle(beta_path.clone());
        assert!(sel.contains(&beta_path));

        // Narrow to "alpha": beta is no longer visible, but stays selected.
        search.set_query("alpha".to_string(), &lib);
        assert_eq!(search.visible_indices(), &[0]);
        assert!(
            sel.contains(&beta_path),
            "a query change must not drop a selection"
        );

        // Clear the query: beta is visible again and still selected.
        search.set_query(String::new(), &lib);
        assert_eq!(search.visible_indices(), &[0, 1]);
        assert!(sel.contains(&beta_path));
    }

    #[test]
    fn select_visible_selects_only_the_visible_projection() {
        // With an active "alpha" filter, select_visible selects ONLY the visible
        // (alpha) book; the filtered-out beta is left untouched.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query("alpha".to_string(), &lib);
        let mut sel = LibrarySelectionState::default();
        sel.select_visible(&search, &lib);

        assert!(sel.contains(&alpha_path), "visible book is selected");
        assert!(!sel.contains(&beta_path), "filtered-out book is not");
        assert_eq!(sel.count(), 1);
    }

    #[test]
    fn all_visible_selected_flips_with_the_visible_set() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible
        let mut sel = LibrarySelectionState::default();

        assert!(
            !sel.all_visible_selected(&search, &lib),
            "nothing selected ⇒ not all-visible-selected"
        );
        sel.toggle(alpha_path.clone());
        assert!(
            !sel.all_visible_selected(&search, &lib),
            "only one of two visible selected"
        );
        assert_eq!(sel.visible_selected_count(&search, &lib), 1);

        sel.toggle(beta_path);
        assert!(
            sel.all_visible_selected(&search, &lib),
            "both visible now selected ⇒ all-visible-selected"
        );
        assert_eq!(sel.visible_selected_count(&search, &lib), 2);

        // Narrow to "alpha": only alpha visible and it IS selected ⇒ flips back to true.
        search.set_query("alpha".to_string(), &lib);
        assert!(sel.all_visible_selected(&search, &lib));
        assert_eq!(sel.visible_selected_count(&search, &lib), 1);
    }

    #[test]
    fn deselect_visible_removes_only_visible_selections_preserves_out_of_search() {
        // Orthogonality: deselect_visible must only remove visible selections;
        // a selected book that is filtered out of the visible projection must stay selected.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        // Only alpha is visible under "alpha" filter.
        search.set_query("alpha".to_string(), &lib);
        assert_eq!(search.visible_indices(), &[0]);

        let mut sel = LibrarySelectionState::default();
        // Select both alpha (visible) and beta (out-of-search).
        sel.toggle(alpha_path.clone());
        sel.toggle(beta_path.clone());
        assert_eq!(sel.count(), 2);

        // deselect_visible must only remove alpha (visible); beta stays selected.
        sel.deselect_visible(&search, &lib);
        assert!(
            !sel.contains(&alpha_path),
            "visible alpha must be deselected"
        );
        assert!(
            sel.contains(&beta_path),
            "out-of-search beta must remain selected (orthogonality)"
        );
        assert_eq!(sel.count(), 1);
    }

    #[test]
    fn deselect_visible_empty_projection_is_noop() {
        // An empty visible projection must leave the selection unchanged.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query("no-match".to_string(), &lib);
        assert!(search.visible_indices().is_empty());

        let mut sel = LibrarySelectionState::default();
        sel.toggle(alpha_path.clone());
        assert_eq!(sel.count(), 1);

        // No visible books ⇒ no-op.
        sel.deselect_visible(&search, &lib);
        assert_eq!(sel.count(), 1, "empty projection must be a no-op");
        assert!(
            sel.contains(&alpha_path),
            "alpha must still be selected after no-op deselect_visible"
        );
    }

    #[test]
    fn select_visible_then_deselect_visible_clears_all_visible() {
        // After select_visible then deselect_visible, all_visible_selected is false
        // and visible_selected_count is 0.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible

        let mut sel = LibrarySelectionState::default();
        sel.select_visible(&search, &lib);
        assert!(
            sel.all_visible_selected(&search, &lib),
            "after select_visible, all visible must be selected"
        );

        sel.deselect_visible(&search, &lib);
        assert!(
            !sel.all_visible_selected(&search, &lib),
            "after deselect_visible, all_visible_selected must be false"
        );
        assert_eq!(
            sel.visible_selected_count(&search, &lib),
            0,
            "visible_selected_count must be 0 after deselect_visible"
        );
    }

    #[test]
    fn deselect_visible_no_panic_when_some_visible_were_never_selected() {
        // Some visible books were never selected: deselect_visible must not panic
        // and must not over-remove (already-absent entries are silently skipped).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib); // both visible

        let mut sel = LibrarySelectionState::default();
        // Only alpha is selected; beta is visible but was never selected.
        sel.toggle(alpha_path.clone());
        assert_eq!(sel.count(), 1);

        // Must not panic even though beta was never in the selection.
        sel.deselect_visible(&search, &lib);
        assert_eq!(sel.count(), 0, "alpha must be deselected");
        assert!(
            !sel.contains(&alpha_path),
            "alpha must no longer be selected"
        );
    }

    #[test]
    fn all_visible_selected_false_for_empty_projection() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        let mut search = LibrarySearchState::default();
        search.set_query("no-match".to_string(), &lib); // empty projection
        assert!(search.visible_indices().is_empty());
        let sel = LibrarySelectionState::default();
        assert!(
            !sel.all_visible_selected(&search, &lib),
            "an empty visible set is not all-selected"
        );
        assert_eq!(sel.visible_selected_count(&search, &lib), 0);
    }

    // ── bookmarked derivation tests ───────────────────────────────────────────

    #[test]
    fn carousel_data_bookmarked_matches_last_opened_book() {
        // The row for the last-opened book must have bookmarked == true.
        // `register_opened` both adds the book and sets `last_opened`.
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/alpha.cbz");
        lib.register_opened(&path, None);

        let rows = carousel_data_for_indices(&lib, &[0]);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].bookmarked,
            "the last-opened book must be bookmarked"
        );
    }

    #[test]
    fn carousel_data_bookmarked_false_for_other_rows() {
        // Only the last-opened book is bookmarked; all other rows are false.
        // Natural sort: "alpha" < "beta", so alpha is index 0, beta is index 1.
        let mut lib = Library::new();
        let alpha = PathBuf::from("/manga/alpha.cbz");
        let beta = PathBuf::from("/manga/beta.cbz");
        lib.register_opened(&alpha, None); // adds alpha + sets last_opened = alpha
        lib.add(beta); // adds beta (not last_opened)

        let rows = carousel_data_for_indices(&lib, &[0, 1]);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].bookmarked, "alpha is the last-opened book");
        assert!(!rows[1].bookmarked, "beta is NOT last-opened");
    }

    #[test]
    fn carousel_data_bookmarked_false_when_last_opened_filtered_out() {
        // When the last-opened book (alpha) is excluded from the index slice
        // (as a search filter would do), the returned row for beta must have
        // bookmarked == false — the ribbon must not bleed onto visible books
        // that are not actually the last-opened book.
        // Natural sort: "alpha" < "beta", so alpha is index 0, beta is index 1.
        let mut lib = Library::new();
        let alpha = PathBuf::from("/manga/alpha.cbz");
        let beta = PathBuf::from("/manga/beta.cbz");
        lib.register_opened(&alpha, None); // adds alpha + sets last_opened = alpha
        lib.add(beta); // adds beta (not last_opened)

        // Derive beta's index from lib.books() to be safe against sort changes.
        let beta_idx = lib
            .books()
            .iter()
            .position(|b| b.path().ends_with("beta.cbz"))
            .expect("beta must be in the library");

        // Pass only beta's index — alpha (last_opened) is excluded.
        let rows = carousel_data_for_indices(&lib, &[beta_idx]);
        assert_eq!(rows.len(), 1);
        assert!(
            !rows[0].bookmarked,
            "beta is not last-opened; bookmarked must be false even though alpha is last_opened"
        );
    }

    #[test]
    fn carousel_data_bookmarked_false_when_last_opened_is_none() {
        // When no book has ever been opened (last_opened == None),
        // every row must have bookmarked == false.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        assert_eq!(lib.last_opened(), None, "no book opened yet");

        let rows = carousel_data_for_indices(&lib, &[0, 1]);
        assert!(
            rows.iter().all(|r| !r.bookmarked),
            "no row should be bookmarked when last_opened is None"
        );
    }

    #[test]
    fn deselect_visible_after_query_pivot_removes_only_new_projection() {
        // Production sequence: select_visible with broad/empty query (all books
        // selected), then set_query narrowing the projection to a subset, then
        // deselect_visible — only the narrowed projection's books must be removed;
        // books outside the narrowed projection must remain selected.
        //
        // Library: alpha, beta, gamma (natural sort order).
        // Step 1: empty query → all 3 visible → select_visible selects all.
        // Step 2: narrow to "alpha" → only alpha visible.
        // Step 3: deselect_visible → only alpha removed; beta and gamma stay.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/alpha.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/beta.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/gamma.cbz")).is_some());
        let alpha_path = lib.books()[0].path().to_path_buf();
        let beta_path = lib.books()[1].path().to_path_buf();
        let gamma_path = lib.books()[2].path().to_path_buf();

        let mut search = LibrarySearchState::default();
        // Step 1: broad (empty) query — all books visible.
        search.set_query(String::new(), &lib);
        assert_eq!(search.visible_indices().len(), 3);

        let mut sel = LibrarySelectionState::default();
        sel.select_visible(&search, &lib);
        assert_eq!(
            sel.count(),
            3,
            "all three books must be selected after select_visible"
        );

        // Step 2: narrow to "alpha" — only alpha is visible now.
        search.set_query("alpha".to_string(), &lib);
        assert_eq!(search.visible_indices(), &[0], "only alpha index visible");

        // Step 3: deselect_visible removes ONLY alpha (the new projection).
        sel.deselect_visible(&search, &lib);

        assert!(
            !sel.contains(&alpha_path),
            "alpha (in narrowed projection) must be deselected"
        );
        assert!(
            sel.contains(&beta_path),
            "beta (outside narrowed projection) must remain selected"
        );
        assert!(
            sel.contains(&gamma_path),
            "gamma (outside narrowed projection) must remain selected"
        );
        // Exactly the two out-of-projection books remain.
        assert_eq!(
            sel.count(),
            2,
            "count must equal the books outside the narrowed projection"
        );
    }
}
