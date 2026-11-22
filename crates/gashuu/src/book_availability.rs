use gashuu_core::Book;

/// Derived availability: whether the book's path currently resolves to a source.
///
/// Availability is not stored. An unavailable book is kept with its reading
/// position preserved; removal is an explicit user action.
pub(crate) fn book_path_exists(book: &Book) -> bool {
    crate::book_path::exists(&crate::book_path::locate(book.id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{BookId, Library};

    #[test]
    fn book_path_exists_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let mut library = Library::new();
        let (present_id, title) = crate::book_path::identify(&folder, true).unwrap();
        let missing_id = BookId::new("/manga/missing.cbz");
        library.add(present_id.clone(), title);
        library.register_opened(&missing_id, "missing", None);
        let present = library
            .books()
            .iter()
            .find(|book| book.id() == &present_id)
            .unwrap();
        let missing = library
            .books()
            .iter()
            .find(|book| book.id() == &missing_id)
            .unwrap();
        assert!(book_path_exists(present));
        assert!(!book_path_exists(missing));
    }
}
