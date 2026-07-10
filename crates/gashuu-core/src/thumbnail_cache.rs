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

/// Build a stable cache key for a single thumbnail-strip page.
///
/// Identical FNV-1a construction to [`cache_key`] PLUS the `page_index` folded in
/// after `max_side`. Folding the page index keeps strip keys disjoint from cover
/// keys (covers never fold a page index), so a strip thumbnail and a cover at the
/// same `(path, mtime_secs, max_side)` never collide on disk — even at page 0,
/// which still folds eight extra zero bytes through the hash.
pub fn page_cache_key(path: &Path, mtime_secs: i64, max_side: u32, page_index: usize) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    fnv1a(&mut hash, path.as_os_str().as_encoded_bytes());
    fnv1a(&mut hash, &mtime_secs.to_le_bytes());
    fnv1a(&mut hash, &max_side.to_le_bytes());
    fnv1a(&mut hash, &page_index.to_le_bytes());
    format!("{hash:016x}")
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Age past which a `write_atomic` temp file (`.tmpXXXXXX`) is a crash leftover,
/// not an in-flight `put` (whose temp-write-then-rename lives milliseconds).
/// [`ThumbnailCache::prune`] reclaims tmp files older than this regardless of the
/// size cap.
const TMP_STALE_SECS: u64 = 60 * 60;

