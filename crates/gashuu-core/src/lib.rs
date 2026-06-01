//! gashuu-core: Slint-independent domain + I/O for the gashuu manga viewer.
//!
//! This crate never depends on `slint`; it returns raw RGBA bytes + dimensions
//! so the presentation layer can convert them to `slint::Image`.

pub mod archive_loader;
pub mod cache;
pub mod error;
pub mod image_ops;
pub mod library;
pub mod page_source;
pub mod settings;
pub mod spread;
#[cfg(test)]
mod test_fixtures;
pub mod thumbnail;
pub mod thumbnail_cache;
pub mod viewport;

pub use archive_loader::ArchiveLoader;
pub use cache::{ImageCache, DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};
pub use error::CoreError;
pub use image_ops::{check_pixel_limit, decode, decode_thumbnail, DecodedImage, MAX_PIXELS};
pub use page_source::{FolderSource, PageEntry, PageSource, RarSource, ZipSource};
pub use settings::{
    CoverMode, FitMode, KeyBindings, ReadingDirection, Settings, SpreadLayout, SpreadMode,
    MAX_RECENT_FILES, SETTINGS_VERSION,
};
pub use spread::{next_leading, normalize_leading, prev_leading, spread_at, Spread};
pub use thumbnail::{generate_thumbnails, DEFAULT_THUMB_MAX_SIDE};
pub use viewport::{
    anchored_zoom, centered_offset, clamp_offset, clamp_zoom, fit_scale, ZOOM_MAX, ZOOM_MIN,
};

#[cfg(feature = "testing")]
pub use page_source::MockPageSource;
