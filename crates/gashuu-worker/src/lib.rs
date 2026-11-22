//! Portable protocol and request dispatch for the gashuu Web Worker.

pub mod book_path;
pub mod cover_store;
pub mod dispatcher;
pub mod import;
pub mod protocol;
pub mod wire_error;

#[cfg(test)]
mod tests;
