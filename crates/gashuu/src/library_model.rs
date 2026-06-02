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

use gashuu_core::Library;

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
}

/// Map a `Library` to its carousel display rows, in natural `Library::books()`
/// order.
///
/// Each row is derived from `Book::progress()`, which returns a
/// `ReadingProgress` value object. `current = progress.current()` (1-based,
/// `>= 1`, saturating); `progress = progress.fraction()` is guarded so
/// `total == 0` yields `0.0` (never NaN/inf); `total` comes from
/// `ReadingProgress::total() -> Option<usize>` via `Book::page_count_opt()` —
/// `None` (unknown) is mapped to `0` when the book has never been opened,
/// the real persisted count once it has been opened.
pub fn carousel_data(library: &Library) -> Vec<CarouselData> {
    library
        .books()
        .iter()
        .map(|book| {
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
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::Library;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;

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
}
