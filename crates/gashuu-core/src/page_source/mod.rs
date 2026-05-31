mod folder;
pub use folder::FolderSource;

use crate::error::CoreError;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    pub path: PathBuf,
    pub name: String,
}

#[cfg_attr(feature = "testing", mockall::automock)]
pub trait PageSource {
    fn list_pages(&self) -> Vec<PageEntry>;
    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError>;
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;

    #[test]
    fn mock_page_source_returns_configured_pages() {
        let mut mock = MockPageSource::new();
        mock.expect_list_pages().returning(|| {
            vec![PageEntry { path: "a.png".into(), name: "a.png".into() }]
        });
        mock.expect_read_bytes().returning(|_| Ok(vec![1, 2, 3]));

        assert_eq!(mock.list_pages().len(), 1);
        assert_eq!(mock.read_bytes(0).unwrap(), vec![1, 2, 3]);
    }
}
