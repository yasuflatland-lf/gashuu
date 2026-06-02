//! Library domain model: an ordered, de-duplicated shelf of books.
//!
//! Headless (no slint, no tracing). Identity is the canonical filesystem path;
//! availability is derived (never stored). Persistence lives in `library_store`.

use crate::reading_progress::ReadingProgress;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One book in the shelf. Identity is the canonical filesystem path.
///
/// Carries display data only: the path, a derived `title` (file stem for a file,
/// directory name for a folder), and the leading page index of the last-viewed
/// spread. The book kind (folder / archive) is resolved at open time by
/// `ArchiveLoader` and is deliberately NOT persisted. Availability (whether the
/// path still resolves) is derived at render time, never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    path: PathBuf,
    title: String,
    #[serde(default)]
    last_page: usize,
    /// Total page count, cached at open time. `0` is the unknown sentinel (set
    /// for a freshly added book and for any older `library.json` missing the
    /// field).
    #[serde(default)]
    page_count: usize,
}

impl Book {
    /// Build a `Book` from a path, deriving the display title. The title is the
    /// file stem (e.g. `Cool Title` from `Cool Title.cbz`) or the directory name
    /// for an extension-less folder, falling back to the lossy full path string
    /// so the title is never empty.
    pub(crate) fn from_path(path: PathBuf) -> Self {
        let title = if path.is_dir() {
            path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
        } else {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
        }
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self {
            path,
            title,
            last_page: 0,
            page_count: 0,
        }
    }

    /// The canonical filesystem path that identifies this book.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Display title (derived: file stem for a file, directory name for a folder).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Leading page index of the last-viewed spread (0 when never opened).
    pub fn last_page(&self) -> usize {
        self.last_page
    }

    /// Total page count cached at open time (0 when unknown).
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// This book's reading progress as a value object: the last-viewed leading
    /// page index together with the cached total page count. The `current` /
    /// `fraction` derivation lives on `ReadingProgress`, not at the call sites.
    pub fn progress(&self) -> ReadingProgress {
        ReadingProgress::new(self.last_page, self.page_count)
    }
}

/// An ordered, de-duplicated shelf of books. Insertion order is the carousel
/// order. Identity / dedup is on the canonical path (best-effort canonicalized at
/// `add` time). Mirrors `Settings::push_recent` discipline: one place owns the
/// invariants.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    books: Vec<Book>,
}

/// Outcome of [`Library::register_opened`]: where to resume reading and whether the
/// stored page count changed (so the caller can decide whether to rebuild the
/// carousel). A pure value — no I/O implied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenRegistration {
    /// The position to resume at (the book's recorded `ReadingProgress`).
    pub resume: ReadingProgress,
    /// Whether the stored page count actually changed during registration.
    pub count_changed: bool,
}

impl Library {
    /// An empty library.
    pub fn new() -> Self {
        Self::default()
    }

    /// The books in insertion (carousel) order.
    pub fn books(&self) -> &[Book] {
        &self.books
    }

