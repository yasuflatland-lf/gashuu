//! Thumbnail cache helpers for persistent on-disk thumbnails.

use crate::error::CoreError;
use crate::image_ops::DecodedImage;
use directories::ProjectDirs;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// Build a stable cache key from the source path and thumbnail parameters.
pub fn cache_key(path: &Path, mtime_secs: i64, max_side: u32) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    mtime_secs.hash(&mut hasher);
    max_side.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Persistent thumbnail cache directory handle.
pub struct ThumbnailCache {
    /// Cache root used for on-disk thumbnail storage.
    dir: PathBuf,
}

impl ThumbnailCache {
    /// Build a cache using the platform cache directory.
    pub fn new() -> Result<ThumbnailCache, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoDataDir)?;
        Ok(Self {
            dir: dirs.cache_dir().join("covers"),
        })
    }

    /// Build a cache rooted at `dir`.
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Look up a thumbnail in the cache.
    ///
    /// Reads `<dir>/<key>.png` and decodes it into a `DecodedImage`. Returns
    /// `None` if the file is missing, unreadable, or not a valid image; it
    /// never panics.
    pub fn get(&self, key: &str) -> Option<DecodedImage> {
        let path = self.dir.join(format!("{key}.png"));
        let bytes = std::fs::read(&path).ok()?;
        crate::image_ops::decode(&bytes).ok()
    }

    /// Store a thumbnail in the cache.
    ///
    /// PNG-encodes `img` at its exact dimensions and writes it atomically to
    /// `<dir>/<key>.png`, creating the cache directory if it does not exist.
    /// The temp-file-then-rename keeps a concurrent reader from seeing a torn file.
    pub fn put(&self, key: &str, img: &DecodedImage) -> Result<(), CoreError> {
        std::fs::create_dir_all(&self.dir)?;

        let raw = image::RgbaImage::from_raw(img.width(), img.height(), img.rgba().to_vec())
            .ok_or_else(|| CoreError::MalformedImage {
                expected: (img.width() as usize)
                    .saturating_mul(img.height() as usize)
                    .saturating_mul(4),
                actual: img.rgba().len(),
            })?;

        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(raw).write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )?;

        let target = self.dir.join(format!("{key}.png"));
        let tmp_path = self.dir.join(format!(".{key}.tmp"));
        std::fs::write(&tmp_path, &png_bytes)?;
        std::fs::rename(&tmp_path, &target)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_key, ThumbnailCache};
    use crate::image_ops::DecodedImage;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn cache_key_is_stable_for_same_inputs() {
        let path = Path::new("/tmp/book/page-001.png");
        let key1 = cache_key(path, 1234, 160);
        let key2 = cache_key(path, 1234, 160);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 16);
    }

    #[test]
    fn cache_key_differs_on_mtime() {
        let path = Path::new("/tmp/book/page-001.png");

        let key1 = cache_key(path, 1234, 160);
        let key2 = cache_key(path, 1235, 160);

        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_key_differs_on_max_side() {
        let path = Path::new("/tmp/book/page-001.png");

        let key1 = cache_key(path, 1234, 160);
        let key2 = cache_key(path, 1234, 161);

        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_key_differs_on_path() {
        let key1 = cache_key(Path::new("/tmp/book/page-001.png"), 1234, 160);
        let key2 = cache_key(Path::new("/tmp/book/page-002.png"), 1234, 160);

        assert_ne!(key1, key2);
    }

    /// Synthesize a tiny 2x3 solid-red RGBA DecodedImage in memory (no files).
    /// Canonical in-test fixture for ThumbnailCache round-trip tests.
    fn tiny_decoded_image() -> DecodedImage {
        // 2 x 3 x 4 bytes = 24 bytes of solid red RGBA.
        let rgba = [255u8, 0, 0, 255].repeat(2 * 3);
        DecodedImage::new(rgba, 2, 3).expect("valid 2x3 RGBA")
    }

    #[test]
    fn put_creates_directory_if_absent() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested_subdir");
        assert!(
            !nested.exists(),
            "pre-condition: nested_subdir must not exist"
        );
        let cache = ThumbnailCache::with_dir(nested.clone());
        cache
            .put("abc123", &tiny_decoded_image())
            .expect("put should succeed");
        assert!(nested.exists(), "put must create the target directory");
    }

    #[test]
    fn put_writes_key_dot_png_inside_dir() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let key = "cafebabe12345678";
        cache
            .put(key, &tiny_decoded_image())
            .expect("put must succeed");
        let expected_path = dir.path().join(format!("{key}.png"));
        assert!(
            expected_path.exists(),
            "expected file at {expected_path:?} to exist"
        );
    }

    #[test]
    fn get_missing_key_returns_none() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        assert!(cache.get("nonexistent_key").is_none());
    }

    #[test]
    fn get_corrupt_file_returns_none() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let key = "deadbeef01234567";
        let path = dir.path().join(format!("{key}.png"));
        std::fs::write(&path, b"this is not a PNG").unwrap();
        assert!(cache.get(key).is_none());
    }

    #[test]
    fn put_get_roundtrip_preserves_dims_and_bytes() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let original = tiny_decoded_image();
        cache
            .put("roundtrip_key", &original)
            .expect("put must succeed");

        let retrieved = cache
            .get("roundtrip_key")
            .expect("get must return Some after put");
        assert_eq!(retrieved.width(), original.width(), "width mismatch");
        assert_eq!(retrieved.height(), original.height(), "height mismatch");
        assert_eq!(retrieved.rgba(), original.rgba(), "RGBA bytes mismatch");
    }
}
