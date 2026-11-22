//! Platform file dialogs and their thread-safe selection results.

use crate::backend::Done;
use std::{path::PathBuf, rc::Rc};

#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) mod web;

pub(crate) enum PickedBook {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Path(PathBuf),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Upload(UploadHandle),
}

/// A Send handle to a browser File owned by the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadHandle {
    pub key: u32,
    pub name: String,
    pub size: u64,
}

pub(crate) trait BookPicker {
    /// Deliver on the UI thread after returning. An empty selection means cancel.
    fn pick_books(&self, done: Done<Vec<PickedBook>>);
    fn supports_folders(&self) -> bool;
    fn pick_folder(&self, done: Done<Option<PickedBook>>);
}

pub(crate) fn platform_picker() -> Rc<dyn BookPicker> {
    #[cfg(not(target_arch = "wasm32"))]
    return Rc::new(desktop::DesktopPicker);
    #[cfg(target_arch = "wasm32")]
    return Rc::new(web::WebPicker::new());
}

pub(crate) fn forget_uploads(uploads: &[UploadHandle]) {
    #[cfg(target_arch = "wasm32")]
    web::forget_files(&uploads.iter().map(|upload| upload.key).collect::<Vec<_>>());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = uploads;
}

#[cfg(test)]
mod tests;
