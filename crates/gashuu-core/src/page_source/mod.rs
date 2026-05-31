mod folder;
pub use folder::FolderSource;

use crate::error::CoreError;
use std::path::PathBuf;

/// A single page in a source: its filesystem path and display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    /// Absolute or source-relative path used to read the page bytes.
    pub path: PathBuf,
    /// Display name (typically the file name).
    pub name: String,
}

/// An ordered collection of image pages with raw byte access by index.
///
/// Implementations must keep the page list stable for the lifetime of the
/// instance (`ViewerState` caches `list_pages().len()`), and `read_bytes(index)`
/// must return [`CoreError::IndexOutOfRange`] when `index >= list_pages().len()`.
#[cfg_attr(feature = "testing", mockall::automock)]
pub trait PageSource {
    /// All pages in display order.
    fn list_pages(&self) -> Vec<PageEntry>;
    /// Read the raw, still-encoded bytes of the page at `index`.
    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError>;
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;

    #[test]
    fn mock_page_source_returns_configured_pages() {
        let mut mock = MockPageSource::new();
        mock.expect_list_pages().returning(|| {
            vec![PageEntry {
                path: "a.png".into(),
                name: "a.png".into(),
            }]
        });
        mock.expect_read_bytes().returning(|_| Ok(vec![1, 2, 3]));

        assert_eq!(mock.list_pages().len(), 1);
        assert_eq!(mock.read_bytes(0).unwrap(), vec![1, 2, 3]);
    }
}