/// `NamedTempFile`'s default name prefix. `write_atomic` (and therefore `put`)
/// creates its same-dir temp via `NamedTempFile::new_in`, whose default name
/// shape is `.tmpXXXXXX` — a `.tmp` PREFIX with random trailing chars, NOT a
/// `.tmp` suffix. The cache-ownership matcher keys off this prefix so `prune`
/// reliably reclaims a temp stranded by an interrupted write.
const TMP_NAME_PREFIX: &str = ".tmp";

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

    /// Look up a thumbnail in the cache, WRITING a hit's file mtime as a side
    /// effect (touch-on-get; detailed below).
    ///
    /// Reads `<dir>/<key>.qoi` and decodes it into a `DecodedImage`. Returns
    /// `None` if the file is missing, unreadable, or not a valid image; it
    /// never panics. A legacy `<key>.png` from a prior build is NOT read — it is
    /// a clean miss (regenerated under the `.qoi` extension) and is reclaimed by
    /// [`prune`](Self::prune)/[`clear`](Self::clear).
    ///
    /// A hit refreshes the file's mtime (touch-on-get), which is what makes
    /// [`prune`](Self::prune)'s mtime-ascending eviction near-LRU: live covers
    /// are re-touched every launch while a key-orphaned or corrupt entry keeps
    /// aging. The touch runs only AFTER a successful decode (corrupt files must
    /// age out) and is best-effort — a failure (entry pruned mid-read, read-only
    /// fs) costs nothing but eviction-order precision.
    pub fn get(&self, key: &str) -> Option<DecodedImage> {
        let path = self.dir.join(format!("{key}.qoi"));
        let bytes = std::fs::read(&path).ok()?;
        let img = crate::image_ops::decode(&bytes).ok()?;
        let _ = std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        Some(img)
    }

    /// Store a thumbnail in the cache.
    ///
    /// QOI-encodes `img` at its exact dimensions and writes it atomically to
    /// `<dir>/<key>.qoi` via [`write_atomic`](crate::atomic_write::write_atomic),
    /// which creates the cache directory if it does not exist and is the single
    /// owner of the temp-then-rename invariant (same-dir `NamedTempFile` +
    /// `sync_all` + rename). The atomic rename keeps a concurrent reader from
    /// seeing a torn file, and the random temp name means concurrent `put`s of
    /// the same key never collide on a fixed temp path.
    pub fn put(&self, key: &str, img: &DecodedImage) -> Result<(), CoreError> {
        let qoi_bytes = encode_qoi(img)?;
        let target = self.dir.join(format!("{key}.qoi"));
        crate::atomic_write::write_atomic(&target, &qoi_bytes)?;
        Ok(())
    }

    /// Best-effort delete the cover QOI files cached for `path`, across each of the
    /// given `max_sides` variants, and return how many files were removed.
    ///
    /// The cover for a book is keyed on `(path, mtime, max_side)` via the private
    /// [`cache_key`] derivation reused here, so the caller passes the CURRENT
    /// `mtime_secs` (the same value the cover was generated under). If the file's
    /// mtime has since drifted, the recomputed key no longer matches the stored
    /// file; that file is then an undeletable-but-harmless orphan ([`prune`](Self::prune)
    /// reclaims it later — never `get`-touched, it ages to the front of the
    /// eviction order). This is best-effort by design: a missing file (cache
    /// miss) and any I/O error are SILENTLY skipped — callers may warn, never
    /// error — so the count is only the files actually unlinked.
    pub fn purge_for(&self, path: &Path, mtime_secs: i64, max_sides: &[u32]) -> usize {
        max_sides
            .iter()
            .filter(|&&max_side| {
                let key = cache_key(path, mtime_secs, max_side);
                let target = self.dir.join(format!("{key}.qoi"));
                std::fs::remove_file(&target).is_ok()
            })
            .count()
    }

    /// Best-effort delete the per-page STRIP QOI files cached for `path` — every
    /// `max_side` in `max_sides` crossed with every page index in `0..page_count`
    /// — and return how many files were removed.
    ///
    /// The strip sibling of [`purge_for`](Self::purge_for): where that reclaims a
    /// book's single COVER key `(path, mtime, max_side)` via [`cache_key`], this
    /// reclaims the per-page keys `(path, mtime, max_side, i)` via
    /// [`page_cache_key`] — the exact derivation the strip generator wrote them
    /// under — so a removed book's strip thumbnails are targeted-deleted instead of
    /// orphaned until the size-cap sweep. Page index 0 is INCLUDED: `page_cache_key`
    /// folds the page index even at 0 (eight zero bytes), so page 0's strip has its
    /// own on-disk file distinct from the cover.
    ///
    /// The caller passes the CURRENT `mtime_secs` (the value the strips were
    /// generated under). If the file's mtime has since drifted, the recomputed keys
    /// no longer match the stored files; those become undeletable-but-harmless
    /// orphans ([`prune`](Self::prune) reclaims them later). Best-effort by design,
    /// exactly like [`purge_for`](Self::purge_for): a missing file (cache miss) and
    /// any I/O error are SILENTLY skipped — callers may warn, never error — so the
    /// count is only the files actually unlinked.
    pub fn purge_pages_for(
        &self,
        path: &Path,
        mtime_secs: i64,
        max_sides: &[u32],
        page_count: usize,
    ) -> usize {
        let mut removed = 0;
        for &max_side in max_sides {
            for i in 0..page_count {
                let key = page_cache_key(path, mtime_secs, max_side, i);
                let target = self.dir.join(format!("{key}.qoi"));
                if std::fs::remove_file(&target).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }

    /// Clear every top-level file owned by this cache directory and report what
    /// was actually removed.
    ///
    /// Missing or unreadable cache directories are treated as empty, matching
    /// [`prune`](Self::prune)'s best-effort cleanup style. The sweep never
    /// recurses and only unlinks regular files whose names match the patterns
    /// this cache owns: `*.qoi` thumbnails (current), legacy `*.png` thumbnails
    /// (reclaimed, never read), and `.tmpXXXXXX` interrupted `write_atomic` temps.
    pub fn clear(&self) -> ClearCacheReport {
        let mut report = ClearCacheReport::default();
        for_each_cache_file(&self.dir, |entry, meta| {
            if !is_owned_cache_file_name(&entry.file_name()) {
                return;
            }
            if std::fs::remove_file(entry.path()).is_ok() {
                report.removed_files += 1;
                report.removed_bytes += meta.len();
            }
        });
        report
    }

    /// Sweep the cache directory down to `max_bytes` of stored `*.qoi` payload,
    /// evicting in ascending `(mtime, file name)` order — oldest first, the name
    /// as a deterministic tie-break. `get` refreshes a hit's mtime, so this order
    /// is near-LRU: a key-orphaned cover (its source's mtime drifted) is never
    /// read again, ages to the front, and is reclaimed once the cap is hit.
    ///
    /// Legacy `*.png` entries from a prior build are never read (a clean miss
    /// under the new `.qoi` extension), so the sweep unlinks every one it finds
    /// as a one-time migration reclaim — counted in the removed totals but not in
    /// the `.qoi` size cap.
    ///
    /// Best-effort like [`purge_for`](Self::purge_for): a missing directory is a
    /// zero report (first launch), unreadable entries are skipped, a failed
    /// unlink is skipped (not counted) and the sweep continues. Files the cache
    /// does not own (neither `*.qoi`/legacy `*.png` nor its tmp shape) are NEVER
    /// touched.
    pub fn prune(&self, max_bytes: u64) -> PruneReport {
        let mut report = PruneReport::default();

        // One stored thumbnail, as the eviction pass needs it: unlink target,
        // size for the running total, and the (mtime, name) sort key.
        struct CacheEntry {
            path: PathBuf,
            size: u64,
            mtime: std::time::SystemTime,
            name: std::ffi::OsString,
        }

        let now = std::time::SystemTime::now();
        let mut entries: Vec<CacheEntry> = Vec::new();
        for_each_cache_file(&self.dir, |entry, meta| {
            let name = entry.file_name();
            let lossy = name.to_string_lossy();
            // A `put` interrupted between temp write and rename strands its `.tmpXXXXXX`
            // temp; reclaim ones past the stale threshold (a younger tmp may be in-flight).
            if lossy.starts_with(TMP_NAME_PREFIX) {
                let mtime = meta.modified().unwrap_or(now);
                let stale = now
                    .duration_since(mtime)
                    .map(|age| age.as_secs() >= TMP_STALE_SECS)
                    .unwrap_or(false);
                if stale && std::fs::remove_file(entry.path()).is_ok() {
                    report.removed_files += 1;
                    report.removed_bytes += meta.len();
                }
                return;
            }
            // Legacy PNG entries are never read under the new `.qoi` extension;
            // reclaim every one as a one-time migration sweep (outside the cap).
            if lossy.ends_with(".png") {
                if std::fs::remove_file(entry.path()).is_ok() {
                    report.removed_files += 1;
                    report.removed_bytes += meta.len();
                }
                return;
            }
            if lossy.ends_with(".qoi") {
                entries.push(CacheEntry {
                    path: entry.path(),
                    size: meta.len(),
                    // An unreadable mtime sorts as the epoch — evicted first,
                    // which is the safe end for an entry we know nothing about.
                    mtime: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    name,
                });
            }
        });

        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        entries.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.name.cmp(&b.name)));
        let mut victims = entries.into_iter();
        while total > max_bytes {
            let Some(entry) = victims.next() else {
                // Every remaining unlink failed; the leftover bytes stay counted.
                break;
            };
            if std::fs::remove_file(&entry.path).is_ok() {
                report.removed_files += 1;
                report.removed_bytes += entry.size;
                total -= entry.size;
            }
        }
        report.retained_bytes = total;
        report
    }
}

