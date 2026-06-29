//! Decoded-image cache: an LRU of `Arc<DecodedImage>` in front of a `PageSource`,
//! with background ±N prefetch via rayon so cache-hit page turns stay instant.
//!
//! `get` returns immediately on a hit; on a miss it decodes synchronously, caches
//! the result, and (either way) spawns a background task that warms the pages
//! around `index`. Prefetch decode failures are dropped — the authoritative error
//! surfaces if/when that page is requested directly via `get`. This crate stays
//! `tracing`-free, so prefetch failures are silent here by design.

use crate::cache_config::CacheConfig;
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

/// An LRU of decoded pages paired with a running total of their decoded bytes,
/// kept together so ordering and byte accounting mutate atomically under one
/// mutex. Eviction is two-dimensional: the `LruCache` capacity enforces the
/// count ceiling, and `max_bytes` caps the total decoded-byte footprint. A
/// single oversized page is retained as the floor (at least one entry always
/// stays cached so the current page survives).
struct PageStore {
    lru: LruCache<usize, Arc<DecodedImage>>,
    total_bytes: usize,
    max_bytes: u64,
}

impl PageStore {
    fn new(capacity: NonZeroUsize, max_bytes: u64) -> Self {
        Self {
            lru: LruCache::new(capacity),
            total_bytes: 0,
            max_bytes,
        }
    }

    /// True when page `index` is cached (does not promote it).
    fn contains(&self, index: &usize) -> bool {
        self.lru.contains(index)
    }

    /// Return the cached page at `index`, promoting it in the LRU.
    fn get(&mut self, index: &usize) -> Option<&Arc<DecodedImage>> {
        self.lru.get(index)
    }

    /// Insert `img` at `index`, updating the byte total, then evict the LRU end
    /// until both the count ceiling and the byte budget hold (always keeping the
    /// just-inserted page plus at least one entry overall).
    fn put(&mut self, index: usize, img: Arc<DecodedImage>) {
        let new_bytes = img.rgba().len();
        // Pre-empt the `LruCache`'s own count-ceiling eviction so the displaced
        // entry's bytes are subtracted here instead of vanishing silently.
        if !self.lru.contains(&index) && self.lru.len() == self.lru.cap().get() {
            if let Some((_, evicted)) = self.lru.pop_lru() {
                self.total_bytes -= evicted.rgba().len();
            }
        }
        match self.lru.put(index, img) {
            Some(old) => {
                self.total_bytes = self.total_bytes - old.rgba().len() + new_bytes;
            }
            None => self.total_bytes += new_bytes,
        }
        self.evict_to_budget();
    }

    /// Drop least-recently-used pages while the byte total exceeds `max_bytes`,
    /// retaining at least one entry (the floor for a single oversized page).
    fn evict_to_budget(&mut self) {
        while self.total_bytes as u64 > self.max_bytes && self.lru.len() > 1 {
            match self.lru.pop_lru() {
                Some((_, evicted)) => self.total_bytes -= evicted.rgba().len(),
                None => break,
            }
        }
    }
}

/// Neighbour indices within `radius` of `center`, clamped to `[0, len)` and
/// excluding `center` itself (the current page is fetched by `get`). Ascending.
fn prefetch_indices(center: usize, radius: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let lo = center.saturating_sub(radius);
    let hi = center.saturating_add(radius).min(len - 1);
    (lo..=hi).filter(|&i| i != center).collect()
}

/// Shared, thread-safe cache state. Held behind an `Arc` so background prefetch
/// tasks can outlive the `ImageCache` handle without dangling.
///
/// Locking discipline: when both mutexes are held at once, always acquire
/// `cache` before `in_flight` (`get` only ever takes `cache`); keep this order in
/// any future code to avoid deadlock. No fallible or user-supplied code runs
/// while a lock is held — reads and decodes happen lock-free — so the mutexes
/// cannot be poisoned in practice, and the `lock().unwrap()` calls are an
/// intentional fail-fast.
struct Inner {
    source: Arc<dyn PageSource>,
    len: usize,
    cache: Mutex<PageStore>,
    in_flight: Mutex<HashSet<usize>>,
}

