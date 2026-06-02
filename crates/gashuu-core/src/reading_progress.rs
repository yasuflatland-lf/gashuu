//! `ReadingProgress`: the one durable fact this app records — how far a reader got
//! in a book — modeled as an immutable core value object.

/// How far a reader got in a book: the last-viewed leading page index (`reached`)
/// and the known total page count (`total`; 0 = unknown / never opened). Bundles
/// the `fraction` / `current` derivation that the carousel AND the resume logic
/// both need, so the `total == 0` guard and the 1-based display offset live in ONE
/// place. Immutable value object.
///
/// Semantics note (load-bearing, do NOT change in this PR): `reached` stores the
/// *leading page of the last-viewed spread*, not "the furthest page the reader
/// saw". This PR only NAMES the existing fact; redefining it is out of scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingProgress {
    reached: usize,
    total: usize,
}

impl ReadingProgress {
    /// Construct from the last-viewed leading page index and the total page count.
    pub fn new(reached: usize, total: usize) -> Self {
        Self { reached, total }
    }
    /// Last-viewed leading page index (0-based; 0 = never opened).
    pub fn reached(&self) -> usize {
        self.reached
    }
    /// Total pages (0 = unknown).
    pub fn total(&self) -> usize {
        self.total
    }
    /// 1-based display page (`reached + 1`, saturating). Always >= 1.
    pub fn current(&self) -> usize {
        self.reached.saturating_add(1)
    }
    /// Reading fraction in `0.0..=1.0`; `0.0` when `total == 0` (no div-by-zero,
    /// never NaN/inf); a stale `reached` past `total` clamps to `1.0`.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.reached as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
    /// Never opened / no recorded position.
    pub fn is_unread(&self) -> bool {
        self.reached == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fraction ---

    #[test]
    fn fraction_total_zero_is_always_zero() {
        let p = ReadingProgress::new(0, 0);
        assert_eq!(p.fraction(), 0.0);
        assert!(!p.fraction().is_nan());

        let p2 = ReadingProgress::new(5, 0);
        assert_eq!(p2.fraction(), 0.0);
        assert!(!p2.fraction().is_nan());
    }

    #[test]
    fn fraction_unread() {
        let p = ReadingProgress::new(0, 10);
        assert_eq!(p.fraction(), 0.0);
    }

    #[test]
    fn fraction_partway() {
        let p = ReadingProgress::new(5, 10);
        assert_eq!(p.fraction(), 0.5);
    }

    #[test]
    fn fraction_done() {
        let p = ReadingProgress::new(10, 10);
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn fraction_overshoot_clamps_to_one() {
        let p = ReadingProgress::new(99, 10);
        assert_eq!(p.fraction(), 1.0);
    }

    // --- current ---

    #[test]
    fn current_at_zero_is_one() {
        let p = ReadingProgress::new(0, 10);
        assert_eq!(p.current(), 1);
    }

    #[test]
    fn current_partway() {
        let p = ReadingProgress::new(5, 10);
        assert_eq!(p.current(), 6);
    }

    #[test]
    fn current_saturates_at_usize_max() {
        let p = ReadingProgress::new(usize::MAX, 0);
        assert_eq!(p.current(), usize::MAX);
    }

    // --- is_unread ---

    #[test]
    fn is_unread_true_when_reached_zero() {
        let p = ReadingProgress::new(0, 10);
        assert!(p.is_unread());
    }

    #[test]
    fn is_unread_false_when_reached_nonzero() {
        let p = ReadingProgress::new(1, 10);
        assert!(!p.is_unread());
    }

    // --- getters ---

    #[test]
    fn getters_return_constructed_values() {
        let p = ReadingProgress::new(7, 42);
        assert_eq!(p.reached(), 7);
        assert_eq!(p.total(), 42);
    }
}
