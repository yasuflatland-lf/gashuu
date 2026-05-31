//! gashuu-core: Slint-independent domain + I/O for the gashuu manga viewer.
//!
//! This crate never depends on `slint`; it returns raw RGBA bytes + dimensions
//! so the presentation layer can convert them to `slint::Image`.

pub mod cache;
pub mod error;
pub mod image_ops;
pub mod page_source;
pub mod settings;

pub use cache::{ImageCache, DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};
pub use error::CoreError;
pub use image_ops::{decode, DecodedImage};
pub use page_source::{FolderSource, PageEntry, PageSource};
pub use settings::{
    KeyBindings, ReadingDirection, Settings, SpreadMode, MAX_RECENT_FILES, SETTINGS_VERSION,
};

#[cfg(feature = "testing")]
pub use page_source::MockPageSource;
