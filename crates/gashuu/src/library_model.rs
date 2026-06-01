//! Pure (Slint-free) mapping from the headless `Library` to the carousel's
//! per-book display rows.
//!
//! The Slint `CarouselItem` carries an `image` (a `!Send`, backend-dependent
//! `slint::Image`) and is awkward to build in a headless unit test, so the
//! derivable display data lives in this plain `CarouselData` struct, table-
//! tested here. `main.rs`'s `to_carousel_item` adapter turns each row into a
//! `CarouselItem` on the UI thread (placeholder cover for PR-C; real covers
//! stream in via PR-V). This is the SINGLE place the Library → carousel
//! display mapping lives (the carousel counterpart of
//! `thumbnail_strip::thumbnail_item`).
//!
//! Progress is `last_page / total` with a `total == 0` guard (an unread or
//! pageless book reports `0.0`); `current` is the 1-based page for display
//! (`last_page + 1`), matching the strip's 1-based page labels.

use gashuu_core::Library;

/// One carousel row's display data, derived from a `Book` in the `Library`.
/// Plain data only (no `slint::Image`) so the derivation is unit-testable
/// without a display backend.
#[derive(Debug, Clone, PartialEq)]
pub struct CarouselData {
    /// Book display title (file stem / directory name; from `Book::title`).
    pub title: String,
    /// 1-based current page for display (`last_page + 1`). A fresh book
    /// (`last_page == 0`) shows `1`.
    pub current: i32,
    /// Total page count for display. Unknown until the book is opened, so the
    /// Library does not store it — `total` is 0 here and PR-V/PR-R fill the
    /// real count once the book's page source is resolved.
    pub total: i32,
    /// Reading progress in `0.0..=1.0` (`last_page / total`, `0.0` when
    /// `total == 0`). Ambient per-cover bar; accent fill, green when `>= 1.0`.
    pub progress: f32,
    /// Derived availability (`Library::is_available`): false when the book's
    /// path no longer resolves. Unavailable books stay in the shelf, rendered
    /// grayed with a broken-cover placeholder.
    pub available: bool,
}

/// Map a `Library` to its carousel display rows, in shelf (insertion) order.
///
/// `current = last_page + 1` (1-based display); `progress = last_page / total`
/// guarded so `total == 0` yields `0.0` (never NaN/inf); `available` is the
/// derived existence check. `total` is 0 until the book is opened (the Library
/// does not persist page counts); PR-V/PR-R supply the real total once the
/// page source is resolved, at which point `current`/`progress` become
/// meaningful against it.
pub fn carousel_data(library: &Library) -> Vec<CarouselData> {
    library
        .books()
        .iter()
        .map(|book| {
            let last_page = book.last_page();
            let total = 0usize; // Unknown until opened; see doc comment.
            let progress = progress_fraction(last_page, total);
            CarouselData {
                title: book.title().to_string(),
                // `last_page` is a `usize` page index; saturate the +1 and the
                // i32 cast so a pathological value can never panic in debug.
                current: clamp_to_i32(last_page.saturating_add(1)),
                total: clamp_to_i32(total),
                progress,
                available: Library::is_available(book),
            }
        })
        .collect()
}

/// Reading-progress fraction `last_page / total`, guarded so `total == 0`
/// yields `0.0` (no division by zero, never NaN/inf) and the result is clamped
/// to `0.0..=1.0`. A `last_page` past `total` (stale position vs. a shrunken
/// book) clamps to `1.0` rather than overshooting the bar.
fn progress_fraction(last_page: usize, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (last_page as f32 / total as f32).clamp(0.0, 1.0)
}

/// Saturating `usize -> i32` for display counts (Slint ints are `i32`); a value
/// beyond `i32::MAX` clamps rather than wrapping negative.
fn clamp_to_i32(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::Library;
    use std::path::PathBuf;

    // `progress_fraction` is the load-bearing guard; table-test it directly so
    // the `total == 0` and overshoot cases are pinned independently of the
    // filesystem-dependent `carousel_data` availability check.
    #[test]
    fn progress_zero_when_total_zero() {
        // Unread/pageless: guard must return 0.0, never NaN.
        assert_eq!(progress_fraction(0, 0), 0.0);
        assert_eq!(progress_fraction(5, 0), 0.0);
    }

    #[test]
    fn progress_unread_partway_done() {
        assert_eq!(progress_fraction(0, 10), 0.0); // unread
        assert_eq!(progress_fraction(5, 10), 0.5); // partway
        assert_eq!(progress_fraction(10, 10), 1.0); // done
    }

    #[test]
    fn progress_overshoot_clamps_to_one() {
        // Stale position past a shrunken book: clamp, don't overshoot.
        assert_eq!(progress_fraction(99, 10), 1.0);
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
        assert!(lib.add(dir.path().to_path_buf()));

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
        assert!(lib.add(path.clone()));
        drop(dir); // remove the directory from disk

        let rows = carousel_data(&lib);
        assert_eq!(rows.len(), 1, "unavailable book is NOT auto-removed");
        assert!(!rows[0].available);
    }
}
