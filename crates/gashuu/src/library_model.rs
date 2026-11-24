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
//! value object. `ReadingProgress::current()` is 1-based (`last_viewed + 1`,
//! saturating, >= 1); `ReadingProgress::fraction()` guards `total == 0` to
//! `0.0` (no NaN/inf); `ReadingProgress::total()` is the persisted page count.

use gashuu_core::{book_matches, Book, BookId, Library};

/// One carousel row's display data, derived from a `Book` in the `Library`.
/// Plain data only (no `slint::Image`) so the derivation is unit-testable
/// without a display backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CarouselData {
    /// Book display title (file stem / directory name; from `Book::title`).
    pub title: String,
    /// 1-based current page for display = `ReadingProgress::current()` (`last_viewed + 1`,
    /// saturating). A fresh book (`last_viewed == 0`) shows `1`.
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
    /// Derived by the caller-supplied availability predicate (desktop:
    /// `book_availability::book_path_exists`). Unavailable books stay in the shelf, rendered
    /// grayed with a broken-cover placeholder.
    pub available: bool,
    /// True when this book is the last-opened book (`Some(book.id()) == library.last_opened()`).
    /// Drives the BookmarkRibbon overlay on the cover card. Pure derivation — no I/O.
    pub bookmarked: bool,
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
/// caller-supplied flag (`Some(book.id()) == library.last_opened()`), derived at
/// the `carousel_data_for_indices` call site where the `Library` is available.
/// This is the single construction site for a `CarouselData`, shared by both
/// the full-library and filtered-index mappings.
fn carousel_data_for_book(book: &Book, bookmarked: bool, available: bool) -> CarouselData {
    let progress = book.progress();
    // 1-based display page; saturate the i32 cast for a pathological value.
    let current = clamp_to_i32(progress.current());
    let total = progress.total().map_or(0, clamp_to_i32);
    let fraction = progress.fraction();
    // Pin the documented invariants at the single construction site (debug-only).
    debug_assert!(current >= 1, "current is 1-based and must be >= 1");
    debug_assert!(
        total == 0 || current <= total,
        "current must not exceed a known total"
    );
    debug_assert!(
        (0.0..=1.0).contains(&fraction),
        "progress fraction must be in 0.0..=1.0"
    );
    CarouselData {
        title: book.title().to_string(),
        current,
        total,
        progress: fraction,
        available,
        bookmarked,
    }
}

