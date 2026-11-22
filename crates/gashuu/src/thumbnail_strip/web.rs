//! The browser demo leaves the thumbnail strip empty.

use crate::backend::{BookBackend, SessionId};
use crate::ViewerWindow;
use std::rc::Rc;

pub struct ThumbnailController;

impl ThumbnailController {
    pub fn new(_ui: &ViewerWindow, _backend: Rc<dyn BookBackend>) -> Self {
        Self
    }

    /// Leave the strip empty so thumbnail decodes do not compete with page turns.
    pub fn start(
        &self,
        _ui_weak: slint::Weak<ViewerWindow>,
        _session: Option<SessionId>,
        _page_count: usize,
    ) {
    }
}