    /// Add `path` to the shelf. Canonicalizes best-effort (falling back to the
    /// path verbatim when canonicalization fails - e.g. a missing file), derives
    /// the title, and de-duplicates by canonical path. Returns `false` (no-op)
    /// when the canonical path is already present.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or(path);
        if self.books.iter().any(|b| b.path() == canonical) {
            return false;
        }
        self.books.push(Book::from_path(canonical));
        true
    }

    /// Remove the book identified by `path`. Returns `false` when absent.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.books.len();
        self.books.retain(|b| b.path() != path);
        self.books.len() != before
    }

    /// The last-viewed leading page index for `path` (0 when unknown).
    pub fn last_page(&self, path: &Path) -> usize {
        self.books
            .iter()
            .find(|b| b.path() == path)
            .map(Book::last_page)
            .unwrap_or(0)
    }

    /// Record `page` as the last-viewed leading page index for `path`. Returns
    /// `false` when the path is absent OR the value is unchanged (mirrors the
    /// `jump_to` "did it actually move" convention, so callers can skip a save).
    pub fn set_last_page(&mut self, path: &Path, page: usize) -> bool {
        match self.books.iter_mut().find(|b| b.path() == path) {
            Some(book) if book.last_page != page => {
                book.last_page = page;
                true
            }
            _ => false,
        }
    }

    /// Record the total `count` of pages for `path`. Returns `false` when the
    /// path is absent OR the value is unchanged (mirrors `set_last_page`, so
    /// callers can skip a save). `count` must be positive: `0` is the unknown
    /// sentinel and is never a valid measured page count.
    pub fn set_page_count(&mut self, path: &Path, count: usize) -> bool {
        debug_assert!(
            count > 0,
            "set_page_count: count must be > 0; 0 is the unknown sentinel"
        );
        match self.books.iter_mut().find(|b| b.path() == path) {
            Some(book) if book.page_count != count => {
                book.page_count = count;
                true
            }
            _ => false,
        }
    }

    /// Register a freshly opened book and report how to resume it. Idempotent add
    /// by canonical path, then back-fill the page count when known (`page_count >
    /// 0`; `0` is the unknown sentinel — an empty/fully-skipped source — and is
    /// left untouched, so `set_page_count`'s `> 0` invariant is never violated).
    /// Returns the resume position (the book's `ReadingProgress`) and whether the
    /// stored count changed, so the caller can decide to rebuild the carousel.
    /// No persistence I/O; the only filesystem touch is the best-effort
    /// `canonicalize` inside `add`, which is idempotent when `canonical` is already
    /// canonical (the result is unchanged, though the syscall still runs; as it is
    /// when read from `open_file`). `canonical` is the
    /// canonicalized open key (the same key `last_page`/`set_page_count` use).
    pub fn register_opened(&mut self, canonical: &Path, page_count: usize) -> OpenRegistration {
        self.add(canonical.to_path_buf());
        let count_changed = page_count > 0 && self.set_page_count(canonical, page_count);
        // The book was just added (or already present), so the lookup by the
        // same canonical key always resolves; assert it to catch a future
        // path-identity regression. The fallback keeps a never-panicking
        // production path.
        let found = self.books.iter().find(|b| b.path() == canonical);
        debug_assert!(
            found.is_some(),
            "register_opened: book not found by canonical path immediately after add"
        );
        let resume = found
            .map(Book::progress)
            .unwrap_or(ReadingProgress::new(0, 0));
        OpenRegistration {
            resume,
            count_changed,
        }
    }

    /// Derived availability: whether the book's path currently resolves on disk.
    /// This is NOT stored - an unavailable book is kept (its reading position is
    /// preserved); removal is an explicit user action.
    pub fn is_available(book: &Book) -> bool {
        book.path().exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_derives_title_from_file_stem() {
        let book = Book::from_path(PathBuf::from("/manga/Cool Title.cbz"));
        assert_eq!(book.title(), "Cool Title");
        assert_eq!(book.path(), Path::new("/manga/Cool Title.cbz"));
        assert_eq!(book.last_page(), 0);
    }

    #[test]
    fn book_derives_title_from_dir_name_when_no_extension() {
        // A folder book has no extension; the title is the directory name.
        let book = Book::from_path(PathBuf::from("/manga/My Folder Book"));
        assert_eq!(book.title(), "My Folder Book");
    }

    #[test]
    fn book_derives_title_from_dotted_existing_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let book = Book::from_path(folder);
        assert_eq!(book.title(), "Series.1997");
    }

    #[test]
    fn book_title_falls_back_to_path_string_when_no_stem() {
        // Pathological path with no file_stem (e.g. root) falls back to the lossy
        // path string so the title is never empty.
        let book = Book::from_path(PathBuf::from("/"));
        assert!(!book.title().is_empty());
    }

    #[test]
    fn new_library_is_empty() {
        let lib = Library::new();
        assert!(lib.books().is_empty());
    }

    #[test]
    fn add_appends_in_insertion_order() {
        // Non-existent paths exercise the best-effort canonicalize fallback
        // (canonicalize fails - path is used verbatim).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")));
        assert!(lib.add(PathBuf::from("/manga/b.cbz")));
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["a", "b"], "insertion order must be preserved");
    }

    #[test]
    fn add_dedups_by_path_and_returns_false() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")));
        assert!(
            !lib.add(PathBuf::from("/manga/a.cbz")),
            "adding an existing path must be a no-op returning false"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_canonicalizes_existing_path_for_identity() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();

        let mut lib = Library::new();
        assert!(lib.add(folder.join(".")));
        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.books()[0].path(),
            folder.canonicalize().unwrap().as_path()
        );
        assert_eq!(lib.books()[0].title(), "Series.1997");
    }

    #[test]
    fn remove_existing_returns_true_and_drops_book() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.remove(&path));
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut lib = Library::new();
        assert!(!lib.remove(Path::new("/manga/missing.cbz")));
    }

    #[test]
    fn last_page_is_zero_for_unknown_path() {
        let lib = Library::new();
        assert_eq!(lib.last_page(Path::new("/manga/missing.cbz")), 0);
    }

    #[test]
    fn set_last_page_updates_and_round_trips() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_last_page(&path, 42));
        assert_eq!(lib.last_page(&path), 42);
    }

    #[test]
    fn set_last_page_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_last_page(Path::new("/manga/missing.cbz"), 12));
    }

    #[test]
    fn set_last_page_false_when_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_last_page(&path, 3));
        assert!(!lib.set_last_page(&path, 3));
    }

    #[test]
    fn page_count_defaults_to_zero_for_fresh_book() {
        let book = Book::from_path(PathBuf::from("/manga/a.cbz"));
        assert_eq!(book.page_count(), 0);
    }

    #[test]
    fn set_page_count_updates_and_returns_true() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_page_count(&path, 42));
        assert_eq!(lib.books()[0].page_count(), 42);
    }

    #[test]
    fn set_page_count_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_page_count(Path::new("/manga/missing.cbz"), 12));
    }

    #[test]
    fn set_page_count_false_when_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_page_count(&path, 3));
        assert!(!lib.set_page_count(&path, 3));
    }

    #[test]
    fn book_without_page_count_field_deserializes_to_zero() {
        // Backward compatibility: an older `Book` JSON object that predates the
        // `page_count` field must still deserialize, defaulting to the unknown
        // sentinel (0).
        let value = serde_json::json!({
            "path": "/manga/a.cbz",
            "title": "a",
            "last_page": 5,
        });
        let book: Book = serde_json::from_value(value).unwrap();
        assert_eq!(book.path(), Path::new("/manga/a.cbz"));
        assert_eq!(book.last_page(), 5);
        assert_eq!(book.page_count(), 0);
    }

    #[test]
    fn progress_is_unread_for_freshly_added_book() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        let p = lib.books()[0].progress();
        assert!(p.is_unread(), "freshly added book must be unread");
        assert_eq!(p.reached(), 0);
        assert_eq!(p.total(), 0);
    }

    #[test]
    fn progress_reflects_last_page_and_page_count() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_last_page(&path, 3));
        assert!(lib.set_page_count(&path, 10));
        let p = lib.books()[0].progress();
        assert_eq!(p.reached(), 3);
        assert_eq!(p.total(), 10);
        assert_eq!(p.current(), 4);
        let expected: f32 = 3.0 / 10.0;
        assert!(
            (p.fraction() - expected).abs() < 1e-6,
            "fraction should be ~0.3, got {}",
            p.fraction()
        );
    }

    #[test]
    fn is_available_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let present = Book::from_path(folder.clone());
        let missing = Book::from_path(PathBuf::from("/manga/missing.cbz"));
        assert!(Library::is_available(&present));
        assert!(!Library::is_available(&missing));
    }

    #[test]
    fn register_opened_adds_fresh_book_and_back_fills_count() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        let reg = lib.register_opened(&path, 10);
        assert_eq!(lib.books().len(), 1, "fresh open must add the book");
        assert!(reg.count_changed, "0 -> 10 must report a count change");
        assert!(reg.resume.is_unread(), "fresh book resumes as unread");
        assert_eq!(reg.resume.reached(), 0);
        assert_eq!(reg.resume.total(), 10);
    }

    #[test]
    fn register_opened_is_idempotent_and_reports_no_change_when_count_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, 10);
        let reg = lib.register_opened(&path, 10);
        assert_eq!(lib.books().len(), 1, "re-opening must not duplicate");
        assert!(
            !reg.count_changed,
            "an unchanged count must report no change"
        );
    }

    #[test]
    fn register_opened_skips_set_page_count_for_unknown_sentinel() {
        // page_count == 0 is the unknown sentinel: the `> 0` guard skips
        // set_page_count, so its debug_assert is never hit and total stays 0.
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/b.cbz");
        let reg = lib.register_opened(&path, 0);
        assert_eq!(lib.books().len(), 1, "the book is still added");
        assert!(!reg.count_changed, "the sentinel must not change the count");
        assert_eq!(reg.resume.total(), 0, "count stays the unknown sentinel");
    }

    #[test]
    fn register_opened_resumes_at_recorded_last_page() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()));
        assert!(lib.set_last_page(&path, 5));
        let reg = lib.register_opened(&path, 10);
        assert_eq!(reg.resume.reached(), 5, "resume reflects the recorded page");
        assert_eq!(reg.resume.current(), 6, "current is reached + 1");
    }

    #[test]
    fn register_opened_twice_preserves_recorded_last_page() {
        // Re-opening a book must NOT reset its resume position: the idempotent
        // add inside register_opened keeps the existing entry (and its last_page).
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&p, 10);
        assert!(lib.set_last_page(&p, 42));
        let reg = lib.register_opened(&p, 10);
        assert_eq!(lib.books().len(), 1, "re-open must not duplicate the book");
        assert_eq!(reg.resume.reached(), 42, "re-open must preserve last_page");
        assert!(!reg.count_changed, "same count on re-open is no change");
    }

    #[test]
    fn register_opened_reports_count_change_on_new_nonzero_count() {
        // A legitimately changed (non-zero -> different non-zero) page count is
        // reported as count_changed, flowing set_page_count's "did it move" branch.
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.register_opened(&p, 10).count_changed);
        let reg = lib.register_opened(&p, 12);
        assert!(reg.count_changed, "10 -> 12 is a real change");
        assert_eq!(reg.resume.total(), 12);
    }
}
