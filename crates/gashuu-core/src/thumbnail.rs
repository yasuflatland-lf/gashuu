//! Parallel thumbnail generation over a [`PageSource`].
//!
//! This module is headless: no slint, no tracing.

use crate::error::CoreError;
use crate::image_ops::{decode_thumbnail, DecodedImage};
use crate::page_source::PageSource;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Default longer-edge size for generated thumbnails.
pub const DEFAULT_THUMB_MAX_SIDE: u32 = 160;

/// Generate a thumbnail for every page of `source` in parallel, invoking
/// `on_ready(index, result)` as each page completes (in arbitrary order).
///
/// This function **blocks** until all pages finish or `cancelled` flips true.
/// The caller is expected to run it on a background thread so that opening a
/// book returns immediately in the UI.
///
/// `cancelled` is polled before the read AND before the callback so that a
/// superseded generation (e.g. the user opened a different book) stops promptly
/// and never delivers stale results to the previous callback.
///
/// A per-page read/decode failure is delivered as `Err` to `on_ready` — never
/// a panic. The UI is expected to render a placeholder cell for that index.
pub fn generate_thumbnails<F>(
    source: Arc<dyn PageSource>,
    max_side: u32,
    cancelled: Arc<AtomicBool>,
    on_ready: F,
) where
    F: Fn(usize, Result<DecodedImage, CoreError>) + Send + Sync,
{
    let n = source.list_pages().len();
    (0..n).into_par_iter().for_each(|i| {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let res = source
            .read_bytes(i)
            .and_then(|b| decode_thumbnail(&b, max_side));
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        on_ready(i, res);
    });
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    // ---------------------------------------------------------------------------
    // Test fixture helpers
    // ---------------------------------------------------------------------------

    /// Encode a tiny solid-color PNG into bytes using the `image` crate.
    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    // ---------------------------------------------------------------------------
    // A minimal in-process PageSource for tests
    // ---------------------------------------------------------------------------

    /// Holds a fixed list of pre-encoded page byte-vecs. Pages whose bytes are
    /// `None` simulate a read failure (returns `CoreError::IndexOutOfRange`).
    struct CountingSource {
        pages: Vec<Option<Vec<u8>>>,
    }

    impl CountingSource {
        fn new(pages: Vec<Option<Vec<u8>>>) -> Self {
            Self { pages }
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

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    /// Every page index 0..N is delivered to `on_ready` exactly once.
    #[test]
    fn all_pages_delivered_exactly_once() {
        const N: usize = 5;
        let pages: Vec<Option<Vec<u8>>> = (0..N).map(|_| Some(tiny_png(8, 8))).collect();
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(pages));
        let cancelled = Arc::new(AtomicBool::new(false));

        // A slot per page: starts as false, set to true on first delivery.
        let delivered: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(vec![false; N]));
        let delivered_clone = Arc::clone(&delivered);

        generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancelled, move |i, res| {
            assert!(res.is_ok(), "page {i} should decode successfully");
            let mut guard = delivered_clone.lock().unwrap();
            assert!(!guard[i], "page {i} delivered more than once");
            guard[i] = true;
        });

        let guard = delivered.lock().unwrap();
        for (i, &seen) in guard.iter().enumerate() {
            assert!(seen, "page {i} was never delivered");
        }
    }

    /// When `cancelled` is already true before the call, `on_ready` is never invoked.
    #[test]
    fn cancelled_flag_suppresses_all_callbacks() {
        const N: usize = 4;
        let pages: Vec<Option<Vec<u8>>> = (0..N).map(|_| Some(tiny_png(4, 4))).collect();
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(pages));
        let cancelled = Arc::new(AtomicBool::new(true)); // pre-cancelled

        let call_count = Arc::new(Mutex::new(0usize));
        let call_count_clone = Arc::clone(&call_count);

        generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancelled, move |_, _| {
            *call_count_clone.lock().unwrap() += 1;
        });

        assert_eq!(
            *call_count.lock().unwrap(),
            0,
            "on_ready should not be called when cancelled"
        );
    }

    /// A page whose bytes are invalid produces `Err` for that index; all valid
    /// pages still produce `Ok`. No panic occurs.
    #[test]
    fn invalid_page_bytes_yield_err_others_yield_ok() {
        const N: usize = 3;
        const BAD: usize = 1; // index 1 has corrupt bytes
        let pages: Vec<Option<Vec<u8>>> = (0..N)
            .map(|i| {
                if i == BAD {
                    Some(b"not-a-valid-image".to_vec())
                } else {
                    Some(tiny_png(6, 6))
                }
            })
            .collect();
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(pages));
        let cancelled = Arc::new(AtomicBool::new(false));

        // results[i] = Some(true) → Ok, Some(false) → Err, None → not delivered.
        let results: Arc<Mutex<Vec<Option<bool>>>> = Arc::new(Mutex::new(vec![None; N]));
        let results_clone = Arc::clone(&results);

        generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancelled, move |i, res| {
            let mut guard = results_clone.lock().unwrap();
            guard[i] = Some(res.is_ok());
        });

        let guard = results.lock().unwrap();
        for (i, &slot) in guard.iter().enumerate() {
            let got_ok = slot.expect("page {i} was not delivered");
            if i == BAD {
                assert!(!got_ok, "page {BAD} (invalid bytes) should produce Err");
            } else {
                assert!(got_ok, "page {i} (valid bytes) should produce Ok");
            }
        }
    }

    /// A 0-page source: `on_ready` is never called and the function returns
    /// without panic.
    #[test]
    fn zero_page_source_is_noop() {
        let source: Arc<dyn PageSource> = Arc::new(CountingSource::new(vec![]));
        let cancelled = Arc::new(AtomicBool::new(false));
        let called = Arc::new(Mutex::new(false));
        let called_clone = Arc::clone(&called);

        generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancelled, move |_, _| {
            *called_clone.lock().unwrap() = true;
        });

        assert!(
            !*called.lock().unwrap(),
            "on_ready must not be called for a 0-page source"
        );
    }

    // ---------------------------------------------------------------------------
    // Post-decode cancel-check guard
    // ---------------------------------------------------------------------------

    /// A single-page source whose `read_bytes` flips `cancelled` as a side effect.
    /// This lets us exercise the SECOND cancel check (line 44, between decode and
    /// `on_ready`) deterministically without any rayon races: the flag is still
    /// `false` at the FIRST check (so we enter read+decode), then `read_bytes`
    /// sets it to `true`, and by the time `generate_thumbnails` reaches the
    /// post-decode check the flag is already set — suppressing the callback.
    ///
    /// Contrast with flipping the flag *inside* `on_ready`: with a multi-page
    /// source other rayon workers may have already passed the second check, making
    /// the suppression count non-deterministic. The read-side-effect approach
    /// keeps the test single-page and fully deterministic.
    struct CancelOnReadSource {
        cancelled: Arc<AtomicBool>,
        bytes: Vec<u8>,
    }

    impl PageSource for CancelOnReadSource {
        fn list_pages(&self) -> Vec<PageEntry> {
            vec![PageEntry {
                name: "page0.png".to_string(),
            }]
        }

        fn read_bytes(&self, _index: usize) -> Result<Vec<u8>, CoreError> {
            // Flip the cancel flag so the post-decode check sees `true`.
            self.cancelled.store(true, Ordering::Relaxed);
            Ok(self.bytes.clone())
        }
        // skipped_count() default 0 is sufficient.
    }

    /// Guards the post-decode (second) cancel check in `generate_thumbnails`.
    ///
    /// Page 0 passes the first `cancelled` check (flag is still `false`),
    /// `read_bytes` flips the flag to `true` as a side effect, decode completes,
    /// then the second check suppresses `on_ready`. The callback count must be 0.
    ///
    /// If the second check (line 44) were deleted, decode would still succeed and
    /// `on_ready` would be called once — making this test fail and exposing the gap.
    #[test]
    fn post_decode_cancel_check_suppresses_callback() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let source: Arc<dyn PageSource> = Arc::new(CancelOnReadSource {
            cancelled: Arc::clone(&cancelled),
            bytes: tiny_png(4, 4),
        });

        let call_count = Arc::new(Mutex::new(0usize));
        let call_count_clone = Arc::clone(&call_count);

        generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancelled, move |_, _| {
            *call_count_clone.lock().unwrap() += 1;
        });

        assert_eq!(
            *call_count.lock().unwrap(),
            0,
            "on_ready must not be called when cancelled is set between decode and callback"
        );
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
}
