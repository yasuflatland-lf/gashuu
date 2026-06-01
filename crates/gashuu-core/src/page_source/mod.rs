mod folder;
mod naming;
mod rar;
mod zip;
pub use folder::FolderSource;
pub use rar::RarSource;
pub use zip::ZipSource;

use crate::error::CoreError;
use std::path::PathBuf;

/// A single page in a source: its filesystem path and display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    /// Source-specific page identifier. For filesystem sources this is a real,
    /// readable path; for archive sources it is a logical entry name used only
    /// for display and identity — bytes are always retrieved via `read_bytes(index)`.
    pub path: PathBuf,
    /// Display name (typically the file name).
    pub name: String,
}

/// An ordered collection of image pages with raw byte access by index.
///
/// Implementations must keep the page list stable for the lifetime of the
/// instance (`ViewerState` caches `list_pages().len()`), and `read_bytes(index)`
/// must return [`CoreError::IndexOutOfRange`] when `index >= list_pages().len()`.
///
/// The `Send + Sync` bound lets `ImageCache` share the source as `Arc<dyn PageSource>`
/// with rayon worker threads during background prefetch. `read_bytes` may therefore
/// be called concurrently from multiple threads and must be safe to do so.
#[cfg_attr(feature = "testing", mockall::automock)]
pub trait PageSource: Send + Sync {
    /// All pages in display order.
    fn list_pages(&self) -> Vec<PageEntry>;
    /// Read the raw, still-encoded bytes of the page at `index`.
    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError>;
    /// Entries silently dropped during `open`. Implementations increment this
    /// for entries they cannot safely include — e.g. unreadable directory
    /// entries in `FolderSource`; zip-slip, oversized, or corrupt entries in
    /// `ZipSource`. Default 0; concrete sources override.
    fn skipped_count(&self) -> usize {
        0
    }
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

#[cfg(all(test, feature = "testing"))]
mod send_sync_tests {
    use super::*;
    use std::sync::Arc;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn page_source_impls_are_send_sync() {
        // FolderSource and the generated mock must both satisfy the supertrait so
        // they can become `Arc<dyn PageSource>` shared with rayon.
        assert_send_sync::<FolderSource>();
        assert_send_sync::<ZipSource>();
        assert_send_sync::<RarSource>();
        assert_send_sync::<MockPageSource>();
        // And the trait object itself must be Send + Sync.
        fn _accepts(_: Arc<dyn PageSource>) {}
    }
}
