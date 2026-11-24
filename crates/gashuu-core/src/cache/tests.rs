use super::*;
use crate::page_source::PageEntry;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

fn get(cache: &ImageCache, i: usize) -> Result<Arc<DecodedImage>, CoreError> {
    if let Some(img) = cache.get_cached(i) {
        return Ok(img);
    }
    cache.dispatch_handle().decode_and_cache(i)
}

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
fn read_raw_returns_source_bytes_without_populating_cache() {
    let (src, _) = counting(1);
    let dispatch = ImageCache::new(src, CacheConfig::new(1, 0)).dispatch_handle();
    assert_eq!(dispatch.read_raw(0).unwrap(), tiny_png());
    assert!(dispatch.cached(0).is_none());
}

#[test]
fn read_raw_propagates_source_index_error() {
    let (src, _) = counting(1);
    let dispatch = ImageCache::new(src, CacheConfig::new(1, 0)).dispatch_handle();
    assert!(matches!(
        dispatch.read_raw(2),
        Err(CoreError::IndexOutOfRange { index: 2, len: 1 })
    ));
}

#[test]
fn insert_decoded_shares_the_cached_arc_and_keeps_the_first_insert() {
    let (src, reads) = counting(1);
    let dispatch = ImageCache::new(src, CacheConfig::new(1, 0)).dispatch_handle();
    let page = DecodedImage::new(vec![1, 2, 3, 4], 1, 1).unwrap();
    let first = dispatch.insert_decoded(0, page.clone());
    let second = dispatch.insert_decoded(0, page);
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first, &dispatch.cached(0).unwrap()));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn hit_returns_same_arc_without_rereading() {
    let (src, reads) = counting(5);
    let cache = ImageCache::new(src, CacheConfig::new(50, 0));
    let a = get(&cache, 0).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let b = get(&cache, 0).unwrap();
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
    let a = get(&cache, 1).unwrap();
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
    let img = get(&cache, 2).unwrap();
    assert_eq!((img.width(), img.height()), (2, 3));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn lru_evicts_oldest_over_capacity() {
    let (src, reads) = counting(5);
    let cache = ImageCache::new(src, CacheConfig::new(2, 0)); // capacity 2
    get(&cache, 0).unwrap();
    get(&cache, 1).unwrap();
    get(&cache, 2).unwrap(); // evicts 0 (least recently used)
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    // 0 was evicted ⇒ a fresh read.
    get(&cache, 0).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    // 2 is still cached ⇒ no new read.
    get(&cache, 2).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 4);
}

#[test]
fn lru_hit_promotes_entry_and_evicts_true_lru() {
    // Distinguishes LRU from FIFO: a hit on 0 must promote it so a later miss
    // evicts 1 (the true LRU), not 0.
    let (src, reads) = counting(5);
    let cache = ImageCache::new(src, CacheConfig::new(2, 0)); // capacity 2, no prefetch
    get(&cache, 0).unwrap(); // [0]
    get(&cache, 1).unwrap(); // [0(lru), 1(mru)]
    get(&cache, 0).unwrap(); // hit: promotes 0 -> [1(lru), 0(mru)]
    assert_eq!(reads.load(Ordering::SeqCst), 2, "a hit must not re-read");
    get(&cache, 2).unwrap(); // miss: evicts 1 (true LRU), keeps 0
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    get(&cache, 0).unwrap(); // still cached as MRU -> no read
    assert_eq!(
        reads.load(Ordering::SeqCst),
        3,
        "promoted entry must survive"
    );
    get(&cache, 1).unwrap(); // evicted -> fresh read
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "true LRU must have been evicted"
    );
}

#[test]
fn cache_capacity_follows_the_config_floor() {
    let (src, _reads) = counting(5);
    let config = CacheConfig::new(0, 0);
    let cache = ImageCache::new(src, config);
    for i in 0..5 {
        get(&cache, i).unwrap();
    }
    assert_eq!(
        cache.inner.cache.lock().unwrap().lru.len(),
        config.capacity()
    );
    assert!(config.capacity() >= 1);
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
        get(&cache, i).unwrap();
    }
    assert!(cache.inner.cache.lock().unwrap().lru.len() <= 3);
}

