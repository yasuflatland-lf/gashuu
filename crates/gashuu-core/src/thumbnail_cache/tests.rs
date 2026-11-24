#[cfg(target_pointer_width = "64")]
use super::encode_qoi;
use super::{source_mtime_secs, ClearCacheReport, PruneReport};
use crate::{
    BookFingerprint, BookId, DecodedImage, FsThumbStore, NullThumbStore, ThumbSlot, ThumbStore,
};
use std::path::Path;
use tempfile::tempdir;

#[cfg(target_pointer_width = "64")]
#[test]
fn fs_store_writes_the_legacy_file_name() {
    let dir = tempdir().unwrap();
    let store = FsThumbStore::with_dir(dir.path().to_owned());
    let path = Path::new("/books/a.cbz");
    let mtime = 1_700_000_000;
    let fp = BookFingerprint::of_path_and_mtime(path, mtime);
    let image = tiny_decoded_image();
    store.put(fp, ThumbSlot::Cover, 512, &image).unwrap();
    store.put(fp, ThumbSlot::Page(3), 160, &image).unwrap();
    for key in ["8888a774ce848046", "6c298ddf8718b0b3"] {
        assert!(dir.path().join(format!("{key}.qoi")).is_file());
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);

    std::fs::write(
        dir.path().join("8b2454e89207fad4.qoi"),
        encode_qoi(&image).unwrap(),
    )
    .unwrap();
    let hit = store.get(fp, ThumbSlot::Page(4), 160).unwrap();
    assert_eq!(hit.rgba(), image.rgba());
}

#[test]
fn fs_store_fingerprint_uses_current_mtime() {
    let dir = tempdir().unwrap();
    let store = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"book").unwrap();
    let id = BookId::from_canonical_path(&path).unwrap();
    for seconds in [1_000, 2_000] {
        set_mtime(
            &path,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
        );
        assert_eq!(
            store.fingerprint(&id),
            BookFingerprint::of_path_and_mtime(&path, seconds as i64)
        );
    }
}

#[test]
fn null_store_is_inert() {
    let store = NullThumbStore;
    let id = BookId::new("immutable:本");
    let fp = store.fingerprint(&id);
    assert_eq!(fp, BookFingerprint::of_id(&id));
    // FNV-1a over the UTF-8 ID and side, without an mtime.
    assert_eq!(fp.cover_key(160), "5d4bbd182bdd1b5b");
    for slot in [ThumbSlot::Cover, ThumbSlot::Page(3)] {
        store.put(fp, slot, 160, &tiny_decoded_image()).unwrap();
        assert!(store.get(fp, slot, 160).is_none());
    }
    assert_eq!(store.purge_cover(fp, &[160]), 0);
    assert_eq!(store.purge_pages(fp, &[160], 4), 0);
    assert_eq!(store.clear(), ClearCacheReport::default());
    assert_eq!(store.prune(0), PruneReport::default());
}

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
    let target = super::fs_clock_now() - std::time::Duration::from_secs(secs_ago);
    set_mtime(path, target);
}

/// On-disk size of the cached QOI file for `key` (seeded via `put`).
fn qoi_size(dir: &Path, key: &str) -> u64 {
    std::fs::metadata(dir.join(format!("{key}.qoi")))
        .expect("seeded qoi exists")
        .len()
}

#[test]
fn source_mtime_secs_reads_real_file_mtime() {
    let file = tempfile::NamedTempFile::new().expect("create source file");

    assert_ne!(
        source_mtime_secs(file.path()),
        0,
        "a real temp file has a readable post-epoch mtime"
    );
}

#[test]
fn source_mtime_secs_returns_zero_for_missing_path() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.cbz");

    assert_eq!(source_mtime_secs(&missing), 0);
}

/// Synthesize a tiny 2x3 solid-red RGBA DecodedImage in memory (no files).
/// Canonical in-test fixture for FsThumbStore round-trip tests.
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
    let cache = FsThumbStore::with_dir(nested.clone());
    cache
        .put_by_key("abc123", &tiny_decoded_image())
        .expect("put should succeed");
    assert!(nested.exists(), "put must create the target directory");
}

#[test]
fn put_writes_key_dot_qoi_inside_dir() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let key = "cafebabe12345678";
    cache
        .put_by_key(key, &tiny_decoded_image())
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
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    assert!(cache.get_by_key("nonexistent_key").is_none());
}

#[test]
fn get_corrupt_file_returns_none() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let key = "deadbeef01234567";
    let path = dir.path().join(format!("{key}.qoi"));
    std::fs::write(&path, b"this is not a QOI").unwrap();
    assert!(cache.get_by_key(key).is_none());
}

