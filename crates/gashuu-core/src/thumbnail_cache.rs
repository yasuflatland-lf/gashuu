//! Thumbnail cache helpers for persistent on-disk thumbnails.

use crate::blob_store::project_dirs;
use crate::image_ops::{encode_qoi, DecodedImage};
use crate::thumb_store::{BookFingerprint, ClearCacheReport, PruneReport, ThumbSlot, ThumbStore};
use crate::{BookId, CoreError};
use std::path::{Path, PathBuf};

/// Wall clock for filesystem mtimes, which `std::fs` reads and writes as
/// `std::time::SystemTime`.
#[allow(clippy::disallowed_methods)]
fn fs_clock_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// Filesystem mtime of `path` as whole seconds since the Unix epoch, or `0`
/// when the file is missing / has no readable mtime. The ONLY implementation
/// of the key-ingredient recipe — generation and purge must both use it.
pub(crate) fn source_mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Age past which a `write_atomic` temp file (`.tmpXXXXXX`) is a crash leftover,
/// not an in-flight `put` (whose temp-write-then-rename lives milliseconds).
/// [`FsThumbStore::prune`] reclaims tmp files older than this regardless of the
/// size cap.
const TMP_STALE_SECS: u64 = 60 * 60;

/// `NamedTempFile`'s default name prefix. `write_atomic` (and therefore `put`)
/// creates its same-dir temp via `NamedTempFile::new_in`, whose default name
/// shape is `.tmpXXXXXX` — a `.tmp` PREFIX with random trailing chars, NOT a
/// `.tmp` suffix. The cache-ownership matcher keys off this prefix so `prune`
/// reliably reclaims a temp stranded by an interrupted write.
const TMP_NAME_PREFIX: &str = ".tmp";

/// Persistent thumbnail cache directory handle.
pub struct FsThumbStore {
    /// Cache root used for on-disk thumbnail storage.
    dir: PathBuf,
}

impl FsThumbStore {
    /// Build a cache using the platform cache directory.
    pub fn new() -> Result<Self, CoreError> {
        let dirs = project_dirs().ok_or(CoreError::NoCacheDir)?;
        Ok(Self {
            dir: dirs.cache_dir().join("thumbnails"),
        })
    }

    /// Build a cache rooted at `dir`.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn entry_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.qoi"))
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
    fn get_by_key(&self, key: &str) -> Option<DecodedImage> {
        let path = self.entry_path(key);
        let bytes = std::fs::read(&path).ok()?;
        let img = crate::image_ops::decode(&bytes).ok()?;
        let _ = std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(fs_clock_now()));
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
    fn put_by_key(&self, key: &str, img: &DecodedImage) -> Result<(), CoreError> {
        let qoi_bytes = encode_qoi(img)?;
        let target = self.entry_path(key);
        crate::atomic_write::write_atomic(&target, &qoi_bytes)?;
        Ok(())
    }
}

impl ThumbStore for FsThumbStore {
    fn fingerprint(&self, id: &BookId) -> BookFingerprint {
        let path = Path::new(id.as_str());
        BookFingerprint::of_path_and_mtime(path, source_mtime_secs(path))
    }

    fn get(&self, fp: BookFingerprint, slot: ThumbSlot, max_side: u32) -> Option<DecodedImage> {
        self.get_by_key(&slot.key(fp, max_side))
    }

    fn put(
        &self,
        fp: BookFingerprint,
        slot: ThumbSlot,
        max_side: u32,
        img: &DecodedImage,
    ) -> Result<(), CoreError> {
        self.put_by_key(&slot.key(fp, max_side), img)
    }

    fn purge_cover(&self, fp: BookFingerprint, max_sides: &[u32]) -> usize {
        max_sides
            .iter()
            .filter(|&&max_side| {
                let key = fp.cover_key(max_side);
                let target = self.entry_path(&key);
                std::fs::remove_file(&target).is_ok()
            })
            .count()
    }

    fn purge_pages(&self, fp: BookFingerprint, max_sides: &[u32], page_count: usize) -> usize {
        let mut removed = 0;
        for &max_side in max_sides {
            for i in 0..page_count {
                let key = fp.page_key(max_side, i);
                let target = self.entry_path(&key);
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
    fn clear(&self) -> ClearCacheReport {
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
    /// Best-effort like [`purge_cover`](Self::purge_cover): a missing directory is a
    /// zero report (first launch), unreadable entries are skipped, a failed
    /// unlink is skipped (not counted) and the sweep continues. Files the cache
    /// does not own (neither `*.qoi`/legacy `*.png` nor its tmp shape) are NEVER
    /// touched.
    fn prune(&self, max_bytes: u64) -> PruneReport {
        let mut report = PruneReport::default();

        // One stored thumbnail, as the eviction pass needs it: unlink target,
        // size for the running total, and the (mtime, name) sort key.
        struct CacheEntry {
            path: PathBuf,
            size: u64,
            mtime: std::time::SystemTime,
            name: std::ffi::OsString,
        }

        let now = fs_clock_now();
        let mut entries: Vec<CacheEntry> = Vec::new();
        for_each_cache_file(&self.dir, |entry, meta| {
            let name = entry.file_name();
            let lossy = name.to_string_lossy();
            // A `put` interrupted between temp write and rename strands its `.tmpXXXXXX`
            // temp; reclaim ones past the stale threshold (a younger tmp may be in-flight).
            if lossy.starts_with(TMP_NAME_PREFIX) {
                let mtime = meta.modified().unwrap_or(now);
                let stale = match now.duration_since(mtime) {
                    Ok(age) => age.as_secs() >= TMP_STALE_SECS,
                    // mtime ahead of the clock: reclaim only when implausibly far ahead.
                    Err(ahead) => ahead.duration().as_secs() >= TMP_STALE_SECS,
                };
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

/// The shared scaffold of the directory sweeps ([`FsThumbStore::clear`] /
/// [`FsThumbStore::prune`]): visit each TOP-LEVEL regular file in `dir`, calling
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

#[cfg(test)]
#[path = "thumbnail_cache/tests.rs"]
mod tests;