// ---- prefetch: synchronous core, deterministic ----

#[test]
fn prefetch_keeps_an_already_cached_next_page() {
    let (src, _) = counting(12);
    let cache = ImageCache::new(src, CacheConfig::new(6, 2));
    let visit = |center| {
        cache.inner.cache_decoded(center, sized_image(4));
        cache.inner.prefetch_blocking(center, 2);
    };

    visit(5);
    assert!(cache.inner.cache.lock().unwrap().contains(&3));
    visit(2);
    let cache = cache.inner.cache.lock().unwrap();
    assert!(cache.contains(&3));
    let window: Vec<_> = cache.lru.iter().take(5).map(|(&key, _)| key).collect();
    assert_eq!(window, [2, 3, 1, 4, 0]);
}

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
    assert!(get(&cache, 2).is_err());
}

// ---- PageStore: byte-budget eviction (unit, exact byte sizes) ----

/// A decoded image whose RGBA buffer is exactly `bytes` long (`bytes` must be
/// a multiple of 4). Laid out as a 1-row image so the size is precise.
fn sized_image(bytes: usize) -> Arc<DecodedImage> {
    assert_eq!(bytes % 4, 0, "RGBA buffer length must be a multiple of 4");
    let width = (bytes / 4) as u32;
    Arc::new(DecodedImage::new(vec![0u8; bytes], width, 1).unwrap())
}

type PageStore = DecodedLru<usize>;

fn store(capacity: usize, max_bytes: u64) -> PageStore {
    PageStore::new(NonZeroUsize::new(capacity).unwrap(), max_bytes)
}

#[test]
fn promote_window_orders_center_then_nearest_neighbours() {
    let mut lru = store(7, u64::MAX);
    for key in 0..=6 {
        lru.put(key, sized_image(4));
    }

    promote_window(&mut lru, 3, 2, 7);
    let order: Vec<_> = lru.lru.iter().map(|(&key, _)| key).collect();
    assert_eq!(order, [3, 4, 2, 5, 1, 6, 0]);

    lru.put(7, sized_image(4));
    lru.put(8, sized_image(4));
    assert!((1..=5).all(|key| lru.contains(&key)));
    assert!(!lru.contains(&0));
    assert!(!lru.contains(&6));

    lru.put(9, sized_image(4));
    assert!(!lru.contains(&1));
    assert!((2..=5).all(|key| lru.contains(&key)));
}

#[test]
fn decoded_lru_promote_reports_presence() {
    let mut lru = store(2, u64::MAX);
    lru.put(0, sized_image(4));
    lru.put(1, sized_image(4));

    assert!(!lru.promote(&9));
    assert!(lru.promote(&0));
    lru.put(2, sized_image(4));

    assert!(lru.contains(&0));
    assert!(!lru.contains(&1));
}

#[test]
fn retain_drops_matching_keys_and_their_bytes() {
    let mut lru = DecodedLru::new(NonZeroUsize::new(3).unwrap(), 24);
    lru.put((1, 0), sized_image(4));
    lru.put((2, 0), sized_image(8));
    lru.put((1, 1), sized_image(12));
    lru.retain(|&(session, _)| session != 1);
    assert_eq!(lru.total_bytes, 8);
    assert_eq!(lru.lru.len(), 1);
    assert!(lru.contains(&(2, 0)));
    lru.put((2, 1), sized_image(16));
    assert_eq!(lru.lru.len(), 2);
    assert_eq!(lru.total_bytes, 24);
}

#[test]
fn remove_updates_total_bytes() {
    let mut lru = DecodedLru::new(NonZeroUsize::new(3).unwrap(), 12);
    lru.put("old", sized_image(4));
    lru.put("kept", sized_image(8));
    assert_eq!(lru.remove(&"old").unwrap().rgba().len(), 4);
    assert!(lru.remove(&"old").is_none());
    assert_eq!(lru.total_bytes, 8);
    lru.put("new", sized_image(4));
    assert!(lru.contains(&"kept"));
    assert_eq!(lru.total_bytes, 12);
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
        get(&cache, i).unwrap();
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
    get(&cache, 0).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 6);
    get(&cache, 4).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 6);
}