#[test]
fn get_ignores_legacy_png_as_a_clean_miss() {
    // A `.png` from a prior build is never read under the new `.qoi` extension: the
    // lookup misses (file later reclaimed by prune/clear), so it regenerates as `.qoi`.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let key = "1234567890abcdef";
    // Seed a VALID legacy PNG so a miss can only come from the extension,
    // not from a decode failure.
    let legacy = encode_png_for_test(&tiny_decoded_image());
    std::fs::write(dir.path().join(format!("{key}.png")), &legacy).unwrap();

    assert!(
        cache.get_by_key(key).is_none(),
        "a legacy .png must be a clean miss under the .qoi extension"
    );
}

#[test]
fn put_get_roundtrip_preserves_dims_and_bytes() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let original = tiny_decoded_image();
    cache
        .put_by_key("roundtrip_key", &original)
        .expect("put must succeed");

    let retrieved = cache
        .get_by_key("roundtrip_key")
        .expect("get must return Some after put");
    assert_eq!(retrieved.width(), original.width(), "width mismatch");
    assert_eq!(retrieved.height(), original.height(), "height mismatch");
    assert_eq!(retrieved.rgba(), original.rgba(), "RGBA bytes mismatch");
}

#[test]
fn put_twice_same_key_returns_latest_image() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());

    // First write: the 2x3 solid-red fixture.
    cache
        .put_by_key("overwrite_key", &tiny_decoded_image())
        .expect("first put");

    // Second write to the SAME key with different bytes AND dimensions.
    let second_rgba = [0u8, 0, 255, 255].repeat(4 * 2); // solid blue, 4x2
    let second = DecodedImage::new(second_rgba, 4, 2).expect("valid 4x2 RGBA");
    cache
        .put_by_key("overwrite_key", &second)
        .expect("second put");

    let retrieved = cache
        .get_by_key("overwrite_key")
        .expect("get after overwrite");
    assert_eq!(retrieved.width(), 4, "overwrite must update width");
    assert_eq!(retrieved.height(), 2, "overwrite must update height");
    assert_eq!(
        retrieved.rgba(),
        second.rgba(),
        "overwrite must return the latest bytes"
    );
}

#[test]
fn purge_cover_removes_the_cover_cached_under_the_current_mtime() {
    // Seed a cover under the exact key purge_cover derives, then purge: the file
    // is unlinked and counted, and a follow-up get misses.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let generated = BookFingerprint::of_path_and_mtime(&path, source_mtime_secs(&path));
    let max_side = 320;
    let key = generated.cover_key(max_side);
    cache
        .put_by_key(&key, &tiny_decoded_image())
        .expect("seed cover");

    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_cover(fp, &[max_side]);
    assert_eq!(removed, 1, "the matching cover is removed and counted");
    assert!(
        cache.get_by_key(&key).is_none(),
        "the cover is gone after purge"
    );
}

#[test]
fn purge_cover_drifted_mtime_removes_nothing_and_does_not_error() {
    // The cover was generated under one mtime; purging with a DRIFTED mtime derives a
    // different key, so nothing matches: 0 removed, no error, orphan stays (prune later).
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let max_side = 320;
    let generated_mtime = source_mtime_secs(&path);
    let stored_key = BookFingerprint::of_path_and_mtime(&path, generated_mtime).cover_key(max_side);
    cache
        .put_by_key(&stored_key, &tiny_decoded_image())
        .expect("seed cover");
    set_mtime(
        &path,
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(generated_mtime as u64 + 10),
    );

    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_cover(fp, &[max_side]);
    assert_eq!(removed, 0, "a drifted mtime matches no cover");
    assert!(
        cache.get_by_key(&stored_key).is_some(),
        "the orphan stays (best-effort, never an error)"
    );
}

#[test]
fn purge_cover_removes_across_multiple_sides() {
    // A book can have covers at several max_side variants; purge_cover removes
    // every present variant and counts only the ones that existed.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let generated = BookFingerprint::of_path_and_mtime(&path, source_mtime_secs(&path));
    // Seed two of the three requested sides; the third (480) is absent.
    for side in [160, 320] {
        let key = generated.cover_key(side);
        cache
            .put_by_key(&key, &tiny_decoded_image())
            .expect("seed cover");
    }

    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_cover(fp, &[160, 320, 480]);
    assert_eq!(removed, 2, "only the two present sides are removed/counted");
    assert!(cache.get_by_key(&generated.cover_key(160)).is_none());
    assert!(cache.get_by_key(&generated.cover_key(320)).is_none());
}

