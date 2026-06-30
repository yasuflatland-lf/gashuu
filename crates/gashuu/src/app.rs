//! Facade for the open-book and bulk-remove use cases, split into `open_book`
//! and `remove_books` (#241): the open path, empty-book removal, and bulk delete
//! share no state and no call path, so each lives in its own module. This facade
//! keeps the established `crate::app::*` paths stable for existing call sites.

pub(crate) use crate::open_book::{
    book_display_title, remove_empty_book, NoticesContent, OpenBookUseCase, OpenOutcome,
    SkippedDetail,
};
pub(crate) use crate::remove_books::{confirm_delete_content, RemoveBooksUseCase};
