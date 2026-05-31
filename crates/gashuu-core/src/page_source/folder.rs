use super::{PageEntry, PageSource};
use crate::error::CoreError;

pub struct FolderSource;

impl PageSource for FolderSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        unimplemented!()
    }
    fn read_bytes(&self, _index: usize) -> Result<Vec<u8>, CoreError> {
        unimplemented!()
    }
}