// Names this cache owns and may reclaim: `.qoi` thumbnails, legacy `.png` (reclaim-only,
// never read after the codec switch), and `.tmpXXXXXX` `write_atomic` temps.
fn is_owned_cache_file_name(name: &std::ffi::OsStr) -> bool {
    let lossy = name.to_string_lossy();
    lossy.ends_with(".qoi") || lossy.ends_with(".png") || lossy.starts_with(TMP_NAME_PREFIX)
}

/// The shared scaffold of the directory sweeps ([`ThumbnailCache::clear`] /
/// [`ThumbnailCache::prune`]): visit each TOP-LEVEL regular file in `dir`, calling
/// `visit(&entry, &meta)` with the entry and its non-following (symlink) metadata.
/// A missing or unreadable directory yields nothing (best-effort, never an error —
/// e.g. first launch); the walk never recurses and skips anything that is not a
/// regular file. Each sweep supplies its own per-entry policy (owned-name filter,
/// tmp age-guard, size-cap collection) as the closure.
fn for_each_cache_file(dir: &Path, mut visit: impl FnMut(&std::fs::DirEntry, &std::fs::Metadata)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        visit(&entry, &meta);
    }
}

/// The write-side codec: QOI-encode `img` at its exact dimensions. QOI is a fast
/// lossless codec — these are regenerable throwaway caches where portability is
/// irrelevant, so the slow PNG encode/decode is traded for QOI's speed. Split from
/// the atomic-write/path concern in [`ThumbnailCache::put`] so the encode is
/// testable in isolation. Errors with `MalformedImage` if the RGBA buffer length
/// does not match `width * height * 4`, or propagates an `image` encode failure.
fn encode_qoi(img: &DecodedImage) -> Result<Vec<u8>, CoreError> {
    let raw = image::RgbaImage::from_raw(img.width(), img.height(), img.rgba().to_vec())
        .ok_or_else(|| CoreError::MalformedImage {
            expected: (img.width() as usize)
                .saturating_mul(img.height() as usize)
                .saturating_mul(4),
            actual: img.rgba().len(),
        })?;

    let mut qoi_bytes: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(raw).write_to(
        &mut std::io::Cursor::new(&mut qoi_bytes),
        image::ImageFormat::Qoi,
    )?;
    Ok(qoi_bytes)
}

