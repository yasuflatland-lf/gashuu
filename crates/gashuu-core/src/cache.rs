//! Decoded-image cache: an LRU of `Arc<DecodedImage>` in front of a `PageSource`,
//! with background ±N prefetch via rayon so cache-hit page turns stay instant.
//!
//! `get` returns immediately on a hit; on a miss it decodes synchronously, caches
//! the result, and (either way) spawns a background task that warms the pages
//! around `index`. Prefetch decode failures are dropped — the authoritative error
//! surfaces if/when that page is requested directly via `get`. This crate stays
//! `tracing`-free, so prefetch failures are silent here by design.

use crate::error::CoreError;
use crate::image_ops::{decode, DecodedImage};
use crate::page_source::PageSource;
use lru::LruCache;
use rayon::prelude::*;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

/// Default number of decoded images held in the LRU.
pub const DEFAULT_CAPACITY: usize = 50;
/// Default prefetch radius: pages on each side of the current page to warm.
pub const DEFAULT_PREFETCH_RADIUS: usize = 3;

/// Neighbour indices within `radius` of `center`, clamped to `[0, len)` and
/// excluding `center` itself (the current page is fetched by `get`). Ascending.
fn prefetch_indices(center: usize, radius: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let lo = center.saturating_sub(radius);
    let hi = (center + radius).min(len - 1);
    (lo..=hi).filter(|&i| i != center).collect()
}

/// Shared, thread-safe cache state. Held behind an `Arc` so background prefetch
/// tasks can outlive the `ImageCache` handle without dangling.
struct Inner {
    source: Arc<dyn PageSource>,
    len: usize,
    cache: Mutex<LruCache<usize, Arc<DecodedImage>>>,
    in_flight: Mutex<HashSet<usize>>,
}

impl Inner {
    /// Synchronously warm every not-yet-cached, not-in-flight neighbour of
    /// `center`. Reads happen here; decodes run in parallel via rayon. Decode
    /// failures are dropped (the page simply stays uncached). In-flight markers
    /// are always cleared, even for pages that failed to decode.
    fn prefetch_blocking(&self, center: usize, radius: usize) {
        // Reserve the work under the locks: pick neighbours that are neither
        // cached nor already being prefetched, and mark them in-flight.
        let to_fetch: Vec<usize> = {
            let cache = self.cache.lock().unwrap();
            let mut in_flight = self.in_flight.lock().unwrap();
            let candidates: Vec<usize> = prefetch_indices(center, radius, self.len)
                .into_iter()
                .filter(|i| !cache.contains(i) && !in_flight.contains(i))
                .collect();
            for &i in &candidates {
                in_flight.insert(i);
            }
            candidates
        };

        // Read + decode in parallel. Errors are dropped via `ok()`.
        let decoded: Vec<(usize, Arc<DecodedImage>)> = to_fetch
            .par_iter()
            .filter_map(|&i| {
                let img = self.source.read_bytes(i).and_then(|b| decode(&b)).ok()?;
                Some((i, Arc::new(img)))
            })
            .collect();

        {
            let mut cache = self.cache.lock().unwrap();
            for (i, img) in &decoded {
                cache.put(*i, Arc::clone(img));
            }
        }
        let mut in_flight = self.in_flight.lock().unwrap();
        for i in &to_fetch {
            in_flight.remove(i);
        }
    }
}

/// An LRU cache of decoded pages with background ±N prefetch.
pub struct ImageCache {
    inner: Arc<Inner>,
    radius: usize,
}