/// Return the indices (in natural `Library::books()` order) of all books that
/// match `query` via [`book_matches`]. An empty query returns every index.
///
/// This is the single place the "which rows are visible?" projection lives for
/// the common (no forced-visible) case. [`LibrarySearchState::recompute`]
/// delegates here when no forced IDs are active so the logic is exercised
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
/// `bookmarked` for each row is derived here: `Some(book.id()) == library.last_opened()`.
/// The `Library` is available at this call site, so the comparison is pure and
/// avoids threading a per-row flag through `carousel_data_for_book`'s callers.
/// Availability is derived by the caller-supplied predicate for each book.
pub fn carousel_data_for_indices(
    library: &Library,
    indices: &[usize],
    is_available: &dyn Fn(&Book) -> bool,
) -> Vec<CarouselData> {
    let last_opened = library.last_opened();
    indices
        .iter()
        .filter_map(|&index| library.books().get(index))
        .map(|book| {
            let bookmarked = last_opened == Some(book.id());
            let available = is_available(book);
            carousel_data_for_book(book, bookmarked, available)
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
/// `forced_visible_ids` keeps freshly-added books visible even when the active
/// query would not match them; they stay forced until the next user query change
/// (see [`set_query`](LibrarySearchState::set_query), which clears them).
///
/// `visible_indices` is kept consistent with `(query, forced_visible_ids)`
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
    forced_visible_ids: Vec<BookId>,
    visible_indices: Vec<usize>,
}

impl LibrarySearchState {
    /// The library row indices currently visible, in natural `Library::books()`
    /// order, as of the last [`recompute`](LibrarySearchState::recompute).
    pub(crate) fn visible_indices(&self) -> &[usize] {
        &self.visible_indices
    }

    /// Replace the query and clear any forced-visible IDs, then recompute the
    /// visible set so it stays consistent with the new query. A user-driven query
    /// change supersedes the temporary "keep just-added books visible" override.
    pub(crate) fn set_query(&mut self, query: String, library: &Library) {
        self.query = query;
        self.forced_visible_ids.clear();
        self.recompute(library);
    }

    /// Force the given IDs to stay visible regardless of the current query,
    /// until the next [`set_query`](LibrarySearchState::set_query), then recompute
    /// the visible set so the forced books appear immediately. IDs already
    /// forced are skipped (dedup), so a repeated add never grows the forced set
    /// with duplicates.
    pub(crate) fn force_visible<I>(&mut self, ids: I, library: &Library)
    where
        I: IntoIterator<Item = BookId>,
    {
        for id in ids {
            if !self.forced_visible_ids.contains(&id) {
                self.forced_visible_ids.push(id);
            }
        }
        self.recompute(library);
    }

    /// Recompute `visible_indices` from `library`: a book is visible if it
    /// matches the query or its ID is forced visible. Indices are emitted in
    /// natural `Library::books()` order.
    ///
    /// The mutators above recompute internally, so this is the entry point for
    /// the LIBRARY-changed-only cases (startup seed + open-time backfill) where
    /// neither the query nor the forced set moved but the books did.
    ///
    /// When no IDs are forced visible the common fast-path delegates to
    /// [`matching_indices`] so that function is always exercised in production
    /// (preventing a dead-code warning while keeping a single filter site).
    pub(crate) fn recompute(&mut self, library: &Library) {
        if self.forced_visible_ids.is_empty() {
            self.visible_indices = matching_indices(library, &self.query);
            return;
        }
        self.visible_indices = library
            .books()
            .iter()
            .enumerate()
            .filter_map(|(index, book)| {
                let forced = self.forced_visible_ids.contains(book.id());
                (book_matches(book, &self.query) || forced).then_some(index)
            })
            .collect();
    }
}

/// Crate-visible bulk-selection state for the Library carousel, sitting next to
/// [`LibrarySearchState`]. Tracks which books the user has selected in selection
/// mode (`x`) so PR-4 can bulk-delete them.
///
/// Selection follows book IDs as the visible projection changes. Iteration
/// uses `BookId` order for the confirmation dialog and removal snapshot.
#[derive(Debug, Default)]
pub(crate) struct LibrarySelectionState {
    selected: std::collections::BTreeSet<BookId>,
}

impl LibrarySelectionState {
    /// Toggle `id`'s membership: select it if absent, deselect if present.
    pub(crate) fn toggle(&mut self, id: BookId) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
    }

    /// Select `id` (idempotent insert into the ID set).
    pub(crate) fn insert(&mut self, id: BookId) {
        self.selected.insert(id);
    }

    /// Deselect `id` (idempotent remove from the ID set).
    pub(crate) fn remove(&mut self, id: &BookId) {
        self.selected.remove(id);
    }

    /// Whether `id` is currently selected. Used by the carousel rebuild id to
    /// set each `CarouselItem.selected` flag (so the accent check badge renders).
    pub(crate) fn contains(&self, id: &BookId) -> bool {
        self.selected.contains(id)
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

    /// Iterate the selected IDs in deterministic (`BTreeSet`, ID) order.
    /// Consumed by `RemoveBooksUseCase::run` (the bulk removal snapshots these IDs) and `confirm_delete_content` (the dialog title list) — PR-5 (#129).
    pub(crate) fn selected(&self) -> impl Iterator<Item = &BookId> {
        self.selected.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book_availability::book_path_exists;
    use gashuu_core::Library;
    use proptest::{prop_assert_eq, prop_oneof, proptest, strategy::Strategy};
    use std::collections::BTreeSet;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;

    /// Test helper: derive carousel rows for the WHOLE library in natural
    /// `Library::books()` order, exercising `carousel_data_for_book` (the single
    /// row-derivation site) through `carousel_data_for_indices` over every index.
    /// Production code always projects through the search state's visible indices,
    /// so there is no production all-library mapping to call here.
    fn carousel_data(library: &Library) -> Vec<CarouselData> {
        let all: Vec<usize> = (0..library.books().len()).collect();
        carousel_data_for_indices(library, &all, &book_path_exists)
    }

    #[test]
    fn carousel_data_uses_injected_availability_predicate() {
        let mut lib = Library::new();
        assert!(lib.add(BookId::new("/manga/a.cbz"), "a".into()).is_some());
        assert!(lib.add(BookId::new("/manga/b.cbz"), "b".into()).is_some());

        let rows = carousel_data_for_indices(&lib, &[0, 1], &|b: &Book| {
            b.id().as_str().ends_with("a.cbz")
        });
        assert!(rows[0].available);
        assert!(!rows[1].available);
    }

    #[test]
    fn empty_library_yields_no_rows() {
        let lib = Library::new();
        assert!(carousel_data(&lib).is_empty());
    }

    #[test]
    fn book_row_derives_title_current_total_progress() {
        // Freshly-added book: `add` derives a title, the path resolves (available), and
        // `resume_page` defaults to 0 — the "unread, total unknown" case (current=1, total=0).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        let (id, title) = crate::book_path::identify(dir.path(), true).unwrap();
        assert!(lib.add(id, title).is_some());

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
        assert_eq!(row.current, 1); // resume_page 0 -> 1-based 1
        assert_eq!(row.total, 0); // total unknown until opened
        assert_eq!(row.progress, 0.0); // total == 0 guard
        assert!(row.available); // the temp dir exists
    }

    #[test]
    fn unavailable_book_marked_unavailable() {
        // Add a real directory, then delete it so the stored path no longer resolves: the
        // book STAYS in the shelf, marked unavailable (no auto-prune — spec §9).
        let dir = tempfile::tempdir().expect("tempdir");
        let path: PathBuf = dir.path().to_path_buf();
        let mut lib = Library::new();
        assert!({
            let (id, title) = crate::book_path::identify(&path, true).unwrap();
            lib.add(id, title)
        }
        .is_some());
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

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Page counts stay nonnegative and saturate at the largest value the UI can display.
            #[test]
            fn prop_clamp_to_i32_saturates(v in prop_oneof![any::<usize>(), 0..=i32::MAX as usize]) {
                let clamped = clamp_to_i32(v);
                let expected = (v as u128).min(i32::MAX as u128) as i64;

                prop_assert_eq!(i64::from(clamped), expected);
                prop_assert!(clamped >= 0);
            }
        }
    }

    #[test]
    fn carousel_data_uses_library_natural_order() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        for name in ["vol 10", "vol 1", "vol 2"] {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).expect("create subdir");
            assert!({
                let (id, title) = crate::book_path::identify(&dir, true).unwrap();
                lib.add(id, title)
            }
            .is_some());
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
        assert!({
            let (id, title) = crate::book_path::identify(&keep, true).unwrap();
            lib.add(id, title)
        }
        .is_some());
        assert!({
            let (id, title) = crate::book_path::identify(&gone, true).unwrap();
            lib.add(id, title)
        }
        .is_some());
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
    fn carousel_data_total_and_progress_from_page_count() {
        // An opened book has a persisted page count: the row surfaces it as the real
        // `total` and computes `progress = fraction()` (last_viewed=4, total=10 → 4/9).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        let (id, title) = crate::book_path::identify(dir.path(), true).unwrap();
        assert!(lib.add(id, title).is_some());
        let path = lib.books()[0].id().clone();
        assert!(lib.set_resume_page(&path, 4));
        assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total, 10); // real persisted count
        assert_eq!(rows[0].current, 5); // 1-based: resume_page 4 -> display 5
        assert_eq!(rows[0].progress, 4.0 / 9.0);
    }

    #[derive(Debug)]
    enum SearchOp {
        SetQuery(String),
        ForceVisible(Vec<usize>),
    }

    proptest! {
        #[test]
        fn search_projection_matches_query_union_forced_ids_in_library_order(
            titles in proptest::collection::vec("[a-dA-D]{1,8}", 0..=8),
            ops in proptest::collection::vec(
                prop_oneof![
                    "[a-dA-D0-9 ]{0,3}".prop_map(SearchOp::SetQuery),
                    proptest::collection::vec(0usize..10, 0..=10).prop_map(SearchOp::ForceVisible),
                ],
                0..=40,
            ),
        ) {
            let ids: Vec<_> = (0..10).map(|i| BookId::new(format!("/manga/{i}.cbz"))).collect();
            let mut library = Library::new();
            library.add_many(ids.iter().cloned().zip(titles).rev().collect::<Vec<_>>());

            let mut state = LibrarySearchState::default();
            let mut query = String::new();
            let mut forced = BTreeSet::new();
            for op in ops {
                match op {
                    SearchOp::SetQuery(next) => {
                        state.set_query(next.clone(), &library);
                        query = next.to_ascii_lowercase();
                        forced.clear();
                    }
                    SearchOp::ForceVisible(indices) => {
                        let ids: Vec<_> = indices.into_iter().map(|i| ids[i].clone()).collect();
                        state.force_visible(ids.iter().cloned(), &library);
                        forced.extend(ids);
                    }
                }
                let expected: Vec<_> = library.books().iter().enumerate().filter_map(|(index, book)| {
                    let matches = book.title().to_ascii_lowercase().contains(&query)
                        || book.id().as_str().to_ascii_lowercase().contains(&query);
                    (matches || forced.contains(book.id())).then_some(index)
                }).collect();
                prop_assert_eq!(state.visible_indices(), expected.as_slice());
            }
        }
    }

    #[test]
    fn carousel_row_current_never_exceeds_total() {
        // The known count bounds the stored resume, so the row displays page 5 of 5.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut lib = Library::new();
        let (id, title) = crate::book_path::identify(dir.path(), true).unwrap();
        assert!(lib.add(id, title).is_some());
        let path = lib.books()[0].id().clone();
        assert!(lib.set_page_count(&path, NonZeroUsize::new(5).unwrap()));
        assert!(lib.set_resume_page(&path, 40));
        let rows = carousel_data(&lib);
        assert_eq!(rows[0].total, 5);
        assert!(
            rows[0].current <= rows[0].total,
            "current {} must not exceed total {}",
            rows[0].current,
            rows[0].total
        );
    }

    #[test]
    fn carousel_data_for_indices_skips_out_of_range_index() {
        // An out-of-range index is dropped rather than panicking; only the valid
        // row is mapped.
        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());

        let rows = carousel_data_for_indices(&lib, &[0, 99], &book_path_exists);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "alpha");
    }

    #[test]
    fn matching_indices_return_library_natural_order_rows() {
        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/vol 10.cbz"), "vol 10".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/vol 1.cbz"), "vol 1".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/bonus.cbz"), "bonus".into())
            .is_some());

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
        let path = BookId::new("/manga/a.cbz");
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
        // Selection is path-keyed, not library-keyed: a path never added to any Library
        // still toggles cleanly — the state owns its set independently of what books exist.
        let mut sel = LibrarySelectionState::default();
        let path = BookId::new("/manga/never-added.cbz");
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
        sel.toggle(BookId::new("/manga/a.cbz"));
        sel.toggle(BookId::new("/manga/b.cbz"));
        assert_eq!(sel.count(), 2);
        sel.clear();
        assert_eq!(sel.count(), 0);
        assert!(!sel.contains(&BookId::new("/manga/a.cbz")));
    }

    #[test]
    fn selected_iterates_in_deterministic_path_order() {
        // BTreeSet ⇒ path-sorted iteration regardless of insertion order.
        let mut sel = LibrarySelectionState::default();
        sel.toggle(BookId::new("/manga/c.cbz"));
        sel.toggle(BookId::new("/manga/a.cbz"));
        sel.toggle(BookId::new("/manga/b.cbz"));
        let paths: Vec<&BookId> = sel.selected().collect();
        assert_eq!(
            paths,
            vec![
                &BookId::new("/manga/a.cbz"),
                &BookId::new("/manga/b.cbz"),
                &BookId::new("/manga/c.cbz"),
            ]
        );
    }

    #[test]
    fn selection_is_orthogonal_to_search_query() {
        // Selecting a book then narrowing the query so it is filtered OUT must NOT drop
        // the selection: it stays selected and reappears when the query clears.
        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/beta.cbz"), "beta".into())
            .is_some());
        let beta_path = lib.books()[1].id().clone();

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

    // ── bookmarked derivation tests ───────────────────────────────────────────

    #[test]
    fn carousel_data_bookmarked_matches_last_opened_book() {
        // The row for the last-opened book must have bookmarked == true.
        // `register_opened` both adds the book and sets `last_opened`.
        let mut lib = Library::new();
        let path = BookId::new("/manga/alpha.cbz");
        lib.register_opened(&path, "alpha", None);

        let rows = carousel_data_for_indices(&lib, &[0], &book_path_exists);
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
        let alpha = BookId::new("/manga/alpha.cbz");
        let beta = BookId::new("/manga/beta.cbz");
        lib.register_opened(&alpha, "alpha", None); // adds alpha + sets last_opened = alpha
        lib.add(beta, "beta".into()); // adds beta (not last_opened)

        let rows = carousel_data_for_indices(&lib, &[0, 1], &book_path_exists);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].bookmarked, "alpha is the last-opened book");
        assert!(!rows[1].bookmarked, "beta is NOT last-opened");
    }

    #[test]
    fn carousel_data_bookmarked_false_when_last_opened_filtered_out() {
        // When the last-opened book (alpha) is excluded from the index slice (as a search
        // filter would), beta's row must be bookmarked == false — the ribbon must not bleed.
        let mut lib = Library::new();
        let alpha = BookId::new("/manga/alpha.cbz");
        let beta = BookId::new("/manga/beta.cbz");
        lib.register_opened(&alpha, "alpha", None); // adds alpha + sets last_opened = alpha
        lib.add(beta, "beta".into()); // adds beta (not last_opened)

        // Derive beta's index from lib.books() to be safe against sort changes.
        let beta_idx = lib
            .books()
            .iter()
            .position(|b| b.id().as_str().ends_with("beta.cbz"))
            .expect("beta must be in the library");

        // Pass only beta's index — alpha (last_opened) is excluded.
        let rows = carousel_data_for_indices(&lib, &[beta_idx], &book_path_exists);
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
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/beta.cbz"), "beta".into())
            .is_some());
        assert_eq!(lib.last_opened(), None, "no book opened yet");

        let rows = carousel_data_for_indices(&lib, &[0, 1], &book_path_exists);
        assert!(
            rows.iter().all(|r| !r.bookmarked),
            "no row should be bookmarked when last_opened is None"
        );
    }
}
