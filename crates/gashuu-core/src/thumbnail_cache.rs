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
    #[allow(dead_code)]
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
    pub fn get(&self, _key: &str) -> Option<DecodedImage> {
        None
    }

    /// Store a thumbnail in the cache.
    pub fn put(&self, _key: &str, _img: &DecodedImage) -> Result<(), CoreError> {
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

    #[test]
    fn with_dir_constructs_and_get_returns_none_skeleton() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let image = DecodedImage::new(vec![0u8; 4], 1, 1).unwrap();

        assert!(cache.get("page-001").is_none());
        assert_eq!(image.width(), 1);
    }

    #[test]
    fn put_is_noop_skeleton_and_writes_nothing() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let image = DecodedImage::new(vec![0u8; 4], 1, 1).unwrap();

        cache.put("page-001", &image).unwrap();

        assert!(dir.path().read_dir().unwrap().next().is_none());
    }
}
