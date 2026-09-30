//! Thumbnail generation over a [`PageSource`]: one strip page per call
//! ([`generate_one_thumbnail`]) and the book cover ([`generate_cover`]).
//!
//! This module is headless: no slint, no tracing.

use crate::error::CoreError;
use crate::image_ops::{decode_thumbnail, DecodedImage};
use crate::page_source::PageSource;
use crate::thumbnail_cache::{page_cache_key, source_mtime_secs, ThumbnailCache};
use std::path::Path;
use std::sync::Arc;

/// Default longer-edge size for generated thumbnails.
pub const DEFAULT_THUMB_MAX_SIDE: u32 = 160;

/// The borrows [`generate_one_thumbnail`] needs to persist each page's strip
/// thumbnail to the on-disk cache. `path` supplies the cache-key inputs the
/// [`PageSource`] trait does not expose; `cache` is the shared on-disk store (the
/// same directory as covers, with disjoint keys via [`page_cache_key`]). Both
/// `&ThumbnailCache` and `&Path` are `Send`/`Sync`, so one context is shared
/// across the UI strip worker's rayon `par_iter` without per-page cloning.
#[derive(Clone, Copy)]
pub struct PageThumbContext<'a> {
    /// On-disk cache the page thumbnails are read from and written to.
    pub cache: &'a ThumbnailCache,
    /// Canonical book path that anchors each page's cache key.
    pub path: &'a Path,
}

