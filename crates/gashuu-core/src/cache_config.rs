//! Validated cache configuration value object.
//!
//! `CacheConfig` bounds the prefetch radius and reserves enough cache slots for
//! the prefetch window and both pages of a spread. A radius of zero disables prefetch.

use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};

/// Hard upper bound for LRU cache capacity accepted by `CacheConfig::new`.
pub const MAX_CACHE_CAPACITY: usize = 100;

/// Hard upper bound for prefetch radius accepted by `CacheConfig::new`.
/// `0` remains a valid "prefetch disabled" value.
pub const MAX_PREFETCH_RADIUS: usize = 5;

const _: () = assert!(CacheConfig::min_capacity(MAX_PREFETCH_RADIUS) <= MAX_CACHE_CAPACITY);

/// Default total decoded-page byte budget (512 MiB). Used by `CacheConfig::new`
/// when no explicit budget is supplied via `with_max_bytes`.
pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Hard lower bound for the decoded-page byte budget (64 MiB). `with_max_bytes`
/// clamps to `[MIN_MAX_BYTES, u64::MAX]` so the budget can always hold at least
/// a few large pages.
pub const MIN_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Validated, immutable cache configuration.
///
/// The prefetch radius ranges from zero to `MAX_PREFETCH_RADIUS`. The LRU capacity
/// ranges from `2 * radius + 2` to `MAX_CACHE_CAPACITY`.
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
    /// Smallest capacity that holds the prefetch window plus a double spread.
    /// The supplied radius should be in `0..=MAX_PREFETCH_RADIUS`.
    pub const fn min_capacity(radius: usize) -> usize {
        2 * radius + 2
    }

    /// Clamp the capacity between the window floor and `MAX_CACHE_CAPACITY`.
    /// `radius` is clamped to `[0, MAX_PREFETCH_RADIUS]` (`0` disables prefetch).
    /// `max_bytes` defaults to `DEFAULT_MAX_BYTES`; use `with_max_bytes` to change it.
    pub fn new(capacity: usize, radius: usize) -> Self {
        let radius = radius.min(MAX_PREFETCH_RADIUS);
        Self {
            capacity: capacity.clamp(Self::min_capacity(radius), MAX_CACHE_CAPACITY),
            radius,
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

    /// LRU capacity in pages, at least `2 * radius + 2`.
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

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// C2-22: Cache configuration clamps each input to its documented bounds.
            /// Replaces `new_clamps_zero_capacity_to_the_radius_zero_floor`,
            /// `new_keeps_capacity_within_bounds`, `new_clamps_capacity_above_max`,
            /// `new_clamps_radius_above_max`, `new_keeps_radius_zero_as_prefetch_disabled`,
            /// `new_clamps_both_to_max_simultaneously`, `new_defaults_max_bytes`,
            /// `with_max_bytes_sets_an_in_range_value`, `with_max_bytes_clamps_below_min`,
            /// and `with_max_bytes_keeps_capacity_and_radius`.
            #[test]
            fn prop_cache_config_clamps_inputs(
                capacity in 0..2 * MAX_CACHE_CAPACITY,
                radius in 0..2 * MAX_PREFETCH_RADIUS,
                // The byte ceiling is u64::MAX. Sample around the default and include both extremes.
                bytes in prop_oneof![0..2 * DEFAULT_MAX_BYTES, Just(0), Just(u64::MAX)],
            ) {
                let expected_radius = radius.min(MAX_PREFETCH_RADIUS);
                // ADR-0003 (2026-11-23 amendment): 2 * radius + 2 holds the
                // prefetch window and a double spread.
                let expected_capacity = capacity.max(2 * expected_radius + 2).min(MAX_CACHE_CAPACITY);
                let config = CacheConfig::new(capacity, radius);
                prop_assert_eq!(config.radius(), expected_radius);
                prop_assert_eq!(config.capacity(), expected_capacity);
                prop_assert_eq!(config.max_bytes(), DEFAULT_MAX_BYTES);

                let configured = config.with_max_bytes(bytes);
                prop_assert_eq!(configured.max_bytes(), bytes.clamp(MIN_MAX_BYTES, u64::MAX));
                prop_assert_eq!(configured.capacity(), config.capacity());
                prop_assert_eq!(configured.radius(), config.radius());
            }
        }
    }

    #[test]
    fn new_raises_capacity_to_hold_the_prefetch_window() {
        for (capacity, radius, expected) in [
            (0, 0, 2),
            (1, 0, 2),
            (1, 1, 4),
            (1, 3, 8),
            (1, 5, 12),
            (7, 1, 7),
            (MAX_CACHE_CAPACITY + 1, 5, MAX_CACHE_CAPACITY),
            (1, MAX_PREFETCH_RADIUS + 9, 12),
        ] {
            assert_eq!(
                CacheConfig::new(capacity, radius).capacity(),
                expected,
                "capacity={capacity}, radius={radius}"
            );
        }
    }

    #[test]
    fn min_capacity_is_two_per_radius_plus_two() {
        for (radius, expected) in (0..=MAX_PREFETCH_RADIUS).zip([2, 4, 6, 8, 10, 12]) {
            assert_eq!(CacheConfig::min_capacity(radius), expected);
        }
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