impl ImageCache {
    /// Build a cache over `source` holding up to `capacity` decoded images and
    /// prefetching `radius` pages on each side of the current page. A `capacity`
    /// of 0 is treated as 1 (the LRU must hold at least the current page).
    pub fn new(source: Arc<dyn PageSource>, capacity: usize, radius: usize) -> Self {
        let len = source.list_pages().len();
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            inner: Arc::new(Inner {
                source,
                len,
                cache: Mutex::new(LruCache::new(cap)),
                in_flight: Mutex::new(HashSet::new()),
            }),
            radius,
        }
    }

    /// Number of pages in the underlying source.
    pub fn len(&self) -> usize {
        self.inner.len
    }

    /// True when the source has no pages.
    pub fn is_empty(&self) -> bool {
        self.inner.len == 0
    }

    /// Return the decoded page at `index`, decoding synchronously on a miss, then
    /// spawn a background task to warm the neighbouring pages. A hit clones an
    /// `Arc` (no buffer copy) and returns instantly.
    pub fn get(&self, index: usize) -> Result<Arc<DecodedImage>, CoreError> {
        if let Some(img) = self.inner.cache.lock().unwrap().get(&index).cloned() {
            self.spawn_prefetch(index);
            return Ok(img);
        }
        let bytes = self.inner.source.read_bytes(index)?;
        let img = Arc::new(decode(&bytes)?);
        self.inner
            .cache
            .lock()
            .unwrap()
            .put(index, Arc::clone(&img));
        self.spawn_prefetch(index);
        Ok(img)
    }

    /// Fire-and-forget background prefetch around `center` on the rayon pool.
    fn spawn_prefetch(&self, center: usize) {
        let inner = Arc::clone(&self.inner);
        let radius = self.radius;
        rayon::spawn(move || inner.prefetch_blocking(center, radius));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_source::PageEntry;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A tiny valid 2x3 PNG so reads decode to a real image.
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([7, 7, 7, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// A `PageSource` that counts `read_bytes` calls (thread-safe) and can be told
    /// to return undecodable bytes for one index so a decode failure during
    /// prefetch can be exercised deterministically.
    struct CountingSource {
        pages: usize,
        reads: Arc<AtomicUsize>,
        fail_index: Option<usize>,
    }

    impl PageSource for CountingSource {
        fn list_pages(&self) -> Vec<PageEntry> {
            vec![
                PageEntry {
                    path: "p".into(),
                    name: "p".into()
                };
                self.pages
            ]
        }
        fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
            if index >= self.pages {
                return Err(CoreError::IndexOutOfRange {
                    index,
                    len: self.pages,
                });
            }
            self.reads.fetch_add(1, Ordering::SeqCst);
            if Some(index) == self.fail_index {
                Ok(vec![1, 2, 3]) // not a valid image: decode() will fail
            } else {
                Ok(tiny_png())
            }
        }
    }

    fn counting(pages: usize) -> (Arc<dyn PageSource>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let src = Arc::new(CountingSource {
            pages,
            reads: Arc::clone(&reads),
            fail_index: None,
        });
        (src, reads)
    }

    // ---- prefetch_indices: pure boundary tests ----

    #[test]
    fn prefetch_range_excludes_center_and_clamps_low() {
        assert_eq!(prefetch_indices(0, 3, 10), vec![1, 2, 3]);
    }

    #[test]
    fn prefetch_range_in_the_middle() {
        assert_eq!(prefetch_indices(5, 3, 10), vec![2, 3, 4, 6, 7, 8]);
    }

    #[test]
    fn prefetch_range_clamps_high_at_last_page() {
        assert_eq!(prefetch_indices(9, 3, 10), vec![6, 7, 8]);
    }

    #[test]
    fn prefetch_range_single_page_is_empty() {
        assert_eq!(prefetch_indices(0, 3, 1), Vec::<usize>::new());
    }

    #[test]
    fn prefetch_range_empty_source_is_empty() {
        assert_eq!(prefetch_indices(0, 3, 0), Vec::<usize>::new());
    }

    #[test]
    fn prefetch_range_zero_radius_is_empty() {
        assert_eq!(prefetch_indices(5, 0, 10), Vec::<usize>::new());
    }

    // ---- cache semantics (radius 0 ⇒ background tasks are inert) ----

    #[test]
    fn hit_returns_same_arc_without_rereading() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, 50, 0);
        let a = cache.get(0).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let b = cache.get(0).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "a hit must return the cached Arc");
        assert_eq!(reads.load(Ordering::SeqCst), 1, "a hit must not re-read");
    }

    #[test]
    fn miss_reads_and_decodes() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, 50, 0);
        let img = cache.get(2).unwrap();
        assert_eq!((img.width(), img.height()), (2, 3));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lru_evicts_oldest_over_capacity() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, 2, 0); // capacity 2
        cache.get(0).unwrap();
        cache.get(1).unwrap();
        cache.get(2).unwrap(); // evicts 0 (least recently used)
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        // 0 was evicted ⇒ a fresh read.
        cache.get(0).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 4);
        // 2 is still cached ⇒ no new read.
        cache.get(2).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn capacity_is_never_exceeded() {
        let (src, _reads) = counting(10);
        let cache = ImageCache::new(src, 3, 0);
        for i in 0..10 {
            cache.get(i).unwrap();
        }
        assert!(cache.inner.cache.lock().unwrap().len() <= 3);
    }

    // ---- prefetch: synchronous core, deterministic ----

    #[test]
    fn prefetch_warms_neighbours_in_range() {
        let (src, reads) = counting(10);
        let cache = ImageCache::new(src, 50, 3);
        cache.inner.prefetch_blocking(0, 3); // warms 1, 2, 3
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        let guard = cache.inner.cache.lock().unwrap();
        assert!(guard.contains(&1));
        assert!(guard.contains(&3));
        assert!(!guard.contains(&4), "page 4 is outside the radius");
    }

    #[test]
    fn prefetch_skips_cached_and_clears_in_flight() {
        let (src, reads) = counting(10);
        let cache = ImageCache::new(src, 50, 3);
        cache.inner.prefetch_blocking(0, 3); // reads 1, 2, 3
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        cache.inner.prefetch_blocking(0, 3); // all cached ⇒ no new reads
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        assert!(cache.inner.in_flight.lock().unwrap().is_empty());
    }

    #[test]
    fn prefetch_decode_failure_is_uncached_and_get_surfaces_error() {
        let reads = Arc::new(AtomicUsize::new(0));
        let src = Arc::new(CountingSource {
            pages: 10,
            reads: Arc::clone(&reads),
            fail_index: Some(2),
        }) as Arc<dyn PageSource>;
        let cache = ImageCache::new(src, 50, 3);
        cache.inner.prefetch_blocking(0, 3); // reads 1, 2, 3; page 2 fails to decode
        assert!(
            !cache.inner.cache.lock().unwrap().contains(&2),
            "a failed decode must not be cached"
        );
        assert!(
            cache.inner.in_flight.lock().unwrap().is_empty(),
            "in-flight must be cleared even on failure"
        );
        // Requesting page 2 directly re-reads and surfaces the real decode error.
        assert!(cache.get(2).is_err());
    }
}