/// Produce page `i`'s thumbnail, consulting the on-disk cache when `cache` is set.
///
/// With no cache context this is byte-for-byte the original behavior: read the
/// page bytes and decode them. With a context, a cache hit skips the full-page
/// read+decode entirely; a miss reads, decodes, then persists best-effort (core
/// stays log-free, so the `put` `Result` is intentionally ignored).
fn page_thumbnail(
    source: &Arc<dyn PageSource>,
    max_side: u32,
    cache: Option<(PageThumbContext<'_>, i64)>,
    i: usize,
) -> Result<DecodedImage, CoreError> {
    let Some((ctx, mtime)) = cache else {
        let bytes = source.read_bytes(i)?;
        return decode_thumbnail(&bytes, max_side);
    };
    let key = page_cache_key(ctx.path, mtime, max_side, i);
    if let Some(img) = ctx.cache.get(&key) {
        return Ok(img);
    }
    let bytes = source.read_bytes(i)?;
    let img = decode_thumbnail(&bytes, max_side)?;
    let _ = ctx.cache.put(&key, &img);
    Ok(img)
}

/// Produce a single page's thumbnail, consulting the on-disk cache when
/// `cache_ctx` is set.
///
/// The UI's strip controller drives it one visible page at a time, so a freshly
/// opened book decodes only the pages near the viewport instead of all `N`. The
/// cache key is `page_cache_key(path, mtime, max_side, page_index)`, so each page
/// of an unchanged book is persisted once and served from disk afterwards.
///
/// With a cache context a hit skips the full-page read+decode; a miss reads,
/// decodes, then persists best-effort (the `put` `Result` is intentionally ignored
/// — core stays log-free). `cache_ctx == None` always reads + decodes, no caching.
///
/// Cancellation is the caller's concern: the controller checks its cancel flag
/// immediately before and after this call, so a superseded generation neither
/// starts disk work it could avoid nor delivers a stale result.
pub fn generate_one_thumbnail(
    source: &Arc<dyn PageSource>,
    max_side: u32,
    page_index: usize,
    cache_ctx: Option<PageThumbContext<'_>>,
) -> Result<DecodedImage, CoreError> {
    // Stat the book once per call and feed its mtime into the per-page cache key.
    let cache = cache_ctx.map(|ctx| (ctx, source_mtime_secs(ctx.path)));
    page_thumbnail(source, max_side, cache, page_index)
}

/// Generate the cover thumbnail for `source`: a thumbnail of page index 0 whose
/// longer edge is at most `max_side` px.
///
/// Returns `Err(CoreError::IndexOutOfRange { index: 0, len: 0 })` when the source
/// has no pages — a sourceless book has no cover. This reuses the same error the
/// `PageSource` contract already produces for an out-of-range read, so callers
/// match one variant for "no page 0".
///
/// Synchronous and headless: the caller (the UI cover controller) runs this on a
/// background rayon job and streams the result to the carousel.
pub fn generate_cover(
    source: Arc<dyn PageSource>,
    max_side: u32,
) -> Result<DecodedImage, CoreError> {
    let n = source.list_pages().len();
    if n == 0 {
        return Err(CoreError::IndexOutOfRange { index: 0, len: 0 });
    }
    let bytes = source.read_bytes(0)?;
    decode_thumbnail(&bytes, max_side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_source::PageEntry;
    use std::io::Cursor;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    /// Encode a tiny solid-color PNG into bytes using the `image` crate.
    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    /// Holds a fixed list of pre-encoded page byte-vecs. Pages whose bytes are
    /// `None` simulate a read failure (returns `CoreError::IndexOutOfRange`).
    struct CountingSource {
        pages: Vec<Option<Vec<u8>>>,
        /// Number of `read_bytes` calls so far — lets a cache test prove the
        /// second open serves every page from disk without touching the source.
        reads: std::sync::atomic::AtomicUsize,
    }

    impl CountingSource {
        fn new(pages: Vec<Option<Vec<u8>>>) -> Self {
            Self {
                pages,
                reads: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        /// Total `read_bytes` calls observed so far.
        fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
        }
    }

    impl PageSource for CountingSource {
        fn list_pages(&self) -> Vec<PageEntry> {
            self.pages
                .iter()
                .enumerate()
                .map(|(i, _)| PageEntry {
                    name: format!("page{i}.png"),
                })
                .collect()
        }

        fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            // `Some(None)` (simulated read failure) and `None` (out-of-range) both
            // surface as IndexOutOfRange.
            match self.pages.get(index) {
                Some(Some(bytes)) => Ok(bytes.clone()),
                Some(None) | None => Err(CoreError::IndexOutOfRange {
                    index,
                    len: self.pages.len(),
                }),
            }
        }
        // skipped_count() default 0 is sufficient.
    }

    /// `generate_cover` returns a thumbnail of PAGE 0, downscaled within `max_side`,
    /// ignoring any later pages. Page 0 is 200x100, page 1 is 8x8; the cover must
    /// reflect the 200x100 page 0 (longer edge clamped to max_side=64), proving it
    /// reads index 0 and not some other page.
    #[test]
    fn generate_cover_returns_page0_thumbnail_within_max_side() {
        let page0 = {
            let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 20, 30, 255]));
            let mut buf = Vec::new();
            img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };
        let pages = vec![Some(page0), Some(tiny_png(8, 8))];
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(pages));

        let cover = generate_cover(source, 64).expect("page 0 cover should generate");
        assert!(cover.width() <= 64, "cover width {} > 64", cover.width());
        assert!(cover.height() <= 64, "cover height {} > 64", cover.height());
        // 200x100 → longer edge clamped to 64 → width should be 64, height ~32.
        assert_eq!(cover.width(), 64, "page-0 longer edge should clamp to 64");
    }

    /// An empty source (0 pages) has no cover: `generate_cover` returns
    /// `Err(CoreError::IndexOutOfRange { index: 0, len: 0 })` rather than reading
    /// page 0 (which would itself error) — the empty check short-circuits first.
    #[test]
    fn generate_cover_empty_source_errors() {
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(vec![]));
        let Err(err) = generate_cover(source, 64) else {
            panic!("expected Err for a 0-page source");
        };
        assert!(
            matches!(err, CoreError::IndexOutOfRange { index: 0, len: 0 }),
            "expected IndexOutOfRange {{ index: 0, len: 0 }}, got {err:?}"
        );
    }

    /// `generate_cover` propagates a decode error from page 0 rather than
    /// swallowing it: a single page whose bytes are not a valid image must yield
    /// the decode `Err`, proving the `?` on the page-0 read+decode is load-bearing
    /// (a future refactor that dropped it would make this test fail).
    #[test]
    fn generate_cover_propagates_decode_error_on_corrupt_page0() {
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(vec![Some(
            b"not-a-valid-image".to_vec(),
        )]));
        let Err(err) = generate_cover(source, 64) else {
            panic!("expected Err for undecodable page-0 bytes");
        };
        // Match the variant the codebase produces for undecodable bytes — align
        // with the existing invalid-bytes test in this module.
        assert!(
            matches!(err, CoreError::Decode(_)),
            "expected a decode error, got {err:?}"
        );
    }

    use crate::thumbnail_cache::ThumbnailCache;

    /// Count the `*.qoi` cache entries written directly in `dir` (non-recursive).
    fn qoi_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "qoi"))
            .count()
    }

    /// Fetch every page of `pages` through `generate_one_thumbnail` with the given
    /// cache context (one call per page, as the strip worker does) and return the
    /// source so the caller can assert the `read_bytes` count.
    fn run_strip(
        pages: Vec<Option<Vec<u8>>>,
        cache: &ThumbnailCache,
        path: &std::path::Path,
    ) -> Arc<CountingSource> {
        let src = Arc::new(CountingSource::new(pages));
        let source: Arc<dyn PageSource> = src.clone();
        for i in 0..source.list_pages().len() {
            let res = generate_one_thumbnail(
                &source,
                DEFAULT_THUMB_MAX_SIDE,
                i,
                Some(PageThumbContext { cache, path }),
            );
            assert!(res.is_ok(), "page {i} should decode successfully");
        }
        src
    }

    /// First open persists one QOI per page; the second open of the same unchanged
    /// book serves every page from disk with ZERO `read_bytes` calls.
    #[test]
    fn cache_ctx_persists_then_serves_from_cache() {
        const N: usize = 3;
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        // A path that does not exist resolves to mtime 0 — stable across both runs.
        let path = std::path::Path::new("/manga/book.cbz");
        let pages: Vec<Option<Vec<u8>>> = (0..N).map(|_| Some(tiny_png(8, 8))).collect();

        let first = run_strip(pages.clone(), &cache, path);
        assert_eq!(first.reads(), N, "first open reads every page");
        assert_eq!(
            qoi_count(dir.path()),
            N,
            "first open persists one QOI per page"
        );

        let second = run_strip(pages, &cache, path);
        assert_eq!(
            second.reads(),
            0,
            "second open serves every page from the cache"
        );
    }

    /// A modified book (mtime drift) regenerates: the recomputed key misses the
    /// stale on-disk entry, so the page is read and decoded again.
    #[test]
    fn cache_ctx_regenerates_after_mtime_drift() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        // A real file so its mtime is readable; drift it between the two runs.
        let book = tempfile::NamedTempFile::new().unwrap();
        let path = book.path();
        let pages = vec![Some(tiny_png(8, 8))];

        set_file_mtime(
            path,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000),
        );
        let first = run_strip(pages.clone(), &cache, path);
        assert_eq!(first.reads(), 1, "first open reads the page");

        set_file_mtime(
            path,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000),
        );
        let second = run_strip(pages, &cache, path);
        assert_eq!(
            second.reads(),
            1,
            "a drifted mtime changes the key, forcing a re-read"
        );
    }

    /// `generate_one_thumbnail` with no cache reads + decodes the requested page
    /// each time, leaving other pages untouched (the lazy O(visible) contract).
    #[test]
    fn one_thumbnail_no_cache_reads_only_requested_page() {
        let pages: Vec<Option<Vec<u8>>> = (0..5).map(|_| Some(tiny_png(8, 8))).collect();
        let src = Arc::new(CountingSource::new(pages));
        let source: Arc<dyn PageSource> = src.clone();

        let img = generate_one_thumbnail(&source, DEFAULT_THUMB_MAX_SIDE, 2, None)
            .expect("page 2 decodes");
        assert!(img.width() > 0 && img.height() > 0);
        assert_eq!(src.reads(), 1, "exactly one page read for one request");
    }

    /// First call for a page is a cache miss (one read + one persisted cache
    /// entry); a second call for the SAME page is served from disk with zero reads.
    #[test]
    fn one_thumbnail_cache_miss_then_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let path = std::path::Path::new("/manga/book.cbz");
        let src = Arc::new(CountingSource::new(vec![
            Some(tiny_png(8, 8)),
            Some(tiny_png(8, 8)),
        ]));
        let source: Arc<dyn PageSource> = src.clone();

        generate_one_thumbnail(
            &source,
            DEFAULT_THUMB_MAX_SIDE,
            1,
            Some(PageThumbContext {
                cache: &cache,
                path,
            }),
        )
        .expect("miss decodes");
        assert_eq!(src.reads(), 1, "miss reads the page once");
        // Count cache entries codec-agnostically (on-disk codec may be PNG or QOI
        // depending on which sibling changes landed); the page is persisted regardless.
        let persisted = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "png" || ext == "qoi")
            })
            .count();
        assert_eq!(persisted, 1, "miss persists one cache entry");

        generate_one_thumbnail(
            &source,
            DEFAULT_THUMB_MAX_SIDE,
            1,
            Some(PageThumbContext {
                cache: &cache,
                path,
            }),
        )
        .expect("hit serves from cache");
        assert_eq!(src.reads(), 1, "hit performs no further read");
    }

    /// An undecodable page surfaces its error rather than panicking, so the
    /// controller can mark just that cell failed.
    #[test]
    fn one_thumbnail_propagates_decode_error() {
        let src = Arc::new(CountingSource::new(vec![Some(b"not-an-image".to_vec())]));
        let source: Arc<dyn PageSource> = src.clone();
        let err = generate_one_thumbnail(&source, DEFAULT_THUMB_MAX_SIDE, 0, None)
            .expect_err("corrupt bytes should error");
        assert!(
            matches!(err, CoreError::Decode(_)),
            "expected a decode error, got {err:?}"
        );
    }

    /// Set `path`'s mtime to `target` so a test controls the cache key the strip
    /// derives across runs.
    fn set_file_mtime(path: &std::path::Path, target: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(target))
            .expect("set fixture mtime");
    }
}
