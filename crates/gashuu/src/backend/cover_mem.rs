use gashuu_core::DecodedImage;
use lru::LruCache;
use std::sync::Arc;

/// Byte budget for the in-memory decoded-cover LRU ([`CoverMemCache`]). A cover
/// is at most `max_side` (512) on its longer edge, so a decoded RGBA buffer is at
/// most 512×512×4 ≈ 1 MiB; 64 MiB therefore holds ~64 covers, a generous bound
/// for the rows a carousel sweeps back and forth over. This is a pure RAM/latency
/// tunable (it never touches the on-disk cache or any cache key), so it can be
/// raised or lowered freely without invalidating anything on disk.
pub(crate) const COVER_MEM_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// Byte-budgeted LRU of decoded carousel covers. On desktop, a memory hit
/// skips the persistent store's QOI read and decode. A hit returns a shared
/// `Arc<DecodedImage>` so the caller can reuse the RGBA buffer.
///
/// Eviction is by running byte total, not entry count: each `insert` adds the new
/// buffer's bytes and pops the least-recently-used entry until the total is back
/// within `budget`, always keeping at least one entry so a single oversized cover
/// is still served from memory (the floor). Shared across rayon workers behind an
/// `Arc<Mutex<_>>`; the lock is held only around `get`/`insert`, never across a
/// decode or any I/O.
pub(crate) struct CoverMemCache {
    /// Unbounded by entry count — the byte budget is the only cap, enforced in
    /// `insert`. Values are shared so a hit clones the `Arc`, not the bytes.
    lru: LruCache<String, Arc<DecodedImage>>,
    /// Running sum of `rgba().len()` over every cached cover; the eviction key.
    total_bytes: u64,
    /// Soft cap in bytes. `insert` evicts down to it (floor of one entry).
    budget: u64,
}

impl CoverMemCache {
    /// Build an empty cache with the default [`COVER_MEM_BUDGET_BYTES`] budget.
    pub(crate) fn new() -> Self {
        Self::with_budget(COVER_MEM_BUDGET_BYTES)
    }

    /// Build an empty cache with an explicit byte budget. Tests use this to
    /// exercise eviction with tiny buffers instead of allocating 64 MiB.
    fn with_budget(budget: u64) -> Self {
        Self {
            lru: LruCache::unbounded(),
            total_bytes: 0,
            budget,
        }
    }

    /// Fetch a cached cover by `key`, promoting it to most-recently-used (so a
    /// cover touched on this carousel pass survives a later eviction). Returns a
    /// cheap `Arc` clone, never a copy of the RGBA bytes.
    pub(crate) fn get(&mut self, key: &str) -> Option<Arc<DecodedImage>> {
        self.lru.get(key).map(Arc::clone)
    }

    /// Insert (or replace) a cover, then evict least-recently-used entries until
    /// the running byte total is within `budget`. Replacing an existing key first
    /// subtracts the old buffer's bytes so the total stays exact. At least one
    /// entry is always kept, so a single cover larger than the whole budget is
    /// still served from memory rather than thrashing in and out.
    pub(crate) fn insert(&mut self, key: String, img: Arc<DecodedImage>) {
        let added = img.rgba().len() as u64;
        if let Some(prev) = self.lru.put(key, img) {
            self.total_bytes = self.total_bytes.saturating_sub(prev.rgba().len() as u64);
        }
        self.total_bytes = self.total_bytes.saturating_add(added);
        while self.total_bytes > self.budget && self.lru.len() > 1 {
            let Some((_, evicted)) = self.lru.pop_lru() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.rgba().len() as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::VecDeque;

    /// Build an `Arc<DecodedImage>` whose RGBA buffer is exactly `bytes` long, so
    /// `CoverMemCache`'s byte accounting can be driven with tiny, predictable
    /// allocations. `bytes` must be a multiple of 4 (RGBA), enforced by the
    /// `DecodedImage::new` invariant (`rgba.len() == width * height * 4`); we make
    /// a `bytes/4`-wide, 1-tall image.
    fn cover_of(bytes: usize) -> Arc<DecodedImage> {
        assert_eq!(bytes % 4, 0, "test cover bytes must be a multiple of 4");
        let width = (bytes / 4) as u32;
        Arc::new(
            DecodedImage::new(vec![0u8; bytes], width, 1).expect("valid RGBA dimensions for test"),
        )
    }

    proptest::proptest! {
        #[test]
        fn cover_cache_matches_byte_budget_lru(
            budget in proptest::prop_oneof![0u64..=64, proptest::strategy::Just(u64::MAX)],
            ops in proptest::collection::vec(
                proptest::prop_oneof![
                    (0usize..8, 1usize..=16).prop_map(|(key, pixels)| CacheOp::Insert(key, pixels * 4)),
                    (0usize..8).prop_map(CacheOp::Get),
                ],
                0..=40,
            ),
        ) {
            let mut cache = CoverMemCache::with_budget(budget);
            let mut expected = VecDeque::<(usize, usize)>::new();
            for op in ops {
                match op {
                    CacheOp::Insert(key, bytes) => {
                        cache.insert(key.to_string(), cover_of(bytes));
                        expected.retain(|&(old_key, _)| old_key != key);
                        expected.push_front((key, bytes));
                        while expected.len() > 1
                            && expected.iter().map(|&(_, n)| n as u64).sum::<u64>() > budget
                        {
                            expected.pop_back();
                        }
                    }
                    CacheOp::Get(key) => {
                        let hit = expected.iter().position(|&(old_key, _)| old_key == key)
                            .map(|index| {
                                let entry = expected.remove(index).unwrap();
                                expected.push_front(entry);
                                entry.1
                            });
                        prop_assert_eq!(cache.get(&key.to_string()).map(|img| img.rgba().len()), hit);
                    }
                }
                let actual: Vec<_> = cache.lru.iter()
                    .map(|(key, image)| (key.parse::<usize>().unwrap(), image.rgba().len()))
                    .collect();
                prop_assert_eq!(actual.as_slice(), &*expected.make_contiguous());
                prop_assert_eq!(
                    cache.total_bytes,
                    expected.iter().map(|&(_, n)| n as u64).sum::<u64>()
                );
            }
        }
    }

    #[derive(Debug)]
    enum CacheOp {
        Insert(usize, usize),
        Get(usize),
    }

    /// The default constructor uses the documented 64 MiB budget tunable.
    #[test]
    fn default_budget_is_the_documented_constant() {
        let c = CoverMemCache::new();
        assert_eq!(c.budget, COVER_MEM_BUDGET_BYTES);
        assert_eq!(c.budget, 64 * 1024 * 1024);
        assert_eq!(c.total_bytes, 0, "a fresh cache holds nothing");
    }
}
