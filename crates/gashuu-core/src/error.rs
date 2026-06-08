//! Typed errors for gashuu-core. The presentation layer formats these with
//! color-eyre; this crate never depends on slint or eyre.

/// Errors produced by gashuu-core I/O and decoding.
///
/// Marked `#[non_exhaustive]` so variants can be added without breaking
/// downstream `match` arms. PR6 added the archive variants (`Zip`,
/// `EntryTooLarge`, `UnsupportedFormat`); later work (e.g. caching) may add
/// more.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// Filesystem read/walk failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A page index was requested outside the available range.
    #[error("page index {index} out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },

    /// The `image` crate failed to decode the bytes.
    #[error("image decode error: {0}")]
    Decode(#[from] image::ImageError),

    /// A decoded image's RGBA buffer length did not match its dimensions.
    #[error("malformed image: expected {expected} RGBA bytes, got {actual}")]
    MalformedImage { expected: usize, actual: usize },

    /// Settings file could not be (de)serialized.
    #[error("settings format error: {0}")]
    Settings(#[from] serde_json::Error),

    /// The OS did not provide a config directory for settings storage.
    #[error("no config directory available for settings")]
    NoConfigDir,

    /// Library file could not be (de)serialized. NOTE: deliberately NOT
    /// `#[from] serde_json::Error` - `CoreError::Settings` already owns that
    /// `From` impl, and a type can have only one. Construct explicitly via
    /// `.map_err(CoreError::Library)`.
    #[error("library format error: {0}")]
    Library(serde_json::Error),

    /// The OS did not provide a data directory for library storage.
    #[error("no data directory available for library")]
    NoDataDir,

    /// The archive backend failed to open the container or read an entry.
    #[error("archive error: {0}")]
    Zip(#[from] ::zip::result::ZipError),

    /// An entry's data exceeded the per-file size ceiling on read.
    #[error("entry too large: {name} exceeds {max} bytes")]
    EntryTooLarge { name: String, max: u64 },

    /// The path is neither a directory nor a recognized archive.
    #[error("unsupported format: {path}")]
    UnsupportedFormat { path: String },

    /// The source contains no displayable image pages.
    #[error("no images found in {path}")]
    EmptyBook { path: String },

    /// The image dimensions exceed the maximum allowed pixel count.
    #[error("image too large: {width}x{height} = {pixels} px exceeds {max}")]
    ImageTooLarge {
        width: u32,
        height: u32,
        pixels: u64,
        max: u64,
    },

    /// The RAR backend failed to open the container or read an entry.
    #[error("rar archive error: {0}")]
    Rar(#[from] ::unrar::error::UnrarError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_out_of_range_displays_index_and_len() {
        let err = CoreError::IndexOutOfRange { index: 5, len: 3 };
        assert_eq!(err.to_string(), "page index 5 out of range (len 3)");
    }

    #[test]
    fn malformed_image_displays_expected_and_actual() {
        let err = CoreError::MalformedImage {
            expected: 16,
            actual: 3,
        };
        assert_eq!(
            err.to_string(),
            "malformed image: expected 16 RGBA bytes, got 3"
        );
    }

    #[test]
    fn no_config_dir_displays_message() {
        let err = CoreError::NoConfigDir;
        assert_eq!(
            err.to_string(),
            "no config directory available for settings"
        );
    }

    #[test]
    fn image_too_large_displays_dimensions_pixels_and_max() {
        let err = CoreError::ImageTooLarge {
            width: 10,
            height: 20,
            pixels: 200,
            max: 100,
        };
        assert_eq!(
            err.to_string(),
            "image too large: 10x20 = 200 px exceeds 100"
        );
    }

    #[test]
    fn settings_displays_with_prefix() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = CoreError::Settings(json_err);
        assert!(err.to_string().starts_with("settings format error: "));
    }

    #[test]
    fn zip_displays_with_prefix() {
        let err = CoreError::Zip(::zip::result::ZipError::FileNotFound);
        assert!(err.to_string().starts_with("archive error: "));
    }

    #[test]
    fn entry_too_large_displays_name_and_max() {
        let err = CoreError::EntryTooLarge {
            name: "p.png".into(),
            max: 10,
        };
        assert_eq!(err.to_string(), "entry too large: p.png exceeds 10 bytes");
    }

    #[test]
    fn unsupported_format_displays_path() {
        let err = CoreError::UnsupportedFormat {
            path: "/x.txt".into(),
        };
        assert_eq!(err.to_string(), "unsupported format: /x.txt");
    }

    #[test]
    fn library_displays_with_prefix() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = CoreError::Library(json_err);
        assert!(err.to_string().starts_with("library format error: "));
    }

    #[test]
    fn no_data_dir_displays_message() {
        let err = CoreError::NoDataDir;
        assert_eq!(err.to_string(), "no data directory available for library");
    }
}