#[test]
fn purge_cover_empty_sides_removes_nothing() {
    // An empty `max_sides` derives no keys, so there is nothing to purge: 0
    // removed, and the seeded cover stays on disk untouched.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let generated = BookFingerprint::of_path_and_mtime(&path, source_mtime_secs(&path));
    let max_side = 320;
    let key = generated.cover_key(max_side);
    cache
        .put_by_key(&key, &tiny_decoded_image())
        .expect("seed cover");

    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_cover(fp, &[]);
    assert_eq!(removed, 0, "no sides requested removes no cover");
    assert!(
        cache.get_by_key(&key).is_some(),
        "the seeded cover is left intact when no sides are requested"
    );
}

#[test]
fn purge_pages_removes_all_strip_thumbnails_for_a_book() {
    // Purge every strip thumbnail for the book while preserving its cover.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let generated = BookFingerprint::of_path_and_mtime(&path, source_mtime_secs(&path));
    let max_side = 160;
    let page_count = 5;

    // Seed one strip thumbnail per page under the exact key purge_pages derives.
    for i in 0..page_count {
        let key = generated.page_key(max_side, i);
        cache
            .put_by_key(&key, &tiny_decoded_image())
            .expect("seed strip");
    }
    // Seed the cover too (disjoint key — it must NOT be swept by the strip purge).
    let cover_key = generated.cover_key(max_side);
    cache
        .put_by_key(&cover_key, &tiny_decoded_image())
        .expect("seed cover");

    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_pages(fp, &[max_side], page_count);
    assert_eq!(
        removed, page_count,
        "every strip page is removed and counted"
    );
    for i in 0..page_count {
        let key = generated.page_key(max_side, i);
        assert!(
            cache.get_by_key(&key).is_none(),
            "strip page {i} is gone after the strip purge"
        );
    }
    assert!(
        cache.get_by_key(&cover_key).is_some(),
        "the cover survives the strip purge (it is purge_cover's job)"
    );

    // The sibling purge_cover then reclaims the cover.
    let removed_cover = cache.purge_cover(fp, &[max_side]);
    assert_eq!(removed_cover, 1, "the cover is reclaimed by purge_cover");
    assert!(
        cache.get_by_key(&cover_key).is_none(),
        "the cover is gone after purge_cover"
    );
}

#[test]
fn purge_pages_overshooting_page_count_skips_missing_keys() {
    // A page_count larger than the strips on disk is best-effort: extra indices derive
    // keys with no file, so only the present pages are removed/counted — no error.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("cache"));
    let path = dir.path().join("book.cbz");
    std::fs::write(&path, b"source book").expect("create source book");
    let generated = BookFingerprint::of_path_and_mtime(&path, source_mtime_secs(&path));
    let max_side = 160;
    let present = 3;

    for i in 0..present {
        let key = generated.page_key(max_side, i);
        cache
            .put_by_key(&key, &tiny_decoded_image())
            .expect("seed strip");
    }

    // Overshoot: ask for 10 pages though only 3 strips exist on disk.
    let fp = cache.fingerprint(&BookId::from_canonical_path(&path).unwrap());
    let removed = cache.purge_pages(fp, &[max_side], 10);
    assert_eq!(
        removed, present,
        "only the present strips are removed/counted; the missing keys are skipped"
    );
    for i in 0..present {
        let key = generated.page_key(max_side, i);
        assert!(
            cache.get_by_key(&key).is_none(),
            "present strip {i} was removed by the overshooting purge"
        );
    }
}

#[test]
fn clear_missing_dir_returns_zero_report() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().join("never_created"));

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
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed qoi");
    cache
        .put_by_key("bbbb", &tiny_decoded_image())
        .expect("seed qoi");
    // A `write_atomic` temp uses the `.tmpXXXXXX` prefix shape.
    let tmp = dir.path().join(".tmpcccc01");
    std::fs::write(&tmp, b"interrupted cache write").unwrap();
    let expected_bytes =
        qoi_size(dir.path(), "aaaa") + qoi_size(dir.path(), "bbbb") + tmp.metadata().unwrap().len();

    let report = cache.clear();

    assert_eq!(report.removed_files, 3);
    assert_eq!(report.removed_bytes, expected_bytes);
    assert!(cache.get_by_key("aaaa").is_none());
    assert!(cache.get_by_key("bbbb").is_none());
    assert!(!tmp.exists(), "cache temp files are owned entries too");
}

#[test]
fn clear_keeps_foreign_files_and_nested_entries() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed qoi");
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
    let cache = FsThumbStore::with_dir(dir.path().join("never_created"));

    let report = cache.prune(0);

    assert_eq!(report, PruneReport::default(), "missing dir prunes nothing");
}

