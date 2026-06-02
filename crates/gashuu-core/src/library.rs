//! Library domain model: an ordered, de-duplicated shelf of books.
//!
//! Headless (no slint, no tracing). Identity is the canonical filesystem path;
//! availability is derived (never stored). Persistence lives in `library_store`.

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
    fn is_available_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let present = Book::from_path(folder.clone());
        let missing = Book::from_path(PathBuf::from("/manga/missing.cbz"));
        assert!(Library::is_available(&present));
        assert!(!Library::is_available(&missing));
    }
}
