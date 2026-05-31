//! Presentation-layer view state: which folder is open and the current page.
//! Decode-on-demand in PR1 (the LRU cache arrives in PR2).

use crate::keymap::NavAction;
use gashuu_core::{decode, CoreError, DecodedImage, FolderSource, PageSource};
use std::path::Path;

/// Holds the active page source and the current page index.
pub struct ViewerState {
    source: Option<Box<dyn PageSource>>,
    page_count: usize,
    index: usize,
}

impl ViewerState {
    pub fn new() -> Self {
        Self {
            source: None,
            page_count: 0,
            index: 0,
        }
    }

    /// Replace the active source (used by `open_folder` and by tests).
    pub fn set_source(&mut self, source: Box<dyn PageSource>) {
        self.page_count = source.list_pages().len();
        self.source = Some(source);
        self.index = 0;
    }

    /// Open a folder as the active source, resetting to the first page.
    pub fn open_folder(&mut self, path: &Path) -> Result<(), CoreError> {
        let source = FolderSource::open(path)?;
        self.set_source(Box::new(source));
        Ok(())
    }

    // Read accessors exercised by tests; will be called from the UI layer in a later PR.
    #[allow(dead_code)]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    #[allow(dead_code)]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Apply a navigation action with clamping. Returns true if the index moved.
    pub fn apply(&mut self, action: NavAction) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let last = self.page_count - 1;
        let next = match action {
            NavAction::Next => (self.index + 1).min(last),
            NavAction::Prev => self.index.saturating_sub(1),
        };
        let moved = next != self.index;
        self.index = next;
        moved
    }

    /// Decode the current page on demand. `None` when no pages are loaded.
    pub fn current_image(&self) -> Option<Result<DecodedImage, CoreError>> {
        let source = self.source.as_ref()?;
        if self.page_count == 0 {
            return None;
        }
        Some(
            source
                .read_bytes(self.index)
                .and_then(|bytes| decode(&bytes)),
        )
    }

    /// Status line, e.g. "3 / 100".
    pub fn status_text(&self) -> String {
        if self.page_count == 0 {
            "No folder opened".to_string()
        } else {
            format!("{} / {}", self.index + 1, self.page_count)
        }
    }
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{MockPageSource, PageEntry};
    use std::io::Cursor;

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([9, 9, 9, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn mock_with(pages: usize) -> Box<dyn PageSource> {
        let mut mock = MockPageSource::new();
        mock.expect_list_pages().returning(move || {
            vec![
                PageEntry {
                    path: "p".into(),
                    name: "p".into()
                };
                pages
            ]
        });
        mock.expect_read_bytes().returning(|_| Ok(tiny_png()));
        Box::new(mock)
    }

    #[test]
    fn empty_state_shows_nothing() {
        let state = ViewerState::new();
        assert_eq!(state.page_count(), 0);
        assert_eq!(state.index(), 0);
        assert!(state.current_image().is_none());
        assert_eq!(state.status_text(), "No folder opened");
    }

    #[test]
    fn next_advances_and_clamps_at_last_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 1);
        assert!(state.apply(NavAction::Next));
        assert_eq!(state.index(), 2);
        assert!(!state.apply(NavAction::Next)); // clamped, no move
        assert_eq!(state.index(), 2);
    }

    #[test]
    fn prev_clamps_at_first_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(3));
        state.apply(NavAction::Next);
        assert!(state.apply(NavAction::Prev));
        assert_eq!(state.index(), 0);
        assert!(!state.apply(NavAction::Prev)); // clamped at 0
        assert_eq!(state.index(), 0);
    }

    #[test]
    fn current_image_decodes_current_page() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(2));
        let decoded = state.current_image().unwrap().unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 3));
        assert_eq!(decoded.rgba.len(), 2 * 3 * 4);
    }

    #[test]
    fn status_text_is_one_based() {
        let mut state = ViewerState::new();
        state.set_source(mock_with(100));
        assert_eq!(state.status_text(), "1 / 100");
        state.apply(NavAction::Next);
        assert_eq!(state.status_text(), "2 / 100");
    }
}
