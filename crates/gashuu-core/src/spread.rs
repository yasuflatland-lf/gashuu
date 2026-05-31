//! Pure page-pairing for two-page spread display. Slint/tracing-free and
//! reading-direction-agnostic: this module decides WHICH pages form a spread
//! (in reading order), never how they are placed left/right. Placement (RTL/LTR)
//! and input (which arrow advances) live in the presentation layer.

use crate::settings::{CoverMode, SpreadMode};

/// One displayed unit: 1–2 page indices in reading order (`leading` first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub leading: usize,
    pub trailing: Option<usize>,
}

/// Largest even page index `< total` (start of the final even-aligned pair).
/// Used by Double/Paired. Returns 0 when `total <= 1` (callers guard `total == 0`).
fn last_even(total: usize) -> usize {
    // (total - 1) rounded down to an even number; `& !1` clears the low bit.
    total.saturating_sub(1) & !1
}

/// Largest valid leading index for Double/Standalone: 0 or the largest odd
/// index `<= total - 1`. Returns 0 when `total <= 1`.
fn last_start_standalone(total: usize) -> usize {
    if total <= 1 {
        0
    } else if (total - 1) % 2 == 1 {
        // total - 1 is already odd: it is its own spread start.
        total - 1
    } else {
        // total - 1 is even (> 0): the largest odd index is one below it.
        total - 2
    }
}

/// The spread (1–2 reading-order page indices) whose leading page is `leading`.
///
/// `leading` is clamped into `[0, total - 1]` defensively; callers guarantee
/// `total > 0`. The returned `trailing` is present only when a partner page
/// actually exists within `total`.
pub fn spread_at(total: usize, mode: SpreadMode, cover: CoverMode, leading: usize) -> Spread {
    let max_index = total.saturating_sub(1);
    let lead = leading.min(max_index);

    match mode {
        SpreadMode::Single => Spread {
            leading: lead,
            trailing: None,
        },
        SpreadMode::Double => match cover {
            // Pairs start even: {0,1}{2,3}… The cover is paired with page 1.
            CoverMode::Paired => Spread {
                leading: lead,
                trailing: pair_trailing(total, lead),
            },
            // Cover stands alone, then {1,2}{3,4}…
            CoverMode::Standalone => {
                if lead == 0 {
                    Spread {
                        leading: 0,
                        trailing: None,
                    }
                } else {
                    Spread {
                        leading: lead,
                        trailing: pair_trailing(total, lead),
                    }
                }
            }
        },
    }
}

/// `Some(leading + 1)` when that partner page exists within `total`, else `None`.
fn pair_trailing(total: usize, leading: usize) -> Option<usize> {
    let trailing = leading.saturating_add(1);
    if trailing < total {
        Some(trailing)
    } else {
        None
    }
}

/// Leading index of the next spread in reading order, clamped at the final
/// spread so repeated "next" at the end is a no-op.
pub fn next_leading(total: usize, mode: SpreadMode, cover: CoverMode, leading: usize) -> usize {
    let max_index = total.saturating_sub(1);
    let lead = leading.min(max_index);

    match mode {
        SpreadMode::Single => lead.saturating_add(1).min(max_index),
        SpreadMode::Double => match cover {
            CoverMode::Paired => lead.saturating_add(2).min(last_even(total)),
            CoverMode::Standalone => {
                let last = last_start_standalone(total);
                if lead == 0 {
                    // Cover → first pair (or stay put when there is no second page).
                    if total > 1 {
                        1
                    } else {
                        0
                    }
                } else if lead % 2 == 1 {
                    // Odd leading is a valid pair start: advance by two, clamp.
                    lead.saturating_add(2).min(last)
                } else {
                    // Even (>0) leading shouldn't occur for a valid spread; be
                    // defensive — normalize onto a valid start, then advance.
                    let norm = normalize_leading(total, mode, cover, lead);
                    if norm == 0 {
                        if total > 1 {
                            1
                        } else {
                            0
                        }
                    } else {
                        norm.saturating_add(2).min(last)
                    }
                }
            }
        },
    }
}

/// Leading index of the previous spread in reading order, clamped at 0 so
/// repeated "prev" at the start is a no-op.
pub fn prev_leading(total: usize, mode: SpreadMode, cover: CoverMode, leading: usize) -> usize {
    let max_index = total.saturating_sub(1);
    let lead = leading.min(max_index);

    match mode {
        SpreadMode::Single => lead.saturating_sub(1),
        SpreadMode::Double => match cover {
            CoverMode::Paired => lead.saturating_sub(2),
            CoverMode::Standalone => {
                if lead <= 1 {
                    // From the first pair (1) or the cover (0), back to the cover.
                    0
                } else if lead % 2 == 1 {
                    // Odd > 1: previous pair start is two below.
                    lead - 2
                } else {
                    // Even (>0) shouldn't occur; normalize, then step back.
                    let norm = normalize_leading(total, mode, cover, lead);
                    if norm <= 1 {
                        0
                    } else {
                        norm - 2
                    }
                }
            }
        },
    }
}

