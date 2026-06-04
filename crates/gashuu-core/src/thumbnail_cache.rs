//! Thumbnail cache helpers for persistent on-disk thumbnails.

use crate::error::CoreError;
use crate::image_ops::DecodedImage;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// Build a stable cache key from the source path and thumbnail parameters.
///
/// Uses FNV-1a (a fixed algorithm) instead of `std::hash::DefaultHasher`, whose
/// hash output is not guaranteed stable across Rust versions. A stable key keeps
/// thumbnails cached by a prior build reachable after a toolchain upgrade rather
/// than being silently orphaned. The path is hashed via its platform-native
/// `OsStr` bytes, which is deterministic on a given platform (the cache is local).
pub fn cache_key(path: &Path, mtime_secs: i64, max_side: u32) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    fnv1a(&mut hash, path.as_os_str().as_encoded_bytes());
    fnv1a(&mut hash, &mtime_secs.to_le_bytes());
    fnv1a(&mut hash, &max_side.to_le_bytes());
    format!("{hash:016x}")
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

// Fold `bytes` into `hash` using the FNV-1a step (xor then multiply).
fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
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

    /// Best-effort delete the cover PNGs cached for `path`, across each of the
    /// given `max_sides` variants, and return how many files were removed.
    ///
    /// The cover for a book is keyed on `(path, mtime, max_side)` via the private
    /// [`cache_key`] derivation reused here, so the caller passes the CURRENT
    /// `mtime_secs` (the same value the cover was generated under). If the file's
    /// mtime has since drifted, the recomputed key no longer matches the stored
    /// PNG; that PNG is then an undeletable-but-harmless orphan (the LRU/size cap
    /// reclaims it later). This is best-effort by design: a missing file (cache
    /// miss) and any I/O error are SILENTLY skipped — callers may warn, never
    /// error — so the count is only the files actually unlinked.
    pub fn purge_for(&self, path: &Path, mtime_secs: i64, max_sides: &[u32]) -> usize {
        max_sides
            .iter()
            .filter(|&&max_side| {
                let key = cache_key(path, mtime_secs, max_side);
                let target = self.dir.join(format!("{key}.png"));
                std::fs::remove_file(&target).is_ok()
            })
            .count()
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

    #[test]
    fn put_twice_same_key_returns_latest_image() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());

        // First write: the 2x3 solid-red fixture.
        cache
            .put("overwrite_key", &tiny_decoded_image())
            .expect("first put");

        // Second write to the SAME key with different bytes AND dimensions.
        let second_rgba = [0u8, 0, 255, 255].repeat(4 * 2); // solid blue, 4x2
        let second = DecodedImage::new(second_rgba, 4, 2).expect("valid 4x2 RGBA");
        cache.put("overwrite_key", &second).expect("second put");

        let retrieved = cache.get("overwrite_key").expect("get after overwrite");
        assert_eq!(retrieved.width(), 4, "overwrite must update width");
        assert_eq!(retrieved.height(), 2, "overwrite must update height");
        assert_eq!(
            retrieved.rgba(),
            second.rgba(),
            "overwrite must return the latest bytes"
        );
    }

    #[test]
    fn purge_for_removes_the_cover_cached_under_the_current_mtime() {
        // Seed a cover under the exact key purge_for derives, then purge: the file
        // is unlinked and counted, and a follow-up get misses.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let mtime = 1234;
        let max_side = 320;
        let key = cache_key(path, mtime, max_side);
        cache.put(&key, &tiny_decoded_image()).expect("seed cover");

        let removed = cache.purge_for(path, mtime, &[max_side]);
        assert_eq!(removed, 1, "the matching cover is removed and counted");
        assert!(cache.get(&key).is_none(), "the cover is gone after purge");
    }

    #[test]
    fn purge_for_drifted_mtime_removes_nothing_and_does_not_error() {
        // The cover was generated under one mtime; purging with a DRIFTED mtime
        // derives a different key, so nothing matches: 0 removed, no error, and the
        // original orphan stays on disk (harmless — reclaimed by the size cap later).
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let max_side = 320;
        let stored_key = cache_key(path, 1234, max_side);
        cache
            .put(&stored_key, &tiny_decoded_image())
            .expect("seed cover");

        let removed = cache.purge_for(path, 9999, &[max_side]);
        assert_eq!(removed, 0, "a drifted mtime matches no cover");
        assert!(
            cache.get(&stored_key).is_some(),
            "the orphan stays (best-effort, never an error)"
        );
    }

    #[test]
    fn purge_for_removes_across_multiple_sides() {
        // A book can have covers at several max_side variants; purge_for removes
        // every present variant and counts only the ones that existed.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let mtime = 42;
        // Seed two of the three requested sides; the third (480) is absent.
        for side in [160, 320] {
            let key = cache_key(path, mtime, side);
            cache.put(&key, &tiny_decoded_image()).expect("seed cover");
        }

        let removed = cache.purge_for(path, mtime, &[160, 320, 480]);
        assert_eq!(removed, 2, "only the two present sides are removed/counted");
        assert!(cache.get(&cache_key(path, mtime, 160)).is_none());
        assert!(cache.get(&cache_key(path, mtime, 320)).is_none());
    }

    #[test]
    fn purge_for_empty_sides_removes_nothing() {
        // An empty `max_sides` derives no keys, so there is nothing to purge: 0
        // removed, and the seeded cover stays on disk untouched.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let mtime = 1234;
        let max_side = 320;
        let key = cache_key(path, mtime, max_side);
        cache.put(&key, &tiny_decoded_image()).expect("seed cover");

        let removed = cache.purge_for(path, mtime, &[]);
        assert_eq!(removed, 0, "no sides requested removes no cover");
        assert!(
            cache.get(&key).is_some(),
            "the seeded cover is left intact when no sides are requested"
        );
    }

    #[test]
    fn put_get_roundtrip_preserves_multicolor_pixels() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());

        // 4x4 image whose pixels all carry distinct RGBA values derived from the
        // pixel index, so a channel swap or row-stride bug cannot round-trip
        // unnoticed (a solid-color fixture would hide it).
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for i in 0..(4u8 * 4) {
            rgba.extend_from_slice(&[i, i.wrapping_add(64), i.wrapping_add(128), 255]);
        }
        let original = DecodedImage::new(rgba, 4, 4).expect("valid 4x4 RGBA");

        cache
            .put("multicolor_key", &original)
            .expect("put must succeed");
        let retrieved = cache.get("multicolor_key").expect("get must return Some");
        assert_eq!(retrieved.width(), original.width());
        assert_eq!(retrieved.height(), original.height());
        assert_eq!(
            retrieved.rgba(),
            original.rgba(),
            "multi-color RGBA must round-trip byte-exact"
        );
    }
}
