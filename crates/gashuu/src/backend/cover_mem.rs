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

/// Process-lifetime, byte-budgeted in-memory LRU of decoded covers, checked
/// BEFORE the disk read in `spawn_load`. The on-disk cover cache HIT path still
/// `fs::read`s and PNG-decodes `<cache>/<key>.png` on every call
/// (`ThumbnailCache::get`), so a carousel scrolled back and forth re-decodes the
/// same covers repeatedly; serving an already-decoded cover from here removes
/// that repeat work. Covers are held as `Arc<DecodedImage>` so a mem hit is a
/// cheap refcount bump rather than an RGBA buffer copy.
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

    /// Inserting past the budget evicts least-recently-used entries so the running
    /// total never exceeds the budget. Budget = 12 bytes; three 8-byte covers can
    /// hold at most one (8 ≤ 12 < 16), so each insert evicts the prior one.
    #[test]
    fn insert_past_budget_evicts_lru_and_caps_total() {
        let mut c = CoverMemCache::with_budget(12);
        c.insert("a".into(), cover_of(8));
        c.insert("b".into(), cover_of(8));
        c.insert("c".into(), cover_of(8));
        assert!(
            c.total_bytes <= 12,
            "running total {} must stay within the budget",
            c.total_bytes
        );
        assert_eq!(c.lru.len(), 1, "only the most-recent cover survives");
        assert!(c.get("c").is_some(), "the most-recently inserted survives");
        assert!(c.get("a").is_none(), "the oldest was evicted");
        assert!(c.get("b").is_none(), "the middle was evicted");
    }

    /// `get` promotes an entry to most-recently-used, so a promoted entry survives
    /// a later eviction while the now-oldest untouched entry is dropped. Budget 16
    /// holds two 8-byte covers; touching "a" then inserting "c" must evict "b".
    #[test]
    fn get_promotes_mru_so_touched_entry_survives_eviction() {
        let mut c = CoverMemCache::with_budget(16);
        c.insert("a".into(), cover_of(8));
        c.insert("b".into(), cover_of(8));
        assert!(c.get("a").is_some(), "promote a to MRU (b is now the LRU)");
        c.insert("c".into(), cover_of(8));
        assert!(c.get("a").is_some(), "promoted entry survived eviction");
        assert!(c.get("c").is_some(), "newly inserted entry is present");
        assert!(
            c.get("b").is_none(),
            "the un-promoted LRU entry was evicted"
        );
        assert!(c.total_bytes <= 16, "total stays within the budget");
    }

    /// A single entry larger than the whole budget is retained (floor = 1): the
    /// eviction loop stops at one entry rather than dropping the only cover, so an
    /// oversized cover is still served from memory instead of thrashing.
    #[test]
    fn single_oversized_entry_is_retained_as_floor() {
        let mut c = CoverMemCache::with_budget(8);
        c.insert("big".into(), cover_of(64));
        assert_eq!(c.lru.len(), 1, "the oversized entry is kept as the floor");
        assert!(c.get("big").is_some(), "oversized cover is still served");
        // A second oversized insert still leaves exactly one (the newest) entry.
        c.insert("big2".into(), cover_of(64));
        assert_eq!(c.lru.len(), 1, "floor stays at one even when over budget");
        assert!(c.get("big2").is_some(), "the newest oversized cover wins");
        assert!(
            c.get("big").is_none(),
            "the prior oversized cover was evicted"
        );
    }

    /// Re-inserting an existing key replaces its value and adjusts the running
    /// total by the byte delta (old subtracted, new added) rather than
    /// double-counting — so a replace never inflates `total_bytes`.
    #[test]
    fn reinsert_same_key_replaces_without_double_counting() {
        let mut c = CoverMemCache::with_budget(64);
        c.insert("k".into(), cover_of(8));
        c.insert("k".into(), cover_of(16));
        assert_eq!(c.lru.len(), 1, "same key replaces, not appends");
        assert_eq!(c.total_bytes, 16, "total reflects only the new buffer");
        let got = c.get("k").expect("key present");
        assert_eq!(got.rgba().len(), 16, "the replacement value is served");
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
