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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConfig {
    capacity: usize,
    radius: usize,
}

impl CacheConfig {
    /// Build a config. `capacity` is clamped to `[1, MAX_CACHE_SIZE]`.
    /// `radius` is clamped to `[0, MAX_PREFETCH_RADIUS]` (`0` disables prefetch).
    pub fn new(capacity: usize, radius: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, MAX_CACHE_SIZE),
            radius: radius.min(MAX_PREFETCH_RADIUS),
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
}