/// RAII guard that clears the reserved in-flight indices when dropped, so a panic
/// in the decode/insert section can never permanently leak in-flight markers
/// (a leaked marker would silently disable prefetch for that page for the cache's
/// lifetime). The `Drop` recovers a poisoned lock via `into_inner` so it can never
/// double-panic while unwinding.
struct InFlightGuard<'a> {
    in_flight: &'a Mutex<HashSet<usize>>,
    keys: Vec<usize>,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        for k in &self.keys {
            in_flight.remove(k);
        }
    }
}

impl Inner {
    /// Return a cached page if present, promoting it in the LRU.
    fn cached(&self, index: usize) -> Option<Arc<DecodedImage>> {
        self.cache.lock().unwrap().get(&index).cloned()
    }

    /// Fire-and-forget background prefetch around `center` on the rayon pool.
    fn spawn_prefetch(self: &Arc<Self>, center: usize, radius: usize) {
        if radius == 0 {
            return;
        }
        let inner = Arc::clone(self);
        rayon::spawn(move || inner.prefetch_blocking(center, radius));
    }

    /// Insert `img` unless another thread already populated the cache.
    fn cache_decoded(&self, index: usize, img: Arc<DecodedImage>) -> Arc<DecodedImage> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(existing) = cache.get(&index).cloned() {
            existing
        } else {
            cache.put(index, Arc::clone(&img));
            img
        }
    }

    /// Warm every not-yet-cached, not-in-flight neighbour of `center`. The
    /// reservation phase (selecting candidates and marking them in-flight) runs
    /// synchronously under the locks; the reads and decodes then run in parallel
    /// via rayon. Decode failures are dropped (the page simply stays uncached).
    /// In-flight markers are always cleared on return — even for pages that failed
    /// to decode, and even if the decode/insert section panics (see `InFlightGuard`).
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
        // Clear the reserved markers on every exit path, including a panic in the
        // parallel decode section below.
        let _guard = InFlightGuard {
            in_flight: &self.in_flight,
            keys: to_fetch.clone(),
        };

        // Read + decode in parallel. Errors are dropped via `ok()`.
        let decoded: Vec<(usize, Arc<DecodedImage>)> = to_fetch
            .par_iter()
            .filter_map(|&i| {
                let img = self.source.read_bytes(i).and_then(|b| decode(&b)).ok()?;
                Some((i, Arc::new(img)))
            })
            .collect();

        let mut cache = self.cache.lock().unwrap();
        for (i, img) in &decoded {
            cache.put(*i, Arc::clone(img));
        }
        // `_guard` drops at end of scope, clearing the in-flight markers.
    }
}

#[derive(Clone)]
pub struct CacheDispatch {
    inner: Arc<Inner>,
    radius: usize,
}

/// An LRU cache of decoded pages with background ±N prefetch.
pub struct ImageCache {
    inner: Arc<Inner>,
    radius: usize,
}