#[test]
fn prune_under_cap_removes_nothing() {
    // Total payload exactly at the cap: nothing is evicted and the report
    // carries the surviving total so the caller can log it.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed");
    cache
        .put_by_key("bbbb", &tiny_decoded_image())
        .expect("seed");
    let total = qoi_size(dir.path(), "aaaa") + qoi_size(dir.path(), "bbbb");

    let report = cache.prune(total);

    assert_eq!(report.removed_files, 0, "at-cap total evicts nothing");
    assert_eq!(report.removed_bytes, 0);
    assert_eq!(report.retained_bytes, total, "report carries the total");
    assert!(cache.get_by_key("aaaa").is_some(), "both entries survive");
    assert!(cache.get_by_key("bbbb").is_some());
}

#[test]
fn prune_over_cap_removes_oldest_first_until_under_cap() {
    // Three entries, distinct ages. A cap that fits exactly the two newest
    // must evict ONLY the oldest, and the report carries the exact numbers.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    for (key, age) in [("oldest", 3000), ("middle", 2000), ("newest", 1000)] {
        cache.put_by_key(key, &tiny_decoded_image()).expect("seed");
        back_date(&dir.path().join(format!("{key}.qoi")), age);
    }
    let oldest = qoi_size(dir.path(), "oldest");
    let cap = qoi_size(dir.path(), "middle") + qoi_size(dir.path(), "newest");

    let report = cache.prune(cap);

    assert_eq!(report.removed_files, 1, "only the oldest is evicted");
    assert_eq!(report.removed_bytes, oldest);
    assert_eq!(report.retained_bytes, cap, "survivors sum to the cap");
    assert!(cache.get_by_key("oldest").is_none(), "oldest entry is gone");
    assert!(
        cache.get_by_key("middle").is_some(),
        "newer entries survive"
    );
    assert!(cache.get_by_key("newest").is_some());
}

#[test]
fn prune_equal_mtime_tie_breaks_by_file_name() {
    // Two entries with the SAME mtime and a cap that fits only one: the file-name
    // tie-break makes eviction deterministic (ascending name, so "aaaa.qoi" first).
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    // ONE shared instant: per-file `now()` reads would differ by microseconds
    // and turn this into an mtime-order test instead of a tie-break test.
    let shared = super::fs_clock_now() - std::time::Duration::from_secs(2000);
    for key in ["bbbb", "aaaa"] {
        cache.put_by_key(key, &tiny_decoded_image()).expect("seed");
        set_mtime(&dir.path().join(format!("{key}.qoi")), shared);
    }
    let cap = qoi_size(dir.path(), "bbbb");

    let report = cache.prune(cap);

    assert_eq!(report.removed_files, 1);
    assert!(
        cache.get_by_key("aaaa").is_none(),
        "name tie-break evicts aaaa first"
    );
    assert!(cache.get_by_key("bbbb").is_some());
}

#[test]
fn prune_zero_cap_removes_all_qois() {
    // Degenerate cap: every cached QOI goes; the directory itself stays.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed");
    cache
        .put_by_key("bbbb", &tiny_decoded_image())
        .expect("seed");

    let report = cache.prune(0);

    assert_eq!(report.removed_files, 2, "cap 0 evicts every entry");
    assert_eq!(report.retained_bytes, 0);
    assert!(cache.get_by_key("aaaa").is_none());
    assert!(cache.get_by_key("bbbb").is_none());
}

#[test]
fn prune_ignores_files_that_are_not_cache_entries() {
    // Safety pin: the sweep only deletes what the cache owns (`*.qoi`, legacy `*.png`,
    // stale tmp). A stray foreign file survives even a zero cap.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
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
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed");
    let stale_tmp = dir.path().join(".tmpdeadbeef");
    std::fs::write(&stale_tmp, b"torn write leftover").unwrap();
    back_date(&stale_tmp, 2 * 60 * 60); // 2 h old: well past the threshold

    let report = cache.prune(u64::MAX);

    assert_eq!(report.removed_files, 1, "the stale tmp is reclaimed");
    assert_eq!(report.removed_bytes, b"torn write leftover".len() as u64);
    assert!(!stale_tmp.exists(), "the stale tmp is gone");
    assert!(
        cache.get_by_key("aaaa").is_some(),
        "the cached QOI is untouched"
    );
}

