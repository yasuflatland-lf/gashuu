use gashuu_core::Book;

/// Derived availability: whether the book's path currently resolves on disk.
///
/// Availability is not stored. An unavailable book is kept with its reading
/// position preserved; removal is an explicit user action.
pub(crate) fn book_path_exists(book: &Book) -> bool {
    book.path().exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{canonical_identity, Library};
    use std::path::PathBuf;

    #[test]
    fn book_path_exists_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let mut library = Library::new();
        let present_path = canonical_identity(&folder);
        let missing_path = PathBuf::from("/manga/missing.cbz");
        library.add(present_path.clone());
        library.register_opened(&missing_path, None);
        let present = library
            .books()
            .iter()
            .find(|book| book.path() == present_path)
            .unwrap();
        let missing = library
            .books()
            .iter()
            .find(|book| book.path() == missing_path)
            .unwrap();
        assert!(book_path_exists(present));
        assert!(!book_path_exists(missing));
    }
}
