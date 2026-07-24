//! `ReadingProgress`: the one durable fact this app records — how far a reader got
//! in a book — modeled as an immutable core value object.

use std::num::NonZeroUsize;

/// How far a reader got in a book: the last-viewed resume page index
/// (`last_viewed`) and the known total page count (`total`; `None` = unknown /
/// never opened). Bundles the `fraction` / `current` derivation that the carousel
/// AND the resume logic both need, so the unknown/single-page guard and the
/// 1-based display offset live in ONE place. Immutable value object.
///
/// Semantics note (load-bearing): `last_viewed` stores the *leading page of the
/// last-viewed spread*, except that a spread containing the final page stores the
/// final page index so completion is representable. Both values resume to the
/// same spread after normalization. This is the reader's resume position, NOT
/// "the furthest page the reader saw", so it can decrease when the reader turns
/// back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingProgress {
    last_viewed: usize,
    total: Option<NonZeroUsize>,
}

impl ReadingProgress {
    /// Construct from the last-viewed resume page index and the total page count
    /// (`None` = unknown / never opened).
    pub fn new(last_viewed: usize, total: Option<NonZeroUsize>) -> Self {
        Self { last_viewed, total }
    }
    /// Last-viewed resume page index (0-based; 0 = never opened).
    pub fn last_viewed(&self) -> usize {
        self.last_viewed
    }
    /// Total pages (`None` = unknown).
    pub fn total(&self) -> Option<usize> {
        self.total.map(NonZeroUsize::get)
    }
    /// 1-based display page (`last_viewed + 1`, saturating). Always >= 1.
    pub fn current(&self) -> usize {
        self.last_viewed.saturating_add(1)
    }
    /// Position-normalized reading fraction in `0.0..=1.0`.
    ///
    /// Uses the last page index (`total - 1`) as the span so the final page reads
    /// exactly `1.0`. An unknown total or a one-page book yields `0.0`: a
    /// one-page book has no meaningful progress bar and is never considered
    /// finished. A stale `last_viewed` past the final index clamps to `1.0`.
    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(t) if t.get() > 1 => {
                (self.last_viewed as f32 / (t.get() - 1) as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
    /// Whether the resume position is at or past the final page index.
    ///
    /// An unknown total and a one-page book are never finished, mirroring
    /// [`Self::fraction`]'s decision that they have no meaningful progress bar.
    pub fn is_finished(&self) -> bool {
        self.total
            .is_some_and(|t| t.get() > 1 && self.last_viewed >= t.get() - 1)
    }
    /// At the start of the book: the last-viewed leading page index is `0`. True
    /// for a never-opened book AND for a read book left on its first page/cover
    /// (both record `last_viewed == 0`), so this is a start-position predicate, not a
    /// "never read" flag.
    pub fn is_at_start(&self) -> bool {
        self.last_viewed == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    fn nz(value: usize) -> Option<NonZeroUsize> {
        NonZeroUsize::new(value)
    }

    // --- fraction ---

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
        let p = ReadingProgress::new(0, nz(10));
        assert_eq!(p.fraction(), 0.0);
    }

    #[test]
    fn fraction_partway() {
        let p = ReadingProgress::new(5, nz(10));
        assert_eq!(p.fraction(), 5.0 / 9.0);
    }

    #[test]
    fn fraction_last_page_is_one() {
        let p = ReadingProgress::new(9, nz(10));
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn fraction_overshoot_clamps_to_one() {
        let p = ReadingProgress::new(99, nz(10));
        assert_eq!(p.fraction(), 1.0);
    }

    #[test]
    fn fraction_one_page_total_is_zero() {
        let p = ReadingProgress::new(0, nz(1));
        assert_eq!(p.fraction(), 0.0);
    }

    // --- is_finished ---

    #[test]
    fn is_finished_matrix() {
        let cases = [
            ("unread", ReadingProgress::new(0, nz(10)), false),
            ("mid-book", ReadingProgress::new(5, nz(10)), false),
            ("last page", ReadingProgress::new(9, nz(10)), true),
            ("one-page book", ReadingProgress::new(0, nz(1)), false),
            ("unknown total", ReadingProgress::new(9, None), false),
        ];

        for (name, progress, expected) in cases {
            assert_eq!(progress.is_finished(), expected, "{name}");
        }
    }

    // --- current ---

    #[test]
    fn current_at_zero_is_one() {
        let p = ReadingProgress::new(0, nz(10));
        assert_eq!(p.current(), 1);
    }

    #[test]
    fn current_partway() {
        let p = ReadingProgress::new(5, nz(10));
        assert_eq!(p.current(), 6);
    }

    #[test]
    fn current_saturates_at_usize_max() {
        let p = ReadingProgress::new(usize::MAX, None);
        assert_eq!(p.current(), usize::MAX);
    }

    // --- is_at_start ---

    #[test]
    fn is_at_start_true_when_last_viewed_zero() {
        let p = ReadingProgress::new(0, nz(10));
        assert!(p.is_at_start());
    }

    #[test]
    fn is_at_start_false_when_last_viewed_nonzero() {
        let p = ReadingProgress::new(1, nz(10));
        assert!(!p.is_at_start());
    }

    // --- getters ---

    #[test]
    fn getters_return_constructed_values() {
        let p = ReadingProgress::new(7, nz(42));
        assert_eq!(p.last_viewed(), 7);
        assert_eq!(p.total(), Some(42));
    }
}
