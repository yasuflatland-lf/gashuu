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
