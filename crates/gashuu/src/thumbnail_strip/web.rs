//! The browser demo leaves the thumbnail strip empty.

use crate::ViewerWindow;
use gashuu_core::PageSource;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ThumbnailController;

impl ThumbnailController {
    pub fn new(_ui: &ViewerWindow) -> Self {
        Self
    }

    /// Leave the strip empty so thumbnail decodes do not compete with page turns.
    pub fn start(
        &self,
        _ui_weak: slint::Weak<ViewerWindow>,
        _source: Option<Arc<dyn PageSource>>,
        _page_count: usize,
        _path: Option<PathBuf>,
    ) {
    }
}
