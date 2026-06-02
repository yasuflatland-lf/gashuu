//! Validated cache configuration value object.
//!
//! `CacheConfig` wraps the LRU `capacity` (always >= 1) and the prefetch
//! `radius` (0 means prefetch is disabled, a valid setting). `CacheConfig::new`
//! is the single place the `capacity >= 1` invariant is enforced, so an invalid
//! capacity cannot exist downstream. The type is an immutable value object.

use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};

/// Validated, immutable cache configuration.
///
/// Holds the LRU `capacity` (clamped to `>= 1` at construction) and the prefetch
/// `radius` (`0` disables prefetch). Constructing a `CacheConfig` is the only way
/// to obtain these values, so downstream consumers can rely on `capacity >= 1`.
///
/// Intentionally NOT `Deserialize`: deserializing would populate the private
/// fields directly and bypass `new`'s `capacity >= 1` clamp. Persistence goes
/// through `Settings`'s raw integer fields plus `Settings::cache_config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConfig {
    capacity: usize,
    radius: usize,
}

impl CacheConfig {
    /// Build a config. `capacity` is clamped to `>= 1` (0 has no meaning for an
    /// LRU). `radius` is taken verbatim (`0` disables prefetch, a deliberate and
    /// valid setting).
    pub fn new(capacity: usize, radius: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            radius,
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
    fn new_keeps_positive_capacity() {
        let cfg = CacheConfig::new(42, 5);
        assert_eq!(cfg.capacity(), 42);
    }

    #[test]
    fn new_keeps_radius_verbatim_including_zero() {
        assert_eq!(
            CacheConfig::new(10, 0).radius(),
            0,
            "radius 0 is a valid disabled-prefetch value"
        );
        assert_eq!(CacheConfig::new(10, 7).radius(), 7);
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