/// Result of one [`ThumbnailCache::clear`] sweep. The cache is best-effort:
/// these counts describe entries actually unlinked, not merely discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClearCacheReport {
    /// Cache-owned files actually unlinked by the clear operation.
    pub removed_files: usize,
    /// Sum of the unlinked files' sizes, in bytes.
    pub removed_bytes: u64,
}

/// Result of one [`ThumbnailCache::prune`] sweep. A named struct (not a tuple)
/// so call sites read; the caller (UI) logs these — core stays log-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PruneReport {
    /// Files actually unlinked by the sweep.
    pub removed_files: usize,
    /// Sum of the unlinked files' sizes, in bytes.
    pub removed_bytes: u64,
    /// Post-sweep total of the surviving `*.qoi` payload, in bytes — lets the
    /// caller log "under the cap" without re-scanning the directory.
    pub retained_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::{cache_key, page_cache_key, ClearCacheReport, PruneReport, ThumbnailCache};
    use crate::image_ops::DecodedImage;
    use std::path::Path;
    use tempfile::tempdir;

    /// Set `path`'s mtime to the exact `target` instant. Tests needing EQUAL
    /// mtimes across files must share one target (two `now()` reads differ).
    fn set_mtime(path: &Path, target: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(target))
            .expect("set fixture mtime");
    }

    /// Back-date `path`'s mtime by `secs_ago` seconds, so a test controls the
    /// eviction order `prune` sees (and the age `get`'s touch must advance).
    fn back_date(path: &Path, secs_ago: u64) {
        let target = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        set_mtime(path, target);
    }

    /// On-disk size of the cached QOI file for `key` (seeded via `put`).
    fn qoi_size(dir: &Path, key: &str) -> u64 {
        std::fs::metadata(dir.join(format!("{key}.qoi")))
            .expect("seeded qoi exists")
            .len()
    }

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
    fn page_cache_key_is_stable_for_same_inputs() {
        let path = Path::new("/manga/book.cbz");
        let key1 = page_cache_key(path, 1234, 160, 7);
        let key2 = page_cache_key(path, 1234, 160, 7);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 16);
    }

    #[test]
    fn page_cache_key_differs_on_page_index() {
        let path = Path::new("/manga/book.cbz");
        let key0 = page_cache_key(path, 1234, 160, 0);
        let key1 = page_cache_key(path, 1234, 160, 1);

        assert_ne!(key0, key1, "different pages must not share a strip key");
    }

    #[test]
    fn page_cache_key_is_disjoint_from_cover_key() {
        // A strip thumbnail folds the page index even at page 0, so it never
        // collides with the cover key at the same (path, mtime, max_side).
        let path = Path::new("/manga/book.cbz");
        let cover = cache_key(path, 1234, 160);
        let page0 = page_cache_key(path, 1234, 160, 0);

        assert_ne!(cover, page0, "strip keys must be disjoint from cover keys");
    }

    /// Synthesize a tiny 2x3 solid-red RGBA DecodedImage in memory (no files).
    /// Canonical in-test fixture for ThumbnailCache round-trip tests.
    fn tiny_decoded_image() -> DecodedImage {
        // 2 x 3 x 4 bytes = 24 bytes of solid red RGBA.
        let rgba = [255u8, 0, 0, 255].repeat(2 * 3);
        DecodedImage::new(rgba, 2, 3).expect("valid 2x3 RGBA")
    }

    /// Encode `img` as VALID legacy PNG bytes — the format the cache wrote before
    /// the QOI switch. Used to seed `.png` files for the migration tests so a miss
    /// can only come from the new extension, never a decode failure.
    fn encode_png_for_test(img: &DecodedImage) -> Vec<u8> {
        let raw = image::RgbaImage::from_raw(img.width(), img.height(), img.rgba().to_vec())
            .expect("valid RGBA buffer");
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(raw)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .expect("PNG encode");
        png_bytes
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
    fn put_writes_key_dot_qoi_inside_dir() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let key = "cafebabe12345678";
        cache
            .put(key, &tiny_decoded_image())
            .expect("put must succeed");
        let expected_path = dir.path().join(format!("{key}.qoi"));
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
        let path = dir.path().join(format!("{key}.qoi"));
        std::fs::write(&path, b"this is not a QOI").unwrap();
        assert!(cache.get(key).is_none());
    }

    #[test]
    fn get_ignores_legacy_png_as_a_clean_miss() {
        // A `.png` from a prior build is never read under the new `.qoi` extension: the
        // lookup misses (file later reclaimed by prune/clear), so it regenerates as `.qoi`.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let key = "1234567890abcdef";
        // Seed a VALID legacy PNG so a miss can only come from the extension,
        // not from a decode failure.
        let legacy = encode_png_for_test(&tiny_decoded_image());
        std::fs::write(dir.path().join(format!("{key}.png")), &legacy).unwrap();

        assert!(
            cache.get(key).is_none(),
            "a legacy .png must be a clean miss under the .qoi extension"
        );
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
        // The cover was generated under one mtime; purging with a DRIFTED mtime derives a
        // different key, so nothing matches: 0 removed, no error, orphan stays (prune later).
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
    fn purge_pages_for_removes_all_strip_thumbnails_for_a_book() {
        // A book's per-page strip thumbnails live under page_cache_key; purge_pages_for
        // reclaims all 0..page_count, while the cover (plain cache_key) survives the purge.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let mtime = 1234;
        let max_side = 160;
        let page_count = 5;

        // Seed one strip thumbnail per page under the exact key purge_pages_for derives.
        for i in 0..page_count {
            let key = page_cache_key(path, mtime, max_side, i);
            cache.put(&key, &tiny_decoded_image()).expect("seed strip");
        }
        // Seed the cover too (disjoint key — it must NOT be swept by the strip purge).
        let cover_key = cache_key(path, mtime, max_side);
        cache
            .put(&cover_key, &tiny_decoded_image())
            .expect("seed cover");

        let removed = cache.purge_pages_for(path, mtime, &[max_side], page_count);
        assert_eq!(
            removed, page_count,
            "every strip page is removed and counted"
        );
        for i in 0..page_count {
            let key = page_cache_key(path, mtime, max_side, i);
            assert!(
                cache.get(&key).is_none(),
                "strip page {i} is gone after the strip purge"
            );
        }
        assert!(
            cache.get(&cover_key).is_some(),
            "the cover survives the strip purge (it is purge_for's job)"
        );

        // The sibling purge_for then reclaims the cover.
        let removed_cover = cache.purge_for(path, mtime, &[max_side]);
        assert_eq!(removed_cover, 1, "the cover is reclaimed by purge_for");
        assert!(
            cache.get(&cover_key).is_none(),
            "the cover is gone after purge_for"
        );
    }

    #[test]
    fn purge_pages_for_overshooting_page_count_skips_missing_keys() {
        // A page_count larger than the strips on disk is best-effort: extra indices derive
        // keys with no file, so only the present pages are removed/counted — no error.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = Path::new("/manga/book.cbz");
        let mtime = 42;
        let max_side = 160;
        let present = 3;

        for i in 0..present {
            let key = page_cache_key(path, mtime, max_side, i);
            cache.put(&key, &tiny_decoded_image()).expect("seed strip");
        }

        // Overshoot: ask for 10 pages though only 3 strips exist on disk.
        let removed = cache.purge_pages_for(path, mtime, &[max_side], 10);
        assert_eq!(
            removed, present,
            "only the present strips are removed/counted; the missing keys are skipped"
        );
        for i in 0..present {
            let key = page_cache_key(path, mtime, max_side, i);
            assert!(
                cache.get(&key).is_none(),
                "present strip {i} was removed by the overshooting purge"
            );
        }
    }

    #[test]
    fn clear_missing_dir_returns_zero_report() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().join("never_created"));

        let report = cache.clear();

        assert_eq!(
            report,
            ClearCacheReport::default(),
            "missing cache dir clears successfully with a zero report"
        );
    }

    #[test]
    fn clear_removes_owned_cache_files_and_reports_them() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed qoi");
        cache.put("bbbb", &tiny_decoded_image()).expect("seed qoi");
        // A `write_atomic` temp uses the `.tmpXXXXXX` prefix shape.
        let tmp = dir.path().join(".tmpcccc01");
        std::fs::write(&tmp, b"interrupted cache write").unwrap();
        let expected_bytes = qoi_size(dir.path(), "aaaa")
            + qoi_size(dir.path(), "bbbb")
            + tmp.metadata().unwrap().len();

        let report = cache.clear();

        assert_eq!(report.removed_files, 3);
        assert_eq!(report.removed_bytes, expected_bytes);
        assert!(cache.get("aaaa").is_none());
        assert!(cache.get("bbbb").is_none());
        assert!(!tmp.exists(), "cache temp files are owned entries too");
    }

    #[test]
    fn clear_keeps_foreign_files_and_nested_entries() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed qoi");
        let stray = dir.path().join("README.txt");
        std::fs::write(&stray, b"not a cache entry").unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let nested_qoi = nested.join("inside.qoi");
        std::fs::write(&nested_qoi, b"nested foreign content").unwrap();

        let report = cache.clear();

        assert_eq!(
            report.removed_files, 1,
            "only the top-level owned qoi is removed"
        );
        assert!(stray.exists(), "foreign files in the cache dir survive");
        assert!(
            nested_qoi.exists(),
            "clear does not recurse into directories"
        );
    }

    #[test]
    fn prune_missing_dir_returns_zero_report() {
        // First launch: the cache directory was never written. Prune must be a
        // no-op zero report, never an error or a panic.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().join("never_created"));

        let report = cache.prune(0);

        assert_eq!(report, PruneReport::default(), "missing dir prunes nothing");
    }

    #[test]
    fn prune_under_cap_removes_nothing() {
        // Total payload exactly at the cap: nothing is evicted and the report
        // carries the surviving total so the caller can log it.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed");
        cache.put("bbbb", &tiny_decoded_image()).expect("seed");
        let total = qoi_size(dir.path(), "aaaa") + qoi_size(dir.path(), "bbbb");

        let report = cache.prune(total);

        assert_eq!(report.removed_files, 0, "at-cap total evicts nothing");
        assert_eq!(report.removed_bytes, 0);
        assert_eq!(report.retained_bytes, total, "report carries the total");
        assert!(cache.get("aaaa").is_some(), "both entries survive");
        assert!(cache.get("bbbb").is_some());
    }

    #[test]
    fn prune_over_cap_removes_oldest_first_until_under_cap() {
        // Three entries, distinct ages. A cap that fits exactly the two newest
        // must evict ONLY the oldest, and the report carries the exact numbers.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        for (key, age) in [("oldest", 3000), ("middle", 2000), ("newest", 1000)] {
            cache.put(key, &tiny_decoded_image()).expect("seed");
            back_date(&dir.path().join(format!("{key}.qoi")), age);
        }
        let oldest = qoi_size(dir.path(), "oldest");
        let cap = qoi_size(dir.path(), "middle") + qoi_size(dir.path(), "newest");

        let report = cache.prune(cap);

        assert_eq!(report.removed_files, 1, "only the oldest is evicted");
        assert_eq!(report.removed_bytes, oldest);
        assert_eq!(report.retained_bytes, cap, "survivors sum to the cap");
        assert!(cache.get("oldest").is_none(), "oldest entry is gone");
        assert!(cache.get("middle").is_some(), "newer entries survive");
        assert!(cache.get("newest").is_some());
    }

    #[test]
    fn prune_equal_mtime_tie_breaks_by_file_name() {
        // Two entries with the SAME mtime and a cap that fits only one: the file-name
        // tie-break makes eviction deterministic (ascending name, so "aaaa.qoi" first).
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        // ONE shared instant: per-file `now()` reads would differ by microseconds
        // and turn this into an mtime-order test instead of a tie-break test.
        let shared = std::time::SystemTime::now() - std::time::Duration::from_secs(2000);
        for key in ["bbbb", "aaaa"] {
            cache.put(key, &tiny_decoded_image()).expect("seed");
            set_mtime(&dir.path().join(format!("{key}.qoi")), shared);
        }
        let cap = qoi_size(dir.path(), "bbbb");

        let report = cache.prune(cap);

        assert_eq!(report.removed_files, 1);
        assert!(
            cache.get("aaaa").is_none(),
            "name tie-break evicts aaaa first"
        );
        assert!(cache.get("bbbb").is_some());
    }

    #[test]
    fn prune_zero_cap_removes_all_qois() {
        // Degenerate cap: every cached QOI goes; the directory itself stays.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed");
        cache.put("bbbb", &tiny_decoded_image()).expect("seed");

        let report = cache.prune(0);

        assert_eq!(report.removed_files, 2, "cap 0 evicts every entry");
        assert_eq!(report.retained_bytes, 0);
        assert!(cache.get("aaaa").is_none());
        assert!(cache.get("bbbb").is_none());
    }

    #[test]
    fn prune_ignores_files_that_are_not_cache_entries() {
        // Safety pin: the sweep only deletes what the cache owns (`*.qoi`, legacy `*.png`,
        // stale tmp). A stray foreign file survives even a zero cap.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let stray = dir.path().join("README.txt");
        std::fs::write(&stray, b"not a cache entry").unwrap();
        back_date(&stray, 9999);

        let report = cache.prune(0);

        assert_eq!(report.removed_files, 0, "foreign files are never touched");
        assert!(stray.exists(), "the stray file survives the sweep");
    }

    #[test]
    fn prune_removes_stale_tmp_even_under_cap() {
        // A crash between `put`'s temp write and rename strands a `.tmpXXXXXX` temp. The
        // sweep reclaims tmp files past the stale threshold regardless of the cap.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed");
        let stale_tmp = dir.path().join(".tmpdeadbeef");
        std::fs::write(&stale_tmp, b"torn write leftover").unwrap();
        back_date(&stale_tmp, 2 * 60 * 60); // 2 h old: well past the threshold

        let report = cache.prune(u64::MAX);

        assert_eq!(report.removed_files, 1, "the stale tmp is reclaimed");
        assert_eq!(report.removed_bytes, b"torn write leftover".len() as u64);
        assert!(!stale_tmp.exists(), "the stale tmp is gone");
        assert!(cache.get("aaaa").is_some(), "the cached QOI is untouched");
    }

    #[test]
    fn prune_keeps_fresh_tmp() {
        // A tmp file younger than the stale threshold may be an in-flight `put`
        // on another thread — the age guard protects it, even under a zero cap.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let fresh_tmp = dir.path().join(".tmpcafebabe");
        std::fs::write(&fresh_tmp, b"write in flight").unwrap();

        let report = cache.prune(0);

        assert_eq!(report.removed_files, 0, "an in-flight tmp is protected");
        assert!(fresh_tmp.exists(), "the fresh tmp survives the sweep");
    }

    #[test]
    fn prune_reclaims_legacy_png_outside_the_cap() {
        // Legacy `.png` entries are never read under the `.qoi` extension, so prune reclaims
        // every one regardless of the cap; only `.qoi` payload counts against the size cap.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("live", &tiny_decoded_image()).expect("seed qoi");
        let legacy = dir.path().join("dead.png");
        std::fs::write(&legacy, encode_png_for_test(&tiny_decoded_image())).unwrap();
        let legacy_bytes = legacy.metadata().unwrap().len();
        // A generous cap that the `.qoi` payload alone fits under.
        let cap = qoi_size(dir.path(), "live");

        let report = cache.prune(cap);

        assert_eq!(report.removed_files, 1, "only the legacy png is reclaimed");
        assert_eq!(report.removed_bytes, legacy_bytes);
        assert_eq!(
            report.retained_bytes, cap,
            "retained total counts the .qoi payload only"
        );
        assert!(!legacy.exists(), "the legacy png is gone");
        assert!(cache.get("live").is_some(), "the live qoi survives");
    }

    #[test]
    fn clear_reclaims_legacy_png() {
        // `clear` recognizes legacy `.png` as an owned entry and unlinks it
        // alongside current `.qoi` thumbnails.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("live", &tiny_decoded_image()).expect("seed qoi");
        let legacy = dir.path().join("dead.png");
        std::fs::write(&legacy, encode_png_for_test(&tiny_decoded_image())).unwrap();
        let expected_bytes = qoi_size(dir.path(), "live") + legacy.metadata().unwrap().len();

        let report = cache.clear();

        assert_eq!(report.removed_files, 2, "both qoi and legacy png cleared");
        assert_eq!(report.removed_bytes, expected_bytes);
        assert!(!legacy.exists(), "the legacy png is gone");
        assert!(cache.get("live").is_none(), "the qoi is gone");
    }

    #[test]
    fn get_hit_advances_file_mtime() {
        // The near-LRU contract: a HIT refreshes the file's mtime so `prune`'s ascending
        // eviction approximates LRU — a cover read each launch outlives yesterday's orphan.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        cache.put("aaaa", &tiny_decoded_image()).expect("seed");
        let qoi = dir.path().join("aaaa.qoi");
        back_date(&qoi, 1000);
        let before = std::fs::metadata(&qoi).unwrap().modified().unwrap();

        assert!(cache.get("aaaa").is_some(), "pre-condition: a cache hit");

        let after = std::fs::metadata(&qoi).unwrap().modified().unwrap();
        assert!(
            after > before,
            "a hit must advance the mtime (touch-on-get)"
        );
    }

    #[test]
    fn get_on_corrupt_file_does_not_touch_mtime() {
        // A corrupt entry must KEEP its old mtime: touching it would push the
        // garbage to the back of the eviction order and keep it alive forever.
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let qoi = dir.path().join("aaaa.qoi");
        std::fs::write(&qoi, b"this is not a QOI").unwrap();
        back_date(&qoi, 1000);
        let before = std::fs::metadata(&qoi).unwrap().modified().unwrap();

        assert!(
            cache.get("aaaa").is_none(),
            "pre-condition: a decode failure"
        );

        let after = std::fs::metadata(&qoi).unwrap().modified().unwrap();
        assert_eq!(after, before, "a failed decode must not refresh the mtime");
    }

    #[test]
    fn is_owned_recognizes_write_atomic_temp_name() {
        // `put` writes through `write_atomic`, whose `NamedTempFile` default name is
        // `.tmpXXXXXX` (a `.tmp` PREFIX). The matcher must recognize it so `prune` reclaims it.
        let dir = tempdir().unwrap();
        let tmp = tempfile::NamedTempFile::new_in(dir.path()).expect("create temp in cache dir");
        let name = tmp
            .path()
            .file_name()
            .expect("temp file has a name")
            .to_owned();
        assert!(
            super::is_owned_cache_file_name(&name),
            "prune must recognize a write_atomic temp ({name:?}) as reclaimable"
        );
    }

    #[test]
    fn put_get_roundtrip_preserves_multicolor_pixels() {
        let dir = tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());

        // 4x4 image whose pixels carry distinct RGBA values (from the pixel index), so a
        // channel-swap or row-stride bug can't round-trip unnoticed (a solid fixture would).
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
