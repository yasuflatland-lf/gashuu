//! Decoded-image cache: an LRU of `Arc<DecodedImage>` in front of a `PageSource`,
//! with background ±N prefetch via rayon so cache-hit page turns stay instant.
//!
//! `ImageCache::get_cached` returns a shared image and schedules neighbour prefetch on
//! a hit. On a miss, callers can use `CacheDispatch::decode_and_cache` to read and
//! decode on a worker and warm neighbouring pages. Prefetch prioritizes resident
//! pages by distance from the current page and silently drops decode failures.
//! Requesting a failed page directly surfaces the decode error.

use crate::cache_config::CacheConfig;
use crate::error::CoreError;
use crate::image_ops::{decode, DecodedImage};
use crate::page_source::PageSource;
use lru::LruCache;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

/// Default number of decoded images held in the LRU.
pub const DEFAULT_CAPACITY: usize = 50;
/// Default prefetch radius: pages on each side of the current page to warm.
pub const DEFAULT_PREFETCH_RADIUS: usize = 3;

/// An LRU of decoded pages paired with a running total of their decoded bytes, kept together so
/// ordering and byte accounting stay in sync. Eviction is two-dimensional: the `LruCache` capacity
/// enforces the count ceiling, and `max_bytes` caps the total decoded-byte footprint. A single
/// oversized page is retained as the floor (at least one entry always stays cached so the current
/// page survives).
pub struct DecodedLru<K: Hash + Eq + Clone> {
    lru: LruCache<K, Arc<DecodedImage>>,
    // The sum can briefly exceed the u64 budget before eviction.
    total_bytes: u128,
    max_bytes: u64,
}

impl<K: Hash + Eq + Clone> DecodedLru<K> {
    pub fn new(capacity: NonZeroUsize, max_bytes: u64) -> Self {
        Self {
            lru: LruCache::new(capacity),
            total_bytes: 0,
            max_bytes,
        }
    }

    /// Check for a cached page without promoting it.
    pub fn contains(&self, key: &K) -> bool {
        self.lru.contains(key)
    }

    /// Return the cached page, promoting it in the LRU.
    pub fn get(&mut self, key: &K) -> Option<&Arc<DecodedImage>> {
        self.lru.get(key)
    }

    /// Mark `key` most-recently used without reading the value. Returns whether it was present.
    pub fn promote(&mut self, key: &K) -> bool {
        self.lru.promote(key)
    }

    /// Insert `img` at `key`, updating the byte total, then evict the LRU end
    /// until both the count ceiling and the byte budget hold (always keeping the
    /// just-inserted page plus at least one entry overall).
    pub fn put(&mut self, key: K, img: Arc<DecodedImage>) {
        let new_bytes = img.rgba().len() as u128;
        // Push reports any displaced entry, including count-ceiling evictions.
        if let Some((_, displaced)) = self.lru.push(key, img) {
            self.total_bytes -= displaced.rgba().len() as u128;
        }
        self.total_bytes += new_bytes;
        self.evict_to_budget();
    }

    pub fn remove(&mut self, key: &K) -> Option<Arc<DecodedImage>> {
        let image = self.lru.pop(key)?;
        self.total_bytes -= image.rgba().len() as u128;
        Some(image)
    }

    /// Keep entries whose keys satisfy the predicate, preserving their recency order.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let removed: Vec<_> = self
            .lru
            .iter()
            .filter_map(|(key, _)| if keep(key) { None } else { Some(key.clone()) })
            .collect();
        for key in removed {
            self.remove(&key);
        }
    }

    /// Drop least-recently-used pages while the byte total exceeds `max_bytes`,
    /// retaining at least one entry (the floor for a single oversized page).
    fn evict_to_budget(&mut self) {
        while self.total_bytes > u128::from(self.max_bytes) && self.lru.len() > 1 {
            match self.lru.pop_lru() {
                Some((_, evicted)) => self.total_bytes -= evicted.rgba().len() as u128,
                None => break,
            }
        }
    }
}

/// Neighbour indices within `radius` of `center`, clamped to `[0, len)` and
/// excluding `center` itself, in ascending order.
fn prefetch_indices(center: usize, radius: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let lo = center.saturating_sub(radius);
    let hi = center.saturating_add(radius).min(len - 1);
    (lo..=hi).filter(|&i| i != center).collect()
}

/// Promote resident neighbours farthest first, breaking ties by ascending index.
/// Promote the current page last so it has priority over every neighbour.
fn promote_window(cache: &mut DecodedLru<usize>, center: usize, radius: usize, len: usize) {
    let mut neighbours = prefetch_indices(center, radius, len);
    neighbours.sort_unstable_by_key(|&i| (Reverse(i.abs_diff(center)), i));
    for i in neighbours {
        cache.promote(&i);
    }
    cache.promote(&center);
}

/// Shared, thread-safe cache state. Held behind an `Arc` so background prefetch
/// tasks can outlive the `ImageCache` handle without dangling.
///
/// Locking discipline: when both mutexes are held at once, always acquire
/// `cache` before `in_flight` (`cached` only takes `cache`). Keep this order in
/// any future code to avoid deadlock. No fallible or user-supplied code runs
/// while a lock is held — reads and decodes happen lock-free — so the mutexes
/// cannot be poisoned in practice, and the `lock().unwrap()` calls are an
/// intentional fail-fast.
struct Inner {
    source: Arc<dyn PageSource>,
    len: usize,
    cache: Mutex<DecodedLru<usize>>,
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
        let to_fetch: Vec<usize> = {
            let mut cache = self.cache.lock().unwrap();
            // Protect resident window pages from subsequent insertions.
            promote_window(&mut cache, center, radius, self.len);
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
        promote_window(&mut cache, center, radius, self.len);
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
                cache: Mutex::new(DecodedLru::new(cap, config.max_bytes())),
                in_flight: Mutex::new(HashSet::new()),
            }),
            radius: config.radius(),
        }
    }

    /// Return the decoded page at `index` if it is already cached, without ever
    /// reading or decoding on a miss.
    ///
    /// A hit promotes the entry in the LRU and spawns background neighbour
    /// prefetch. A miss leaves the cache untouched and schedules no work.
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
}

impl CacheDispatch {
    /// Look up a page and promote a hit in the LRU without reading or prefetching.
    fn cached(&self, index: usize) -> Option<Arc<DecodedImage>> {
        self.inner.cached(index)
    }

    /// Read the encoded page bytes directly from the source.
    fn read_raw(&self, index: usize) -> Result<Vec<u8>, CoreError> {
        self.inner.source.read_bytes(index)
    }

    /// Cache a decoded page, returning the existing handle if another reader won the race.
    fn insert_decoded(&self, index: usize, decoded: DecodedImage) -> Arc<DecodedImage> {
        self.inner.cache_decoded(index, Arc::new(decoded))
    }

    /// Read + decode page `index` on the calling thread, insert it into the
    /// shared cache, and warm neighbouring pages.
    pub fn decode_and_cache(&self, index: usize) -> Result<Arc<DecodedImage>, CoreError> {
        if let Some(img) = self.cached(index) {
            self.inner.spawn_prefetch(index, self.radius);
            return Ok(img);
        }

        let bytes = self.read_raw(index)?;

        if let Some(img) = self.cached(index) {
            self.inner.spawn_prefetch(index, self.radius);
            return Ok(img);
        }

        let img = self.insert_decoded(index, decode(&bytes)?);
        self.inner.spawn_prefetch(index, self.radius);
        Ok(img)
    }
}

#[cfg(test)]
#[path = "cache/tests.rs"]
mod tests;
