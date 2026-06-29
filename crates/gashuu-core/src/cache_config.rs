//! Validated cache configuration value object.
//!
//! `CacheConfig` wraps the LRU `capacity` (always >= 1 and <= MAX_CACHE_SIZE)
//! and the prefetch `radius` (0 means prefetch is disabled, a valid setting;
//! clamped to MAX_PREFETCH_RADIUS). `CacheConfig::new` is the single place both
//! bounds are enforced, so an invalid configuration cannot exist downstream.
//! The type is an immutable value object.

use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};

/// Hard upper bound for LRU cache capacity accepted by `CacheConfig::new`.
pub const MAX_CACHE_SIZE: usize = 100;

/// Hard upper bound for prefetch radius accepted by `CacheConfig::new`.
/// `0` remains a valid "prefetch disabled" value.
pub const MAX_PREFETCH_RADIUS: usize = 5;

/// Default total decoded-page byte budget (512 MiB). Used by `CacheConfig::new`
/// when no explicit budget is supplied via `with_max_bytes`.
pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Hard lower bound for the decoded-page byte budget (64 MiB). `with_max_bytes`
/// clamps to `[MIN_MAX_BYTES, u64::MAX]` so the budget can always hold at least
/// a few large pages.
pub const MIN_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Validated, immutable cache configuration.
///
/// Holds the LRU `capacity` (clamped to `[1, MAX_CACHE_SIZE]` at construction)
/// and the prefetch `radius` (clamped to `[0, MAX_PREFETCH_RADIUS]`; `0`
/// disables prefetch). Constructing a `CacheConfig` is the only way to obtain
/// these values, so downstream consumers can rely on the bounds being upheld.
///
/// Intentionally NOT `Deserialize`: deserializing would populate the private
/// fields directly and bypass `new`'s clamps. Persistence goes through
/// `Settings`'s raw integer fields plus `Settings::cache_config`.
///
/// The decoded-page cache evicts by two dimensions: the `capacity` count
/// ceiling and `max_bytes`, the total decoded-byte budget. `new` keeps its
/// two-argument signature and defaults `max_bytes` to `DEFAULT_MAX_BYTES`;
/// callers tune the budget with the chainable `with_max_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConfig {
    capacity: usize,
    radius: usize,
    max_bytes: u64,
}

impl CacheConfig {
    /// Build a config. `capacity` is clamped to `[1, MAX_CACHE_SIZE]`.
    /// `radius` is clamped to `[0, MAX_PREFETCH_RADIUS]` (`0` disables prefetch).
    /// `max_bytes` defaults to `DEFAULT_MAX_BYTES`; use `with_max_bytes` to change it.
    pub fn new(capacity: usize, radius: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, MAX_CACHE_SIZE),
            radius: radius.min(MAX_PREFETCH_RADIUS),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Return a copy with the decoded-page byte budget set to `bytes`, clamped to
    /// `[MIN_MAX_BYTES, u64::MAX]`. Chainable on top of `new`.
    pub fn with_max_bytes(self, bytes: u64) -> Self {
        Self {
            max_bytes: bytes.max(MIN_MAX_BYTES),
            ..self
        }
    }

    /// LRU capacity; always `>= 1`.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Prefetch radius; `0` means prefetch is disabled.
    pub fn radius(&self) -> usize {
        self.radius
    }

    /// Total decoded-page byte budget; always `>= MIN_MAX_BYTES`.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

impl Default for CacheConfig {
    /// Crate defaults: `DEFAULT_CAPACITY` capacity, `DEFAULT_PREFETCH_RADIUS` radius.
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clamps_zero_capacity_to_one() {
        let cfg = CacheConfig::new(0, 0);
        assert_eq!(cfg.capacity(), 1, "capacity 0 must clamp to 1");
    }

    #[test]
    fn new_keeps_capacity_within_bounds() {
        let cfg = CacheConfig::new(42, 3);
        assert_eq!(cfg.capacity(), 42);
    }

    #[test]
    fn new_clamps_capacity_above_max() {
        let cfg = CacheConfig::new(MAX_CACHE_SIZE + 1, 0);
        assert_eq!(cfg.capacity(), MAX_CACHE_SIZE);
    }

    #[test]
    fn new_clamps_radius_above_max() {
        let cfg = CacheConfig::new(10, MAX_PREFETCH_RADIUS + 1);
        assert_eq!(cfg.radius(), MAX_PREFETCH_RADIUS);
    }

    #[test]
    fn new_keeps_radius_zero_as_prefetch_disabled() {
        assert_eq!(
            CacheConfig::new(10, 0).radius(),
            0,
            "radius 0 is a valid disabled-prefetch value"
        );
    }

    #[test]
    fn new_clamps_both_to_max_simultaneously() {
        let cfg = CacheConfig::new(MAX_CACHE_SIZE + 99, MAX_PREFETCH_RADIUS + 99);
        assert_eq!(cfg.capacity(), MAX_CACHE_SIZE);
        assert_eq!(cfg.radius(), MAX_PREFETCH_RADIUS);
    }

    #[test]
    fn default_uses_crate_constants() {
        let cfg = CacheConfig::default();
        assert_eq!(cfg.capacity(), DEFAULT_CAPACITY);
        assert_eq!(cfg.radius(), DEFAULT_PREFETCH_RADIUS);
    }

    #[test]
    fn is_copy_and_eq() {
        let a = CacheConfig::new(8, 2);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn new_defaults_max_bytes() {
        assert_eq!(CacheConfig::new(50, 0).max_bytes(), DEFAULT_MAX_BYTES);
    }

    #[test]
    fn with_max_bytes_sets_an_in_range_value() {
        let bytes = 256 * 1024 * 1024;
        let cfg = CacheConfig::new(50, 0).with_max_bytes(bytes);
        assert_eq!(cfg.max_bytes(), bytes);
    }

    #[test]
    fn with_max_bytes_clamps_below_min() {
        let cfg = CacheConfig::new(50, 0).with_max_bytes(0);
        assert_eq!(cfg.max_bytes(), MIN_MAX_BYTES);
    }

    #[test]
    fn with_max_bytes_keeps_capacity_and_radius() {
        let cfg = CacheConfig::new(8, 2).with_max_bytes(MIN_MAX_BYTES);
        assert_eq!(cfg.capacity(), 8);
        assert_eq!(cfg.radius(), 2);
        assert_eq!(cfg.max_bytes(), MIN_MAX_BYTES);
    }
}
