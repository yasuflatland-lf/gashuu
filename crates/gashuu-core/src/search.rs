//! Search predicates over the library domain.
//!
//! These are pure, headless domain rules ("does this `Book` match a query?")
//! shared by the presentation layer's visible-row projection. The projection
//! itself (mapping a match onto carousel row indices) stays in the UI crate;
//! only the predicate lives here, next to the other `Book`/`Library`
//! derivations (`display_title`, `ReadingProgress`).

use crate::Book;

/// Case-insensitive substring match of `query` against a book's display title
/// and ID. An empty query matches every book (no filter).
///
/// The query is matched verbatim (not trimmed): callers pass the exact debounced
/// search text, so leading/trailing whitespace is significant by design.
pub fn book_matches(book: &Book, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let needle = query.to_lowercase();
    let title = book.title().to_lowercase();
    let id = book.id().as_str().to_lowercase();

    title.contains(&needle) || id.contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BookId;
    use crate::Library;

    #[test]
    fn book_matches_title_and_path_case_insensitively() {
        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/Akira/Vol 01.cbz"), "Vol 01".into())
            .is_some());
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
        assert!(lib
            .add(BookId::new("/manga/One Piece.cbz"), "One Piece".into())
            .is_some());
        assert!(book_matches(&lib.books()[0], ""));
    }
}
