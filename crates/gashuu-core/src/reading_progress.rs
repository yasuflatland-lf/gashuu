//! `ReadingProgress`: the one durable fact this app records — how far a reader got
//! in a book — modeled as an immutable core value object.

/// How far a reader got in a book: the last-viewed leading page index (`reached`)
/// and the known total page count (`total`; `None` = unknown / never opened).
/// Bundles the `fraction` / `current` derivation that the carousel AND the resume
/// logic both need, so the unknown/zero-total guard and the 1-based display offset live
/// in ONE place. Immutable value object.
///
/// Semantics note (load-bearing, do NOT change in this PR): `reached` stores the
/// *leading page of the last-viewed spread*, not "the furthest page the reader
/// saw". This PR only NAMES the existing fact; redefining it is out of scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingProgress {
    reached: usize,
    total: Option<usize>,
}

impl ReadingProgress {
    /// Construct from the last-viewed leading page index and the total page count
    /// (`None` = unknown / never opened).
    pub fn new(reached: usize, total: Option<usize>) -> Self {
        Self { reached, total }
    }
    /// Last-viewed leading page index (0-based; 0 = never opened).
    pub fn reached(&self) -> usize {
        self.reached
    }
    /// Total pages (`None` = unknown).
    pub fn total(&self) -> Option<usize> {
        self.total
    }
    /// 1-based display page (`reached + 1`, saturating). Always >= 1.
    pub fn current(&self) -> usize {
        self.reached.saturating_add(1)
    }
    /// Reading fraction in `0.0..=1.0`; `0.0` when `total` is unknown or `0` (no
    /// div-by-zero, never NaN/inf); a stale `reached` past `total` clamps to `1.0`.
    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(t) if t > 0 => (self.reached as f32 / t as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
    /// At the start of the book: the last-viewed leading page index is `0`. True
    /// for a never-opened book AND for a read book left on its first page/cover
    /// (both record `reached == 0`), so this is a start-position predicate, not a
    /// "never read" flag.
    pub fn is_at_start(&self) -> bool {
        self.reached == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fraction ---

    #[test]
    fn fraction_total_some_zero_is_zero_not_nan() {
        // `Some(0)` is constructible, and `fraction()` must collapse it to 0.0 (the
        // `_ => 0.0` arm), never div-by-zero. Pins the "0.0 when total is 0" promise.
        let p = ReadingProgress::new(5, Some(0));
        assert_eq!(p.fraction(), 0.0);
        assert!(!p.fraction().is_nan() && p.fraction().is_finite());
    }

    #[test]
    fn fraction_total_none_is_always_zero() {
        let p = ReadingProgress::new(0, None);
        assert_eq!(p.fraction(), 0.0);
        assert!(!p.fraction().is_nan());

        let p2 = ReadingProgress::new(5, None);
        assert_eq!(p2.fraction(), 0.0);
        assert!(!p2.fraction().is_nan());
    }

    #[test]
    fn fraction_unread() {
        let p = ReadingProgress::new(0, Some(10));
        assert_eq!(p.fraction(), 0.0);
    }

    #[test]
    fn fraction_partway() {
        let p = ReadingProgress::new(5, Some(10));
        assert_eq!(p.fraction(), 0.5);
    }

    #[test]
    fn fraction_done() {
        let p = ReadingProgress::new(10, Some(10));
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn fraction_overshoot_clamps_to_one() {
        let p = ReadingProgress::new(99, Some(10));
        assert_eq!(p.fraction(), 1.0);
    }

    // --- current ---

    #[test]
    fn current_at_zero_is_one() {
        let p = ReadingProgress::new(0, Some(10));
        assert_eq!(p.current(), 1);
    }

    #[test]
    fn current_partway() {
        let p = ReadingProgress::new(5, Some(10));
        assert_eq!(p.current(), 6);
    }

    #[test]
    fn current_saturates_at_usize_max() {
        let p = ReadingProgress::new(usize::MAX, None);
        assert_eq!(p.current(), usize::MAX);
    }

    // --- is_at_start ---

    #[test]
    fn is_at_start_true_when_reached_zero() {
        let p = ReadingProgress::new(0, Some(10));
        assert!(p.is_at_start());
    }

    #[test]
    fn is_at_start_false_when_reached_nonzero() {
        let p = ReadingProgress::new(1, Some(10));
        assert!(!p.is_at_start());
    }

    // --- getters ---

    #[test]
    fn getters_return_constructed_values() {
        let p = ReadingProgress::new(7, Some(42));
        assert_eq!(p.reached(), 7);
        assert_eq!(p.total(), Some(42));
    }
}