#[test]
fn prune_keeps_fresh_tmp() {
    // A tmp file younger than the stale threshold may be an in-flight `put`
    // on another thread — the age guard protects it, even under a zero cap.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let fresh_tmp = dir.path().join(".tmpcafebabe");
    std::fs::write(&fresh_tmp, b"write in flight").unwrap();

    let report = cache.prune(0);

    assert_eq!(report.removed_files, 0, "an in-flight tmp is protected");
    assert!(fresh_tmp.exists(), "the fresh tmp survives the sweep");
}

#[test]
fn prune_reclaims_tmp_with_mtime_far_in_the_future() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let future_tmp = dir.path().join(".tmpfuture");
    std::fs::write(&future_tmp, b"clock-skewed crash leftover").unwrap();
    set_mtime(
        &future_tmp,
        super::fs_clock_now() + std::time::Duration::from_secs(2 * 60 * 60),
    );

    let report = cache.prune(u64::MAX);

    assert_eq!(report.removed_files, 1, "the far-future tmp is reclaimed");
    assert!(!future_tmp.exists(), "the far-future tmp is gone");
}

#[test]
fn prune_keeps_tmp_with_mtime_slightly_in_the_future() {
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let future_tmp = dir.path().join(".tmpnear-future");
    std::fs::write(&future_tmp, b"write with minor clock skew").unwrap();
    set_mtime(
        &future_tmp,
        super::fs_clock_now() + std::time::Duration::from_secs(10),
    );

    let report = cache.prune(0);

    assert_eq!(report.removed_files, 0, "minor clock skew is protected");
    assert!(future_tmp.exists(), "the near-future tmp survives");
}

#[test]
fn prune_reclaims_legacy_png_outside_the_cap() {
    // Legacy `.png` entries are never read under the `.qoi` extension, so prune reclaims
    // every one regardless of the cap; only `.qoi` payload counts against the size cap.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("live", &tiny_decoded_image())
        .expect("seed qoi");
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
    assert!(cache.get_by_key("live").is_some(), "the live qoi survives");
}

#[test]
fn clear_reclaims_legacy_png() {
    // `clear` recognizes legacy `.png` as an owned entry and unlinks it
    // alongside current `.qoi` thumbnails.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("live", &tiny_decoded_image())
        .expect("seed qoi");
    let legacy = dir.path().join("dead.png");
    std::fs::write(&legacy, encode_png_for_test(&tiny_decoded_image())).unwrap();
    let expected_bytes = qoi_size(dir.path(), "live") + legacy.metadata().unwrap().len();

    let report = cache.clear();

    assert_eq!(report.removed_files, 2, "both qoi and legacy png cleared");
    assert_eq!(report.removed_bytes, expected_bytes);
    assert!(!legacy.exists(), "the legacy png is gone");
    assert!(cache.get_by_key("live").is_none(), "the qoi is gone");
}

#[test]
fn get_hit_advances_file_mtime() {
    // The near-LRU contract: a HIT refreshes the file's mtime so `prune`'s ascending
    // eviction approximates LRU — a cover read each launch outlives yesterday's orphan.
    let dir = tempdir().unwrap();
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    cache
        .put_by_key("aaaa", &tiny_decoded_image())
        .expect("seed");
    let qoi = dir.path().join("aaaa.qoi");
    back_date(&qoi, 1000);
    let before = std::fs::metadata(&qoi).unwrap().modified().unwrap();

    assert!(
        cache.get_by_key("aaaa").is_some(),
        "pre-condition: a cache hit"
    );

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
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());
    let qoi = dir.path().join("aaaa.qoi");
    std::fs::write(&qoi, b"this is not a QOI").unwrap();
    back_date(&qoi, 1000);
    let before = std::fs::metadata(&qoi).unwrap().modified().unwrap();

    assert!(
        cache.get_by_key("aaaa").is_none(),
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
    let cache = FsThumbStore::with_dir(dir.path().to_path_buf());

    // 4x4 image whose pixels carry distinct RGBA values (from the pixel index), so a
    // channel-swap or row-stride bug can't round-trip unnoticed (a solid fixture would).
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for i in 0..(4u8 * 4) {
        rgba.extend_from_slice(&[i, i.wrapping_add(64), i.wrapping_add(128), 255]);
    }
    let original = DecodedImage::new(rgba, 4, 4).expect("valid 4x4 RGBA");

    cache
        .put_by_key("multicolor_key", &original)
        .expect("put must succeed");
    let retrieved = cache
        .get_by_key("multicolor_key")
        .expect("get must return Some");
    assert_eq!(retrieved.width(), original.width());
    assert_eq!(retrieved.height(), original.height());
    assert_eq!(
        retrieved.rgba(),
        original.rgba(),
        "multi-color RGBA must round-trip byte-exact"
    );
}