impl ImageCache {
    /// Build a cache over `source` using `config`: hold up to `config.capacity()`
    /// decoded images (always >= 1, guaranteed by `CacheConfig`) and prefetch
    /// `config.radius()` pages on each side of the current page.
    pub fn new(source: Arc<dyn PageSource>, config: CacheConfig) -> Self {
        let len = source.list_pages().len();
        let cap = NonZeroUsize::new(config.capacity()).unwrap();
        Self {
            inner: Arc::new(Inner {
                source,
                len,
                cache: Mutex::new(PageStore::new(cap, config.max_bytes())),
                in_flight: Mutex::new(HashSet::new()),
            }),
            radius: config.radius(),
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

    /// Return the decoded page at `index` if it is already cached.
    ///
    /// This is a pure cache-hit probe: it never reads or decodes on a miss.
    /// Hits are promoted in the LRU and still spawn neighbour prefetch just like
    /// `get`.
    pub fn get_cached(&self, index: usize) -> Option<Arc<DecodedImage>> {
        let img = self.inner.cached(index);
        if img.is_some() {
            self.inner.spawn_prefetch(index, self.radius);
        }
        img
    }

    /// Shareable decode handle for off-thread page work.
    pub fn dispatch_handle(&self) -> CacheDispatch {
        CacheDispatch {
            inner: Arc::clone(&self.inner),
            radius: self.radius,
        }
    }

    /// Return the decoded page at `index`, decoding synchronously on a miss, then
    /// spawn a background task to warm the neighbouring pages. A hit clones an
    /// `Arc` (no buffer copy) and returns instantly.
    pub fn get(&self, index: usize) -> Result<Arc<DecodedImage>, CoreError> {
        if let Some(img) = self.get_cached(index) {
            return Ok(img);
        }
        self.dispatch_handle().decode_and_cache(index)
    }
}

impl CacheDispatch {
    /// Read + decode page `index` on the calling thread, insert it into the
    /// shared cache, and warm neighbouring pages.
    pub fn decode_and_cache(&self, index: usize) -> Result<Arc<DecodedImage>, CoreError> {
        if let Some(img) = self.inner.cached(index) {
            self.inner.spawn_prefetch(index, self.radius);
            return Ok(img);
        }

        let bytes = self.inner.source.read_bytes(index)?;

        if let Some(img) = self.inner.cached(index) {
            self.inner.spawn_prefetch(index, self.radius);
            return Ok(img);
        }

        let decoded = Arc::new(decode(&bytes)?);
        let img = self.inner.cache_decoded(index, decoded);
        self.inner.spawn_prefetch(index, self.radius);
        Ok(img)
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
            vec![PageEntry { name: "p".into() }; self.pages]
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
        let cache = ImageCache::new(src, CacheConfig::new(50, 0));
        let a = cache.get(0).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let b = cache.get(0).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "a hit must return the cached Arc");
        assert_eq!(reads.load(Ordering::SeqCst), 1, "a hit must not re-read");
    }

    #[test]
    fn get_cached_miss_does_not_read() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(50, 0));
        assert!(cache.get_cached(2).is_none());
        assert_eq!(
            reads.load(Ordering::SeqCst),
            0,
            "a cached miss must not trigger read_bytes"
        );
    }

    #[test]
    fn get_cached_hit_returns_same_arc_after_fill() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(50, 0));
        let a = cache.get(1).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let b = cache.get_cached(1).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "cached hit must return the same Arc");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispatch_decode_and_cache_stores_then_get_cached_hits() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(50, 0));
        let dispatch = cache.dispatch_handle();
        let a = dispatch.decode_and_cache(3).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let b = cache.get_cached(3).unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "dispatch must populate the shared cache"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn miss_reads_and_decodes() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(50, 0));
        let img = cache.get(2).unwrap();
        assert_eq!((img.width(), img.height()), (2, 3));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lru_evicts_oldest_over_capacity() {
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(2, 0)); // capacity 2
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
    fn lru_hit_promotes_entry_and_evicts_true_lru() {
        // Distinguishes LRU from FIFO: a hit on 0 must promote it so a later miss
        // evicts 1 (the true LRU), not 0.
        let (src, reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(2, 0)); // capacity 2, no prefetch
        cache.get(0).unwrap(); // [0]
        cache.get(1).unwrap(); // [0(lru), 1(mru)]
        cache.get(0).unwrap(); // hit: promotes 0 -> [1(lru), 0(mru)]
        assert_eq!(reads.load(Ordering::SeqCst), 2, "a hit must not re-read");
        cache.get(2).unwrap(); // miss: evicts 1 (true LRU), keeps 0
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        cache.get(0).unwrap(); // still cached as MRU -> no read
        assert_eq!(
            reads.load(Ordering::SeqCst),
            3,
            "promoted entry must survive"
        );
        cache.get(1).unwrap(); // evicted -> fresh read
        assert_eq!(
            reads.load(Ordering::SeqCst),
            4,
            "true LRU must have been evicted"
        );
    }

    #[test]
    fn capacity_zero_treated_as_one_does_not_panic() {
        // CacheConfig::new coerces capacity 0 to 1 (the LRU must hold at least
        // the current page); constructing and reading must not panic.
        let (src, _reads) = counting(5);
        let cache = ImageCache::new(src, CacheConfig::new(0, 0));
        let img = cache.get(0).unwrap();
        assert_eq!((img.width(), img.height()), (2, 3));
        assert!(cache.inner.cache.lock().unwrap().lru.len() <= 1);
    }

    #[test]
    fn prefetch_skips_pages_already_in_flight() {
        // Exercises the `!in_flight.contains(i)` filter branch in isolation:
        // pages already marked in-flight by another batch are not re-read.
        let (src, reads) = counting(10);
        let cache = ImageCache::new(src, CacheConfig::new(50, 3));
        {
            let mut in_flight = cache.inner.in_flight.lock().unwrap();
            in_flight.insert(1);
            in_flight.insert(2);
        }
        cache.inner.prefetch_blocking(0, 3); // candidates 1,2,3 -> only 3 is fetched
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "only the not-in-flight page is read"
        );
        let in_flight = cache.inner.in_flight.lock().unwrap();
        assert!(
            in_flight.contains(&1) && in_flight.contains(&2),
            "pre-existing in-flight markers from another batch are left untouched"
        );
        assert!(
            !in_flight.contains(&3),
            "this batch's own marker is cleared on return"
        );
    }

    #[test]
    fn capacity_is_never_exceeded() {
        let (src, _reads) = counting(10);
        let cache = ImageCache::new(src, CacheConfig::new(3, 0));
        for i in 0..10 {
            cache.get(i).unwrap();
        }
        assert!(cache.inner.cache.lock().unwrap().lru.len() <= 3);
    }

    // ---- prefetch: synchronous core, deterministic ----

    #[test]
    fn prefetch_warms_neighbours_in_range() {
        let (src, reads) = counting(10);
        let cache = ImageCache::new(src, CacheConfig::new(50, 3));
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
        let cache = ImageCache::new(src, CacheConfig::new(50, 3));
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
        let cache = ImageCache::new(src, CacheConfig::new(50, 3));
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

    // ---- PageStore: byte-budget eviction (unit, exact byte sizes) ----

    /// A decoded image whose RGBA buffer is exactly `bytes` long (`bytes` must be
    /// a multiple of 4). Laid out as a 1-row image so the size is precise.
    fn sized_image(bytes: usize) -> Arc<DecodedImage> {
        assert_eq!(bytes % 4, 0, "RGBA buffer length must be a multiple of 4");
        let width = (bytes / 4) as u32;
        Arc::new(DecodedImage::new(vec![0u8; bytes], width, 1).unwrap())
    }

    fn store(capacity: usize, max_bytes: u64) -> PageStore {
        PageStore::new(NonZeroUsize::new(capacity).unwrap(), max_bytes)
    }

    #[test]
    fn byte_budget_evicts_lru_and_caps_total() {
        // Budget 250, pages of 100 bytes; count ceiling kept high so only the
        // byte budget can evict.
        let mut s = store(50, 250);
        s.put(0, sized_image(100)); // total 100
        s.put(1, sized_image(100)); // total 200
        s.put(2, sized_image(100)); // total 300 > 250 -> evict LRU (page 0) -> 200
        assert!(s.total_bytes as u64 <= 250, "total must stay within budget");
        assert!(!s.contains(&0), "the oldest page must be evicted");
        assert!(s.contains(&2), "the most-recent page must be retained");
        assert!(s.contains(&1));
    }

    #[test]
    fn byte_budget_keeps_single_oversized_page_as_floor() {
        // A lone page larger than the budget is still cached (the floor).
        let mut s = store(50, 100);
        s.put(0, sized_image(400)); // 400 > 100 but len would drop to 0 -> kept
        assert_eq!(s.lru.len(), 1, "at least one entry is always retained");
        assert!(s.contains(&0));
        assert_eq!(s.total_bytes, 400);
    }

    #[test]
    fn count_ceiling_never_exceeded_with_byte_budget_slack() {
        // Huge byte budget; many tiny pages must still respect the count ceiling.
        let mut s = store(3, u64::MAX);
        for i in 0..10 {
            s.put(i, sized_image(4));
        }
        assert!(s.lru.len() <= 3, "count ceiling must never be exceeded");
        // The byte total must match exactly what remains (3 pages x 4 bytes).
        assert_eq!(s.total_bytes, 12);
    }

    #[test]
    fn count_eviction_keeps_byte_total_in_sync() {
        // When the LRU sheds an entry for the count ceiling, its bytes must be
        // subtracted from the running total.
        let mut s = store(2, u64::MAX);
        s.put(0, sized_image(40));
        s.put(1, sized_image(40));
        s.put(2, sized_image(40)); // evicts page 0 (count ceiling)
        assert_eq!(s.lru.len(), 2);
        assert_eq!(s.total_bytes, 80, "evicted page's bytes must be subtracted");
        assert!(!s.contains(&0));
    }

    #[test]
    fn replacing_existing_key_adjusts_total() {
        let mut s = store(50, u64::MAX);
        s.put(0, sized_image(100));
        assert_eq!(s.total_bytes, 100);
        s.put(0, sized_image(40)); // same key, smaller buffer
        assert_eq!(s.lru.len(), 1);
        assert_eq!(s.total_bytes, 40, "re-put must replace, not accumulate");
    }

    // ---- ImageCache: byte budget wired through the real decode path ----

    /// A `PageSource` returning a precomputed PNG that decodes to a large RGBA
    /// buffer, so the byte budget (clamped floor 64 MiB) can be exercised.
    struct LargeSource {
        pages: usize,
        bytes: Vec<u8>,
        reads: Arc<AtomicUsize>,
    }

    impl PageSource for LargeSource {
        fn list_pages(&self) -> Vec<PageEntry> {
            vec![PageEntry { name: "p".into() }; self.pages]
        }
        fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
            if index >= self.pages {
                return Err(CoreError::IndexOutOfRange {
                    index,
                    len: self.pages,
                });
            }
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.clone())
        }
    }

    #[test]
    fn byte_budget_evicts_through_the_cache() {
        // A 2048x2048 page decodes to 16 MiB; with the minimum 64 MiB budget,
        // four pages fit exactly and the fifth forces an eviction.
        let img = image::RgbaImage::from_pixel(2048, 2048, image::Rgba([3, 3, 3, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let src = Arc::new(LargeSource {
            pages: 5,
            bytes,
            reads: Arc::clone(&reads),
        }) as Arc<dyn PageSource>;
        // radius 0 keeps prefetch inert; with_max_bytes clamps to MIN_MAX_BYTES (64 MiB).
        let cache = ImageCache::new(src, CacheConfig::new(50, 0).with_max_bytes(0));

        for i in 0..5 {
            cache.get(i).unwrap();
        }
        assert_eq!(reads.load(Ordering::SeqCst), 5);

        {
            let store = cache.inner.cache.lock().unwrap();
            assert!(
                store.total_bytes as u64 <= 64 * 1024 * 1024,
                "resident bytes must stay within the budget"
            );
            assert!(store.contains(&4), "the most-recent page is retained");
            assert!(!store.contains(&0), "the oldest page was evicted by bytes");
        }

        // Page 0 was evicted -> a fresh read; page 4 is still cached -> no read.
        cache.get(0).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 6);
        cache.get(4).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 6);
    }
}
