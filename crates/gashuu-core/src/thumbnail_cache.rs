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

#[cfg(test)]
mod tests {
    use super::cache_key;
    use std::path::Path;

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
}
