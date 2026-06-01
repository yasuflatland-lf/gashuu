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
}

/// An ordered, de-duplicated shelf of books. Insertion order is the carousel
/// order. Identity / dedup is on the canonical path (best-effort canonicalized at
/// `add` time). Mirrors `Settings::push_recent` discipline: one place owns the
/// invariants.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
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