/// Leading index of the spread that CONTAINS page `index`. Used after a
/// mode/cover toggle so the visible page stays on screen. `index` is clamped
/// into `[0, total - 1]`; callers guarantee `total > 0`.
pub fn normalize_leading(total: usize, mode: SpreadMode, cover: CoverMode, index: usize) -> usize {
    let max_index = total.saturating_sub(1);
    let idx = index.min(max_index);

    match mode {
        SpreadMode::Single => idx,
        SpreadMode::Double => match cover {
            // Round down to the even pair start, clamp to the final even start.
            CoverMode::Paired => (idx & !1).min(last_even(total)),
            CoverMode::Standalone => {
                let last = last_start_standalone(total);
                let start = if idx == 0 {
                    0
                } else if idx % 2 == 1 {
                    // Odd index is itself a pair start.
                    idx
                } else {
                    // Even (>0): the pair start is the odd index just below it.
                    idx - 1
                };
                start.min(last)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CoverMode, SpreadMode};

    // ---- helpers -----------------------------------------------------------

    fn sp(leading: usize, trailing: Option<usize>) -> Spread {
        Spread { leading, trailing }
    }

    /// Assert the visible `index` is one of the pages of the spread reached by
    /// normalizing then materializing it (the core toggle invariant).
    fn assert_contains(total: usize, mode: SpreadMode, cover: CoverMode, index: usize) {
        let lead = normalize_leading(total, mode, cover, index);
        let spread = spread_at(total, mode, cover, lead);
        let clamped = index.min(total - 1);
        let contains = spread.leading == clamped || spread.trailing == Some(clamped);
        assert!(
            contains,
            "index {clamped} not in {spread:?} (total={total}, mode={mode:?}, cover={cover:?})"
        );
    }

    // ---- SINGLE mode -------------------------------------------------------

    #[test]
    fn single_spread_at_trailing_always_none() {
        for cover in [CoverMode::Standalone, CoverMode::Paired] {
            for total in 1..=3 {
                for leading in 0..total {
                    let s = spread_at(total, SpreadMode::Single, cover, leading);
                    assert_eq!(s, sp(leading, None));
                }
            }
        }
    }

    #[test]
    fn single_spread_at_clamps_oob_leading() {
        // leading past the end clamps to the last page.
        assert_eq!(
            spread_at(3, SpreadMode::Single, CoverMode::Standalone, 99),
            sp(2, None)
        );
    }

    #[test]
    fn single_next_clamps_at_end() {
        let m = SpreadMode::Single;
        let c = CoverMode::Standalone;
        assert_eq!(next_leading(1, m, c, 0), 0);
        assert_eq!(next_leading(3, m, c, 0), 1);
        assert_eq!(next_leading(3, m, c, 1), 2);
        assert_eq!(next_leading(3, m, c, 2), 2); // clamp at last
        assert_eq!(next_leading(3, m, c, 99), 2); // oob clamp
    }

    #[test]
    fn single_prev_clamps_at_start() {
        let m = SpreadMode::Single;
        let c = CoverMode::Standalone;
        assert_eq!(prev_leading(3, m, c, 2), 1);
        assert_eq!(prev_leading(3, m, c, 1), 0);
        assert_eq!(prev_leading(3, m, c, 0), 0); // clamp at 0
    }

    #[test]
    fn single_normalize_is_identity_with_clamp() {
        let m = SpreadMode::Single;
        let c = CoverMode::Standalone;
        assert_eq!(normalize_leading(3, m, c, 0), 0);
        assert_eq!(normalize_leading(3, m, c, 2), 2);
        assert_eq!(normalize_leading(3, m, c, 99), 2); // clamp
    }

    #[test]
    fn single_does_not_panic_on_tiny_totals() {
        let m = SpreadMode::Single;
        let c = CoverMode::Standalone;
        // total == 1: every op stays at 0.
        assert_eq!(next_leading(1, m, c, 0), 0);
        assert_eq!(prev_leading(1, m, c, 0), 0);
        assert_eq!(normalize_leading(1, m, c, 0), 0);
        // total == 0: defensive, must not panic (callers guard this).
        assert_eq!(next_leading(0, m, c, 0), 0);
        assert_eq!(prev_leading(0, m, c, 0), 0);
        assert_eq!(normalize_leading(0, m, c, 0), 0);
    }

    // ---- DOUBLE + Paired ---------------------------------------------------

    #[test]
    fn double_paired_spread_at_full_pairs() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        // total 4: {0,1}{2,3}
        assert_eq!(spread_at(4, m, c, 0), sp(0, Some(1)));
        assert_eq!(spread_at(4, m, c, 2), sp(2, Some(3)));
    }

    #[test]
    fn double_paired_spread_at_last_partial_when_odd_total() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        // total 5: {0,1}{2,3}{4}
        assert_eq!(spread_at(5, m, c, 4), sp(4, None));
        // total 3: {0,1}{2}
        assert_eq!(spread_at(3, m, c, 2), sp(2, None));
        // total 1: {0}
        assert_eq!(spread_at(1, m, c, 0), sp(0, None));
    }

    #[test]
    fn double_paired_spread_at_total2() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        assert_eq!(spread_at(2, m, c, 0), sp(0, Some(1)));
    }

    #[test]
    fn double_paired_next_advances_by_two_and_clamps() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        // total 5: last_even = 4
        assert_eq!(next_leading(5, m, c, 0), 2);
        assert_eq!(next_leading(5, m, c, 2), 4);
        assert_eq!(next_leading(5, m, c, 4), 4); // clamp
                                                 // total 4: last_even = 2
        assert_eq!(next_leading(4, m, c, 0), 2);
        assert_eq!(next_leading(4, m, c, 2), 2); // clamp
                                                 // total 2: last_even = 0 -> stays
        assert_eq!(next_leading(2, m, c, 0), 0);
        // total 1: last_even = 0
        assert_eq!(next_leading(1, m, c, 0), 0);
    }

    #[test]
    fn double_paired_prev_steps_back_by_two_and_clamps() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        assert_eq!(prev_leading(5, m, c, 4), 2);
        assert_eq!(prev_leading(5, m, c, 2), 0);
        assert_eq!(prev_leading(5, m, c, 0), 0); // clamp
                                                 // odd leading (defensive) saturates down by two.
        assert_eq!(prev_leading(5, m, c, 1), 0);
    }

    #[test]
    fn double_paired_normalize_rounds_down_to_even() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        // total 5: last_even = 4
        assert_eq!(normalize_leading(5, m, c, 0), 0);
        assert_eq!(normalize_leading(5, m, c, 1), 0); // odd -> even down
        assert_eq!(normalize_leading(5, m, c, 2), 2);
        assert_eq!(normalize_leading(5, m, c, 3), 2);
        assert_eq!(normalize_leading(5, m, c, 4), 4);
        assert_eq!(normalize_leading(5, m, c, 99), 4); // clamp
                                                       // total 4: last_even = 2
        assert_eq!(normalize_leading(4, m, c, 3), 2);
    }

    #[test]
    fn double_paired_does_not_panic_on_tiny_totals() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        assert_eq!(next_leading(0, m, c, 0), 0);
        assert_eq!(prev_leading(0, m, c, 0), 0);
        assert_eq!(normalize_leading(0, m, c, 0), 0);
    }

    // ---- DOUBLE + Standalone ----------------------------------------------

    #[test]
    fn double_standalone_spread_at_cover_then_pairs() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 6: {0}{1,2}{3,4}{5}
        assert_eq!(spread_at(6, m, c, 0), sp(0, None));
        assert_eq!(spread_at(6, m, c, 1), sp(1, Some(2)));
        assert_eq!(spread_at(6, m, c, 3), sp(3, Some(4)));
        assert_eq!(spread_at(6, m, c, 5), sp(5, None)); // last odd standalone
    }

    #[test]
    fn double_standalone_spread_at_small_totals() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 1: {0}
        assert_eq!(spread_at(1, m, c, 0), sp(0, None));
        // total 2: {0}{1}
        assert_eq!(spread_at(2, m, c, 0), sp(0, None));
        assert_eq!(spread_at(2, m, c, 1), sp(1, None));
        // total 3: {0}{1,2}
        assert_eq!(spread_at(3, m, c, 0), sp(0, None));
        assert_eq!(spread_at(3, m, c, 1), sp(1, Some(2)));
        // total 4: {0}{1,2}{3}
        assert_eq!(spread_at(4, m, c, 3), sp(3, None));
        // total 5: {0}{1,2}{3,4}
        assert_eq!(spread_at(5, m, c, 3), sp(3, Some(4)));
    }

    #[test]
    fn double_standalone_last_start_values() {
        // last_start_standalone via normalize of a large index.
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        assert_eq!(normalize_leading(1, m, c, 99), 0); // total<=1 -> 0
        assert_eq!(normalize_leading(2, m, c, 99), 1); // total-1=1 odd -> 1
        assert_eq!(normalize_leading(3, m, c, 99), 1); // total-1=2 even -> 1
        assert_eq!(normalize_leading(4, m, c, 99), 3); // total-1=3 odd -> 3
        assert_eq!(normalize_leading(5, m, c, 99), 3); // total-1=4 even -> 3
        assert_eq!(normalize_leading(6, m, c, 99), 5); // total-1=5 odd -> 5
    }

    #[test]
    fn double_standalone_next() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 6: last_start = 5
        assert_eq!(next_leading(6, m, c, 0), 1); // cover -> first pair
        assert_eq!(next_leading(6, m, c, 1), 3);
        assert_eq!(next_leading(6, m, c, 3), 5);
        assert_eq!(next_leading(6, m, c, 5), 5); // clamp at last_start
                                                 // total 1: cover stays.
        assert_eq!(next_leading(1, m, c, 0), 0);
        // total 5: last_start = 3
        assert_eq!(next_leading(5, m, c, 1), 3);
        assert_eq!(next_leading(5, m, c, 3), 3); // clamp
    }

    #[test]
    fn double_standalone_prev() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 6
        assert_eq!(prev_leading(6, m, c, 5), 3);
        assert_eq!(prev_leading(6, m, c, 3), 1);
        assert_eq!(prev_leading(6, m, c, 1), 0); // first pair -> cover
        assert_eq!(prev_leading(6, m, c, 0), 0); // clamp
    }

    #[test]
    fn double_standalone_normalize() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 6: {0}{1,2}{3,4}{5}; last_start = 5
        assert_eq!(normalize_leading(6, m, c, 0), 0); // cover
        assert_eq!(normalize_leading(6, m, c, 1), 1); // odd -> same
        assert_eq!(normalize_leading(6, m, c, 2), 1); // even>0 -> -1
        assert_eq!(normalize_leading(6, m, c, 3), 3);
        assert_eq!(normalize_leading(6, m, c, 4), 3);
        assert_eq!(normalize_leading(6, m, c, 5), 5);
        // total 5: last_start = 3; index 4 (even>0) -> 3, within range.
        assert_eq!(normalize_leading(5, m, c, 4), 3);
        // total 3: last_start = 1; index 2 (even>0) -> 1.
        assert_eq!(normalize_leading(3, m, c, 2), 1);
    }

    #[test]
    fn double_standalone_next_defensive_even_leading() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // Even leading > 0 is not a valid start; treat as normalize-then-advance.
        // total 6: leading 2 normalizes to 1, next -> 3.
        assert_eq!(next_leading(6, m, c, 2), 3);
        // leading 4 normalizes to 3, next -> 5.
        assert_eq!(next_leading(6, m, c, 4), 5);
    }

    #[test]
    fn double_standalone_prev_defensive_even_leading() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        // total 6: leading 2 normalizes to 1, prev -> 0.
        assert_eq!(prev_leading(6, m, c, 2), 0);
        // leading 4 normalizes to 3, prev -> 1.
        assert_eq!(prev_leading(6, m, c, 4), 1);
    }

    #[test]
    fn double_standalone_does_not_panic_on_tiny_totals() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        assert_eq!(next_leading(0, m, c, 0), 0);
        assert_eq!(prev_leading(0, m, c, 0), 0);
        assert_eq!(normalize_leading(0, m, c, 0), 0);
    }

    // ---- Toggle boundary invariants ---------------------------------------

    #[test]
    fn toggle_boundary_single_index_5() {
        // single index 5 -> normalize under each (mode, cover).
        assert_eq!(
            normalize_leading(8, SpreadMode::Double, CoverMode::Paired, 5),
            4
        );
        assert_eq!(
            normalize_leading(8, SpreadMode::Double, CoverMode::Standalone, 5),
            5
        );
        assert_eq!(
            normalize_leading(8, SpreadMode::Single, CoverMode::Standalone, 5),
            5
        );
    }

    #[test]
    fn toggle_boundary_visible_page_stays_contained() {
        // For a spread of (mode, cover, index) transitions, normalizing then
        // materializing must keep the original page visible.
        let modes = [SpreadMode::Single, SpreadMode::Double];
        let covers = [CoverMode::Standalone, CoverMode::Paired];
        for total in 1..=8usize {
            for &mode in &modes {
                for &cover in &covers {
                    for index in 0..total {
                        assert_contains(total, mode, cover, index);
                    }
                    // Also exercise an out-of-range index (clamped path).
                    assert_contains(total, mode, cover, total + 5);
                }
            }
        }
    }

    #[test]
    fn next_prev_round_trip_double_paired() {
        let m = SpreadMode::Double;
        let c = CoverMode::Paired;
        // From 0, next then prev returns to 0 (when total allows advancing).
        let n = next_leading(6, m, c, 0);
        assert_eq!(n, 2);
        assert_eq!(prev_leading(6, m, c, n), 0);
    }

    #[test]
    fn next_prev_round_trip_double_standalone() {
        let m = SpreadMode::Double;
        let c = CoverMode::Standalone;
        let n = next_leading(6, m, c, 1);
        assert_eq!(n, 3);
        assert_eq!(prev_leading(6, m, c, n), 1);
    }
}
