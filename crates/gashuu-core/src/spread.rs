//! Pure page-pairing for two-page spread display. Slint/tracing-free and
//! reading-direction-agnostic: this module decides WHICH pages form a spread
//! (in reading order), never how they are placed left/right. Placement (RTL/LTR)
//! and input (which arrow advances) live in the presentation layer.
//!
//! Pairing receives an already-resolved `SpreadLayout` (never `Auto`).
//! `SpreadNavigation` owns `SpreadMode::Auto` resolution from the viewport
//! aspect ratio and supplies that resolved layout to the pairing functions.
//!
//! The one direction-aware item here is the free fn `scrub_fraction_to_page`,
//! which takes `rtl` as an explicit parameter to invert a knob fraction. It maps
//! a scrub position to a RAW page index and carries no layout awareness, so the
//! pairing/placement separation above still holds: direction never reaches the
//! pairing functions or `SpreadNavigation`.

use crate::view_modes::{CoverMode, SpreadLayout, SpreadMode};
use std::num::NonZeroUsize;

/// One displayed unit: 1–2 page indices in reading order (`leading` first).
///
/// Invariant (when produced by this module): `trailing == Some(leading + 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub leading: usize,
    pub trailing: Option<usize>,
}

/// Clamp a page/leading index into `[0, total - 1]` (the shared defensive guard
/// every entry point applies; callers guarantee `total > 0`). Returns 0 when
/// `total == 0`.
fn clamp_index(total: usize, index: usize) -> usize {
    index.min(total.saturating_sub(1))
}

/// Leading index of the first Standalone pair: page 1 ({1,2}), or 0 (the cover)
/// when there is no second page to pair with.
fn first_pair_start(total: usize) -> usize {
    if total > 1 {
        1
    } else {
        0
    }
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
pub fn spread_at(total: usize, layout: SpreadLayout, cover: CoverMode, leading: usize) -> Spread {
    let lead = clamp_index(total, leading);

    match layout {
        SpreadLayout::Single => Spread {
            leading: lead,
            trailing: None,
        },
        SpreadLayout::Double => match cover {
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
pub fn next_leading(total: usize, layout: SpreadLayout, cover: CoverMode, leading: usize) -> usize {
    let lead = clamp_index(total, leading);

    match layout {
        SpreadLayout::Single => lead.saturating_add(1).min(total.saturating_sub(1)),
        SpreadLayout::Double => match cover {
            CoverMode::Paired => {
                // Even (>0) pair starts are the valid convention; be defensive
                // against an odd (invalid) leading — normalize onto the even
                // start first, then advance (symmetric with Standalone below).
                let start = normalize_leading(total, layout, cover, lead);
                start.saturating_add(2).min(last_even(total))
            }
            CoverMode::Standalone => {
                let last = last_start_standalone(total);
                if lead == 0 {
                    // Cover → first pair (or stay put when there is no second page).
                    first_pair_start(total)
                } else if lead % 2 == 1 {
                    // Odd leading is a valid pair start: advance by two, clamp.
                    lead.saturating_add(2).min(last)
                } else {
                    // Even (>0) leading shouldn't occur for a valid spread; be
                    // defensive — normalize onto a valid start, then advance.
                    let norm = normalize_leading(total, layout, cover, lead);
                    if norm == 0 {
                        first_pair_start(total)
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
pub fn prev_leading(total: usize, layout: SpreadLayout, cover: CoverMode, leading: usize) -> usize {
    let lead = clamp_index(total, leading);

    match layout {
        SpreadLayout::Single => lead.saturating_sub(1),
        SpreadLayout::Double => match cover {
            CoverMode::Paired => {
                // Defend against an odd (invalid) leading: normalize onto the
                // even start first, then step back (symmetric with Standalone).
                let start = normalize_leading(total, layout, cover, lead);
                start.saturating_sub(2)
            }
            CoverMode::Standalone => {
                if lead <= 1 {
                    // From the first pair (1) or the cover (0), back to the cover.
                    0
                } else if lead % 2 == 1 {
                    // Odd > 1: previous pair start is two below.
                    lead - 2
                } else {
                    // Even (>0) shouldn't occur; normalize, then step back.
                    let norm = normalize_leading(total, layout, cover, lead);
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
pub fn normalize_leading(
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
    index: usize,
) -> usize {
    let idx = clamp_index(total, index);

    match layout {
        SpreadLayout::Single => idx,
        SpreadLayout::Double => match cover {
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

/// Bundles the three values that always travel together when querying spreads —
/// total page count, resolved layout, and cover mode — and owns the pairing
/// queries so call sites read as intent rather than positional-arg plumbing.
///
/// Holds an already-resolved `SpreadLayout` (no `Auto` resolution here; that
/// stays in the UI layer). This is an additive cohesive wrapper over the pure
/// free functions, which remain the single source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpreadContext {
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
}

impl SpreadContext {
    pub fn new(total: usize, layout: SpreadLayout, cover: CoverMode) -> Self {
        Self {
            total,
            layout,
            cover,
        }
    }

    pub fn spread_at(&self, leading: usize) -> Spread {
        spread_at(self.total, self.layout, self.cover, leading)
    }

    pub fn normalize(&self, index: usize) -> usize {
        normalize_leading(self.total, self.layout, self.cover, index)
    }

    pub fn next(&self, index: usize) -> usize {
        next_leading(self.total, self.layout, self.cover, index)
    }

    pub fn prev(&self, index: usize) -> usize {
        prev_leading(self.total, self.layout, self.cover, index)
    }
}

/// Mutable spread position plus every input that determines page pairing.
///
/// `index` is always a valid leading page for the current resolved layout and
/// cover mode. All mutations that can affect pairing re-normalize it before
/// returning, so callers cannot observe an intermediate invalid position.
#[derive(Debug, Clone)]
pub struct SpreadNavigation {
    total: usize,
    spread_mode: SpreadMode,
    cover_mode: CoverMode,
    viewport_aspect: f32,
    index: usize,
}

impl SpreadNavigation {
    /// Construct empty navigation with the requested display modes.
    pub fn new(spread_mode: SpreadMode, cover_mode: CoverMode) -> Self {
        Self {
            total: 0,
            spread_mode,
            cover_mode,
            viewport_aspect: 1.0,
            index: 0,
        }
    }

    /// Replace the page count and reset navigation to the first page.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        self.index = 0;
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn total_nonzero(&self) -> Option<NonZeroUsize> {
        NonZeroUsize::new(self.total)
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Return the current resolved pairing context.
    pub fn context(&self) -> SpreadContext {
        SpreadContext::new(
            self.total,
            self.spread_mode.resolve(self.viewport_aspect),
            self.cover_mode,
        )
    }

    /// Return the current spread, or `None` when there are no pages.
    pub fn spread(&self) -> Option<Spread> {
        self.total_nonzero()
            .map(|_| self.context().spread_at(self.index))
    }

    /// Jump to the spread containing `page`, returning whether the index moved.
    pub fn jump_to(&mut self, page: usize) -> bool {
        if self.total == 0 {
            return false;
        }
        let target = self.context().normalize(page);
        let moved = target != self.index;
        self.index = target;
        moved
    }

    /// Advance one spread, returning whether the index moved.
    #[allow(clippy::should_implement_trait)] // Domain navigation, not iterator consumption.
    pub fn next(&mut self) -> bool {
        if self.total == 0 {
            return false;
        }
        let next = self.context().next(self.index);
        let moved = next != self.index;
        self.index = next;
        moved
    }

    /// Retreat one spread, returning whether the index moved.
    pub fn prev(&mut self) -> bool {
        if self.total == 0 {
            return false;
        }
        let prev = self.context().prev(self.index);
        let moved = prev != self.index;
        self.index = prev;
        moved
    }

    /// Set the configured spread mode and re-normalize after a real change.
    pub fn set_spread_mode(&mut self, mode: SpreadMode) -> bool {
        if self.spread_mode == mode {
            return false;
        }
        self.spread_mode = mode;
        self.renormalize_index();
        true
    }

    /// Set the cover mode and re-normalize after a real change.
    pub fn set_cover_mode(&mut self, mode: CoverMode) -> bool {
        if self.cover_mode == mode {
            return false;
        }
        self.cover_mode = mode;
        self.renormalize_index();
        true
    }

    /// Update the viewport aspect ratio used to resolve `SpreadMode::Auto`.
    ///
    /// Returns `true` only when the effective layout changes. A non-finite or
    /// non-positive ratio is stored as `1.0`.
    pub fn set_viewport_size(&mut self, width: f32, height: f32) -> bool {
        let before = self.spread_mode.resolve(self.viewport_aspect);
        let aspect = width / height;
        self.viewport_aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let after = self.spread_mode.resolve(self.viewport_aspect);
        self.renormalize_index();
        before != after
    }

    /// Return the spread a jump to `page` would select without moving.
    pub fn preview_spread(&self, page: usize) -> Option<Spread> {
        self.total_nonzero().map(|_| {
            let context = self.context();
            context.spread_at(context.normalize(page))
        })
    }

    /// Return whether a jump to `page` would select a two-page spread.
    pub fn preview_is_double(&self, page: usize) -> bool {
        self.preview_spread(page)
            .is_some_and(|spread| spread.trailing.is_some())
    }

    /// Return the page index that should be persisted as the resume position.
    ///
    /// When the current spread contains the final page, persist that final page
    /// so a finished book reaches 100%; otherwise persist the spread leading.
    pub fn resume_index_to_persist(&self) -> usize {
        let Some(total) = self.total_nonzero().map(NonZeroUsize::get) else {
            return self.index;
        };
        let final_index = total - 1;
        let spread = self.context().spread_at(self.index);
        if spread.trailing == Some(final_index) || spread.leading == final_index {
            final_index
        } else {
            self.index
        }
    }

    pub fn spread_mode(&self) -> SpreadMode {
        self.spread_mode
    }

    pub fn cover_mode(&self) -> CoverMode {
        self.cover_mode
    }

    fn renormalize_index(&mut self) {
        self.index = if self.total == 0 {
            0
        } else {
            self.context().normalize(self.index)
        };
    }
}

/// Map a scrub fraction (knob position along the track, 0.0 = screen-left edge,
/// 1.0 = screen-right edge) to a 0-based RAW page index. Pure and total: clamps
/// the fraction to `[0, 1]` (a non-finite input clamps to 0), guards
/// `page_count == 0` (returns 0), and rounds to the nearest page using the last
/// index (`page_count - 1`) as the span so BOTH ends are reachable.
///
/// `rtl` inverts the fraction: in right-to-left (manga) reading, dragging the
/// knob LEFT advances the page, so the screen-left edge maps to the LAST page
/// and the screen-right edge to the FIRST (spec §5 / decision 11). The returned
/// index is RAW; the caller normalizes it to a valid spread leading via
/// `ViewerState::jump_to` (which respects single/double + cover mode), so this
/// helper carries NO layout awareness — only direction and clamping.
// Single source of rounding (#71 Part D): the Slint scrubber passes the RAW clamped
// fraction via preview/commit; `handlers/viewer.rs` resolves the page through this helper.
pub fn scrub_fraction_to_page(fraction: f32, page_count: usize, rtl: bool) -> usize {
    if page_count == 0 {
        return 0;
    }
    // Clamp; a non-finite fraction (NaN/inf) collapses to 0 first.
    let f = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let f = if rtl { 1.0 - f } else { f };
    let last = page_count - 1;
    // Round half-up: +0.5 then floor (via `as usize` truncation on a non-negative
    // value). `last` fits f32 exactly for any realistic page count.
    let scaled = (f * last as f32 + 0.5) as usize;
    scaled.min(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_modes::{CoverMode, SpreadLayout, SpreadMode};

    // ---- helpers -----------------------------------------------------------

    fn sp(leading: usize, trailing: Option<usize>) -> Spread {
        Spread { leading, trailing }
    }

    /// Assert the visible `index` is one of the pages of the spread reached by
    /// normalizing then materializing it (the core toggle invariant).
    fn assert_contains(total: usize, layout: SpreadLayout, cover: CoverMode, index: usize) {
        let lead = normalize_leading(total, layout, cover, index);
        let spread = spread_at(total, layout, cover, lead);
        let clamped = index.min(total - 1);
        let contains = spread.leading == clamped || spread.trailing == Some(clamped);
        assert!(
            contains,
            "index {clamped} not in {spread:?} (total={total}, layout={layout:?}, cover={cover:?})"
        );
    }

    // ---- SINGLE mode -------------------------------------------------------

    #[test]
    fn single_spread_at_trailing_always_none() {
        for cover in [CoverMode::Standalone, CoverMode::Paired] {
            for total in 1..=3 {
                for leading in 0..total {
                    let s = spread_at(total, SpreadLayout::Single, cover, leading);
                    assert_eq!(s, sp(leading, None));
                }
            }
        }
    }

    #[test]
    fn single_spread_at_clamps_oob_leading() {
        // leading past the end clamps to the last page.
        assert_eq!(
            spread_at(3, SpreadLayout::Single, CoverMode::Standalone, 99),
            sp(2, None)
        );
    }

    #[test]
    fn single_next_clamps_at_end() {
        let m = SpreadLayout::Single;
        let c = CoverMode::Standalone;
        assert_eq!(next_leading(1, m, c, 0), 0);
        assert_eq!(next_leading(3, m, c, 0), 1);
        assert_eq!(next_leading(3, m, c, 1), 2);
        assert_eq!(next_leading(3, m, c, 2), 2); // clamp at last
        assert_eq!(next_leading(3, m, c, 99), 2); // oob clamp
    }

    #[test]
    fn single_prev_clamps_at_start() {
        let m = SpreadLayout::Single;
        let c = CoverMode::Standalone;
        assert_eq!(prev_leading(3, m, c, 2), 1);
        assert_eq!(prev_leading(3, m, c, 1), 0);
        assert_eq!(prev_leading(3, m, c, 0), 0); // clamp at 0
    }

    #[test]
    fn single_normalize_is_identity_with_clamp() {
        let m = SpreadLayout::Single;
        let c = CoverMode::Standalone;
        assert_eq!(normalize_leading(3, m, c, 0), 0);
        assert_eq!(normalize_leading(3, m, c, 2), 2);
        assert_eq!(normalize_leading(3, m, c, 99), 2); // clamp
    }

    #[test]
    fn single_does_not_panic_on_tiny_totals() {
        let m = SpreadLayout::Single;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        // total 4: {0,1}{2,3}
        assert_eq!(spread_at(4, m, c, 0), sp(0, Some(1)));
        assert_eq!(spread_at(4, m, c, 2), sp(2, Some(3)));
    }

    #[test]
    fn double_paired_spread_at_last_partial_when_odd_total() {
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        assert_eq!(spread_at(2, m, c, 0), sp(0, Some(1)));
    }

    #[test]
    fn double_paired_next_advances_by_two_and_clamps() {
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        assert_eq!(prev_leading(5, m, c, 4), 2);
        assert_eq!(prev_leading(5, m, c, 2), 0);
        assert_eq!(prev_leading(5, m, c, 0), 0); // clamp
                                                 // odd leading (defensive) saturates down by two.
        assert_eq!(prev_leading(5, m, c, 1), 0);
    }

    #[test]
    fn double_paired_next_defensive_odd_leading() {
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        // Odd (invalid) leading normalizes to the even start, then advances.
        // total 5: normalize(1) -> 0, next -> min(0+2, last_even(5)=4) = 2.
        assert_eq!(next_leading(5, m, c, 1), 2);
        // total 5: normalize(3) -> 2, next -> min(2+2, 4) = 4.
        assert_eq!(next_leading(5, m, c, 3), 4);
        // total 4: normalize(3) -> min(2, last_even(4)=2) = 2, next -> min(4, 2) = 2 (clamp).
        assert_eq!(next_leading(4, m, c, 3), 2);
        // In-range even inputs are unchanged (no regression).
        assert_eq!(next_leading(5, m, c, 0), 2);
        assert_eq!(next_leading(5, m, c, 2), 4);
    }

    #[test]
    fn double_paired_prev_defensive_odd_leading() {
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        // total 4: normalize(3) -> 2, prev -> 2-2 = 0 (an even, valid start).
        assert_eq!(prev_leading(4, m, c, 3), 0);
        // total 5: normalize(3) -> 2, prev -> 0.
        assert_eq!(prev_leading(5, m, c, 3), 0);
        // total 6: normalize(5) -> min(4, last_even(6)=4) = 4, prev -> 2.
        assert_eq!(prev_leading(6, m, c, 5), 2);
        // In-range even inputs are unchanged (no regression).
        assert_eq!(prev_leading(4, m, c, 2), 0);
        assert_eq!(prev_leading(5, m, c, 4), 2);
    }

    #[test]
    fn double_paired_next_prev_always_even_for_any_leading() {
        // For any leading (including odd, invalid inputs), the Paired start
        // returned by next/prev is always even (a valid pair start).
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        for total in 1..=12usize {
            for leading in 0..total + 3 {
                let n = next_leading(total, m, c, leading);
                let p = prev_leading(total, m, c, leading);
                assert_eq!(n % 2, 0, "next {n} odd (total={total}, leading={leading})");
                assert_eq!(p % 2, 0, "prev {p} odd (total={total}, leading={leading})");
            }
        }
    }

    #[test]
    fn double_paired_normalize_rounds_down_to_even() {
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        assert_eq!(next_leading(0, m, c, 0), 0);
        assert_eq!(prev_leading(0, m, c, 0), 0);
        assert_eq!(normalize_leading(0, m, c, 0), 0);
    }

    // ---- DOUBLE + Standalone ----------------------------------------------

    #[test]
    fn double_standalone_spread_at_cover_then_pairs() {
        let m = SpreadLayout::Double;
        let c = CoverMode::Standalone;
        // total 6: {0}{1,2}{3,4}{5}
        assert_eq!(spread_at(6, m, c, 0), sp(0, None));
        assert_eq!(spread_at(6, m, c, 1), sp(1, Some(2)));
        assert_eq!(spread_at(6, m, c, 3), sp(3, Some(4)));
        assert_eq!(spread_at(6, m, c, 5), sp(5, None)); // last odd standalone
    }

    #[test]
    fn double_standalone_spread_at_small_totals() {
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Standalone;
        // total 6
        assert_eq!(prev_leading(6, m, c, 5), 3);
        assert_eq!(prev_leading(6, m, c, 3), 1);
        assert_eq!(prev_leading(6, m, c, 1), 0); // first pair -> cover
        assert_eq!(prev_leading(6, m, c, 0), 0); // clamp
    }

    #[test]
    fn double_standalone_normalize() {
        let m = SpreadLayout::Double;
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Standalone;
        // Even leading > 0 is not a valid start; treat as normalize-then-advance.
        // total 6: leading 2 normalizes to 1, next -> 3.
        assert_eq!(next_leading(6, m, c, 2), 3);
        // leading 4 normalizes to 3, next -> 5.
        assert_eq!(next_leading(6, m, c, 4), 5);
    }

    #[test]
    fn double_standalone_prev_defensive_even_leading() {
        let m = SpreadLayout::Double;
        let c = CoverMode::Standalone;
        // total 6: leading 2 normalizes to 1, prev -> 0.
        assert_eq!(prev_leading(6, m, c, 2), 0);
        // leading 4 normalizes to 3, prev -> 1.
        assert_eq!(prev_leading(6, m, c, 4), 1);
    }

    #[test]
    fn double_standalone_does_not_panic_on_tiny_totals() {
        let m = SpreadLayout::Double;
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
            normalize_leading(8, SpreadLayout::Double, CoverMode::Paired, 5),
            4
        );
        assert_eq!(
            normalize_leading(8, SpreadLayout::Double, CoverMode::Standalone, 5),
            5
        );
        assert_eq!(
            normalize_leading(8, SpreadLayout::Single, CoverMode::Standalone, 5),
            5
        );
    }

    #[test]
    fn toggle_boundary_visible_page_stays_contained() {
        // For a spread of (mode, cover, index) transitions, normalizing then
        // materializing must keep the original page visible.
        let modes = [SpreadLayout::Single, SpreadLayout::Double];
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
        let m = SpreadLayout::Double;
        let c = CoverMode::Paired;
        // From 0, next then prev returns to 0 (when total allows advancing).
        let n = next_leading(6, m, c, 0);
        assert_eq!(n, 2);
        assert_eq!(prev_leading(6, m, c, n), 0);
    }

    #[test]
    fn next_prev_round_trip_double_standalone() {
        let m = SpreadLayout::Double;
        let c = CoverMode::Standalone;
        let n = next_leading(6, m, c, 1);
        assert_eq!(n, 3);
        assert_eq!(prev_leading(6, m, c, n), 1);
    }

    // ---- SpreadContext delegation -----------------------------------------

    #[test]
    fn spread_context_delegates_to_free_fns() {
        let ctx = SpreadContext::new(10, SpreadLayout::Double, CoverMode::Paired);
        assert_eq!(
            ctx.spread_at(0),
            spread_at(10, SpreadLayout::Double, CoverMode::Paired, 0)
        );
        assert_eq!(
            ctx.normalize(3),
            normalize_leading(10, SpreadLayout::Double, CoverMode::Paired, 3)
        );
        assert_eq!(
            ctx.next(2),
            next_leading(10, SpreadLayout::Double, CoverMode::Paired, 2)
        );
        assert_eq!(
            ctx.prev(4),
            prev_leading(10, SpreadLayout::Double, CoverMode::Paired, 4)
        );
    }

    #[test]
    fn spread_context_delegates_standalone() {
        let ctx = SpreadContext::new(6, SpreadLayout::Double, CoverMode::Standalone);
        assert_eq!(
            ctx.spread_at(1),
            spread_at(6, SpreadLayout::Double, CoverMode::Standalone, 1)
        );
        assert_eq!(
            ctx.normalize(2),
            normalize_leading(6, SpreadLayout::Double, CoverMode::Standalone, 2)
        );
        assert_eq!(
            ctx.next(1),
            next_leading(6, SpreadLayout::Double, CoverMode::Standalone, 1)
        );
        assert_eq!(
            ctx.prev(3),
            prev_leading(6, SpreadLayout::Double, CoverMode::Standalone, 3)
        );
    }

    #[test]
    fn spread_context_delegates_single_layout() {
        let ctx = SpreadContext::new(5, SpreadLayout::Single, CoverMode::Standalone);
        assert_eq!(
            ctx.spread_at(3),
            spread_at(5, SpreadLayout::Single, CoverMode::Standalone, 3)
        );
        assert_eq!(
            ctx.normalize(3),
            normalize_leading(5, SpreadLayout::Single, CoverMode::Standalone, 3)
        );
        assert_eq!(
            ctx.next(3),
            next_leading(5, SpreadLayout::Single, CoverMode::Standalone, 3)
        );
        assert_eq!(
            ctx.prev(3),
            prev_leading(5, SpreadLayout::Single, CoverMode::Standalone, 3)
        );
    }

    #[test]
    fn spread_context_is_copy_and_eq() {
        let ctx = SpreadContext::new(5, SpreadLayout::Single, CoverMode::Standalone);
        let ctx2 = ctx; // Copy
        assert_eq!(ctx, ctx2);
    }

    // ---- SpreadNavigation -------------------------------------------------

    #[test]
    fn index_is_always_a_valid_leading() {
        let mut failures = Vec::new();

        for total in 1..=9 {
            for spread_mode in [SpreadMode::Single, SpreadMode::Double] {
                for cover_mode in [CoverMode::Standalone, CoverMode::Paired] {
                    let label = format!("total={total}, {spread_mode:?}/{cover_mode:?}");
                    let mut nav = SpreadNavigation::new(spread_mode, cover_mode);

                    nav.set_total(total);
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("set_total left an invalid index: {label}"));
                    }

                    nav.jump_to(total - 1);
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("jump_to left an invalid index: {label}"));
                    }

                    nav.next();
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("next left an invalid index: {label}"));
                    }

                    nav.prev();
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("prev left an invalid index: {label}"));
                    }

                    nav.set_spread_mode(SpreadMode::Single);
                    nav.jump_to(total - 1);
                    nav.set_spread_mode(SpreadMode::Double);
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("set_spread_mode left an invalid index: {label}"));
                    }

                    nav.set_cover_mode(match cover_mode {
                        CoverMode::Standalone => CoverMode::Paired,
                        CoverMode::Paired => CoverMode::Standalone,
                    });
                    nav.jump_to(total - 1);
                    nav.set_cover_mode(cover_mode);
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("set_cover_mode left an invalid index: {label}"));
                    }

                    nav.set_spread_mode(SpreadMode::Auto);
                    nav.set_viewport_size(1600.0, 900.0);
                    nav.jump_to(total - 1);
                    nav.set_viewport_size(900.0, 1200.0);
                    if nav.context().normalize(nav.index()) != nav.index() {
                        failures.push(format!("set_viewport_size left an invalid index: {label}"));
                    }
                }
            }
        }

        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn mode_change_then_jump_lands_on_the_containing_spread() {
        let cases = [
            (
                SpreadMode::Single,
                CoverMode::Standalone,
                SpreadMode::Double,
                CoverMode::Paired,
                5,
                2,
                2,
            ),
            (
                SpreadMode::Single,
                CoverMode::Paired,
                SpreadMode::Double,
                CoverMode::Standalone,
                6,
                3,
                3,
            ),
            (
                SpreadMode::Double,
                CoverMode::Standalone,
                SpreadMode::Single,
                CoverMode::Standalone,
                5,
                2,
                2,
            ),
            (
                SpreadMode::Double,
                CoverMode::Standalone,
                SpreadMode::Single,
                CoverMode::Paired,
                5,
                2,
                2,
            ),
            (
                SpreadMode::Double,
                CoverMode::Standalone,
                SpreadMode::Double,
                CoverMode::Paired,
                6,
                2,
                2,
            ),
            (
                SpreadMode::Double,
                CoverMode::Paired,
                SpreadMode::Single,
                CoverMode::Standalone,
                3,
                1,
                1,
            ),
            (
                SpreadMode::Double,
                CoverMode::Paired,
                SpreadMode::Single,
                CoverMode::Paired,
                3,
                1,
                1,
            ),
            (
                SpreadMode::Double,
                CoverMode::Paired,
                SpreadMode::Double,
                CoverMode::Standalone,
                7,
                3,
                3,
            ),
        ];
        let mut failures = Vec::new();

        for (before_spread, before_cover, book_spread, book_cover, total, resume, expected) in cases
        {
            let mut nav = SpreadNavigation::new(before_spread, before_cover);
            nav.set_total(total);
            nav.set_spread_mode(book_spread);
            nav.set_cover_mode(book_cover);
            nav.jump_to(resume);

            let spread = nav.spread();
            let contains = spread
                .is_some_and(|spread| spread.leading == resume || spread.trailing == Some(resume));
            if nav.index() != expected || !contains {
                failures.push(format!(
                    "resume {resume} of {total}: {before_spread:?}/{before_cover:?} -> \
                     {book_spread:?}/{book_cover:?}; index={}, spread={spread:?}, expected={expected}",
                    nav.index()
                ));
            }
        }

        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn set_viewport_size_returns_true_only_when_the_effective_layout_flips() {
        let cases = [
            ("landscape to portrait", 1600.0, 900.0, 900.0, 1200.0, true),
            ("portrait to landscape", 900.0, 1200.0, 1600.0, 900.0, true),
            (
                "landscape to landscape",
                1600.0,
                900.0,
                1200.0,
                900.0,
                false,
            ),
            ("portrait to portrait", 900.0, 1200.0, 600.0, 1000.0, false),
            ("square to zero", 1.0, 1.0, 0.0, 0.0, false),
            ("portrait to zero", 900.0, 1200.0, 0.0, 0.0, true),
            ("square to NaN", 1.0, 1.0, f32::NAN, f32::NAN, false),
            (
                "portrait to infinity",
                900.0,
                1200.0,
                f32::INFINITY,
                1.0,
                true,
            ),
            ("square to negative", 1.0, 1.0, -1.0, 1.0, false),
        ];
        let mut failures = Vec::new();

        for (label, before_width, before_height, width, height, expected) in cases {
            let mut nav = SpreadNavigation::new(SpreadMode::Auto, CoverMode::Standalone);
            nav.set_viewport_size(before_width, before_height);
            let actual = nav.set_viewport_size(width, height);
            if actual != expected {
                failures.push(format!("{label}: expected {expected}, got {actual}"));
            }
        }

        for spread_mode in [SpreadMode::Single, SpreadMode::Double] {
            let mut nav = SpreadNavigation::new(spread_mode, CoverMode::Standalone);
            nav.set_viewport_size(900.0, 1200.0);
            if nav.set_viewport_size(1600.0, 900.0) {
                failures.push(format!(
                    "fixed {spread_mode:?} reported an effective-layout flip"
                ));
            }
        }

        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn set_total_resets_the_index() {
        let mut nav = SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone);
        nav.set_total(5);
        assert!(nav.jump_to(3));

        nav.set_total(7);

        assert_eq!(nav.total(), 7);
        assert_eq!(nav.index(), 0);
    }

    #[test]
    fn next_prev_are_no_ops_at_zero_total() {
        let mut nav = SpreadNavigation::new(SpreadMode::Double, CoverMode::Paired);

        assert!(!nav.next());
        assert!(!nav.prev());
        assert_eq!(nav.index(), 0);
        assert_eq!(nav.spread(), None);
    }

    fn navigation_with(
        total: usize,
        spread_mode: SpreadMode,
        cover_mode: CoverMode,
    ) -> SpreadNavigation {
        let mut nav = SpreadNavigation::new(spread_mode, cover_mode);
        nav.set_total(total);
        nav
    }

    #[test]
    fn navigation_resume_index_to_persist_cases() {
        let empty = SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone);
        assert_eq!(empty.resume_index_to_persist(), 0);

        let mut single_mid = navigation_with(10, SpreadMode::Single, CoverMode::Standalone);
        assert!(single_mid.jump_to(5));
        assert_eq!(single_mid.resume_index_to_persist(), 5);

        let mut single_final = navigation_with(10, SpreadMode::Single, CoverMode::Standalone);
        assert!(single_final.jump_to(9));
        assert_eq!(single_final.resume_index_to_persist(), 9);

        let mut paired_final = navigation_with(10, SpreadMode::Double, CoverMode::Paired);
        assert!(paired_final.jump_to(8));
        assert_eq!(paired_final.index(), 8);
        assert_eq!(paired_final.resume_index_to_persist(), 9);

        let mut paired_mid = navigation_with(10, SpreadMode::Double, CoverMode::Paired);
        assert!(paired_mid.jump_to(2));
        assert_eq!(paired_mid.index(), 2);
        assert_eq!(paired_mid.resume_index_to_persist(), 2);
    }

    #[test]
    fn navigation_single_next_prev_and_clamping() {
        let mut nav = navigation_with(3, SpreadMode::Single, CoverMode::Standalone);
        assert!(nav.next());
        assert_eq!(nav.index(), 1);
        assert!(nav.next());
        assert_eq!(nav.index(), 2);
        assert!(!nav.next());
        assert_eq!(nav.index(), 2);
        assert!(nav.prev());
        assert_eq!(nav.index(), 1);
        assert!(nav.prev());
        assert_eq!(nav.index(), 0);
        assert!(!nav.prev());

        let mut one_page = navigation_with(1, SpreadMode::Single, CoverMode::Standalone);
        assert!(!one_page.next());
        assert!(!one_page.prev());
        assert_eq!(one_page.index(), 0);
    }

    #[test]
    fn navigation_double_standalone_advances_by_spread() {
        let mut nav = navigation_with(6, SpreadMode::Double, CoverMode::Standalone);
        assert_eq!(nav.index(), 0);
        assert!(nav.next());
        assert_eq!(nav.index(), 1);
        assert!(nav.next());
        assert_eq!(nav.index(), 3);
        assert!(nav.next());
        assert_eq!(nav.index(), 5);
        assert!(!nav.next());
        assert_eq!(nav.index(), 5);
        assert!(nav.prev());
        assert_eq!(nav.index(), 3);
        assert!(nav.prev());
        assert_eq!(nav.index(), 1);
        assert!(nav.prev());
        assert_eq!(nav.index(), 0);
        assert!(!nav.prev());
    }

    #[test]
    fn navigation_double_paired_advances_by_spread() {
        let mut nav = navigation_with(5, SpreadMode::Double, CoverMode::Paired);
        assert_eq!(nav.index(), 0);
        assert!(nav.next());
        assert_eq!(nav.index(), 2);
        assert!(nav.next());
        assert_eq!(nav.index(), 4);
        assert!(!nav.next());
        assert_eq!(nav.index(), 4);
        assert!(nav.prev());
        assert_eq!(nav.index(), 2);
        assert!(nav.prev());
        assert_eq!(nav.index(), 0);
        assert!(!nav.prev());
    }

    #[test]
    fn navigation_jump_to_cases() {
        let mut empty = SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone);
        assert!(!empty.jump_to(0));
        assert_eq!(empty.index(), 0);

        let mut single = navigation_with(6, SpreadMode::Single, CoverMode::Standalone);
        assert!(!single.jump_to(0));
        assert!(single.jump_to(4));
        assert_eq!(single.index(), 4);
        assert!(single.jump_to(99));
        assert_eq!(single.index(), 5);

        let mut standalone = navigation_with(6, SpreadMode::Double, CoverMode::Standalone);
        assert!(standalone.jump_to(2));
        assert_eq!(standalone.index(), 1);
        assert!(standalone.jump_to(4));
        assert_eq!(standalone.index(), 3);

        let mut paired = navigation_with(6, SpreadMode::Double, CoverMode::Paired);
        assert!(!paired.jump_to(1));
        assert_eq!(paired.index(), 0);
        assert!(paired.jump_to(3));
        assert_eq!(paired.index(), 2);
        assert!(paired.jump_to(5));
        assert_eq!(paired.index(), 4);
    }

    #[test]
    fn navigation_preview_spread_cases() {
        let paired = navigation_with(10, SpreadMode::Double, CoverMode::Paired);
        assert_eq!(paired.preview_spread(3), Some(sp(2, Some(3))));

        let paired_tail = navigation_with(2, SpreadMode::Double, CoverMode::Paired);
        assert_eq!(paired_tail.preview_spread(1), Some(sp(0, Some(1))));

        let standalone = navigation_with(10, SpreadMode::Double, CoverMode::Standalone);
        assert_eq!(standalone.preview_spread(0), Some(sp(0, None)));

        let single = navigation_with(10, SpreadMode::Single, CoverMode::Standalone);
        assert_eq!(single.preview_spread(3), Some(sp(3, None)));

        let empty = SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone);
        assert_eq!(empty.preview_spread(3), None);
    }

    #[test]
    fn navigation_preview_is_double_cases() {
        let standalone = navigation_with(6, SpreadMode::Double, CoverMode::Standalone);
        assert!(!standalone.preview_is_double(0));
        assert!(standalone.preview_is_double(1));
        assert!(standalone.preview_is_double(2));
        assert!(standalone.preview_is_double(3));
        assert!(!standalone.preview_is_double(5));
        assert_eq!(standalone.index(), 0, "preview must not move the index");

        let empty = SpreadNavigation::new(SpreadMode::Single, CoverMode::Standalone);
        assert!(!empty.preview_is_double(0));

        let single = navigation_with(6, SpreadMode::Single, CoverMode::Standalone);
        assert!(!single.preview_is_double(0));
        assert!(!single.preview_is_double(3));

        let paired = navigation_with(6, SpreadMode::Double, CoverMode::Paired);
        assert!(paired.preview_is_double(0));
        assert!(paired.preview_is_double(1));
        assert!(paired.preview_is_double(4));
        assert_eq!(paired.index(), 0, "preview must not move the index");

        let paired_odd = navigation_with(5, SpreadMode::Double, CoverMode::Paired);
        assert!(paired_odd.preview_is_double(0));
        assert!(!paired_odd.preview_is_double(4));
    }

    #[test]
    fn navigation_mode_setters_are_idempotent_and_renormalize() {
        let mut spread = navigation_with(6, SpreadMode::Single, CoverMode::Standalone);
        assert!(spread.jump_to(2));
        assert!(!spread.set_spread_mode(SpreadMode::Single));
        assert_eq!(spread.index(), 2);
        assert!(spread.set_spread_mode(SpreadMode::Double));
        assert_eq!(spread.index(), 1);

        let mut cover = navigation_with(6, SpreadMode::Double, CoverMode::Standalone);
        assert!(cover.jump_to(5));
        assert!(!cover.set_cover_mode(CoverMode::Standalone));
        assert_eq!(cover.index(), 5);
        assert!(cover.set_cover_mode(CoverMode::Paired));
        assert_eq!(cover.index(), 4);
        assert!(cover.set_cover_mode(CoverMode::Standalone));
        assert_eq!(cover.index(), 3);
    }

    #[test]
    fn navigation_auto_resolves_from_viewport() {
        let mut portrait = navigation_with(6, SpreadMode::Auto, CoverMode::Standalone);
        assert!(portrait.set_viewport_size(900.0, 1200.0));
        assert_eq!(portrait.spread(), Some(sp(0, None)));
        assert!(portrait.next());
        assert_eq!(portrait.index(), 1);
        assert_eq!(portrait.spread(), Some(sp(1, Some(2))));
        assert!(portrait.next());
        assert_eq!(portrait.index(), 3);
        assert!(portrait.next());
        assert_eq!(portrait.index(), 5);
        assert_eq!(portrait.spread(), Some(sp(5, None)));

        let mut landscape = navigation_with(5, SpreadMode::Auto, CoverMode::Standalone);
        assert!(!landscape.set_viewport_size(1600.0, 900.0));
        assert!(landscape.next());
        assert_eq!(landscape.index(), 1);
        assert_eq!(landscape.spread(), Some(sp(1, None)));
        assert!(landscape.next());
        assert_eq!(landscape.index(), 2);
        assert_eq!(landscape.spread(), Some(sp(2, None)));
    }

    #[test]
    fn navigation_auto_portrait_with_paired_cover_advances_by_spread() {
        let mut nav = navigation_with(5, SpreadMode::Auto, CoverMode::Paired);
        assert!(nav.set_viewport_size(900.0, 1200.0));
        assert_eq!(nav.spread(), Some(sp(0, Some(1))));
        assert!(nav.next());
        assert_eq!(nav.index(), 2);
        assert_eq!(nav.spread(), Some(sp(2, Some(3))));
        assert!(nav.next());
        assert_eq!(nav.index(), 4);
        assert_eq!(nav.spread(), Some(sp(4, None)));
        assert!(!nav.next());
        assert_eq!(nav.index(), 4);
    }

    #[test]
    fn navigation_viewport_flip_renormalizes_and_fixed_modes_ignore_aspect() {
        let mut auto_paired = navigation_with(6, SpreadMode::Auto, CoverMode::Paired);
        assert!(!auto_paired.set_viewport_size(1600.0, 900.0));
        assert!(auto_paired.next());
        assert_eq!(auto_paired.index(), 1);
        assert!(auto_paired.set_viewport_size(900.0, 1200.0));
        assert_eq!(auto_paired.index(), 0);

        let mut fixed = navigation_with(6, SpreadMode::Double, CoverMode::Standalone);
        assert!(fixed.next());
        let before = fixed.index();
        assert!(!fixed.set_viewport_size(900.0, 1200.0));
        assert_eq!(fixed.index(), before);
        assert!(!fixed.set_viewport_size(1600.0, 900.0));
        assert_eq!(fixed.index(), before);
    }

    #[test]
    fn navigation_set_auto_uses_current_viewport() {
        let mut portrait = navigation_with(6, SpreadMode::Single, CoverMode::Standalone);
        assert!(portrait.jump_to(2));
        portrait.set_viewport_size(900.0, 1200.0);
        assert!(portrait.set_spread_mode(SpreadMode::Auto));
        assert_eq!(portrait.spread_mode(), SpreadMode::Auto);
        assert_eq!(portrait.index(), 1);

        let mut landscape = navigation_with(6, SpreadMode::Single, CoverMode::Standalone);
        assert!(landscape.jump_to(2));
        landscape.set_viewport_size(1600.0, 900.0);
        assert!(landscape.set_spread_mode(SpreadMode::Auto));
        assert_eq!(landscape.spread_mode(), SpreadMode::Auto);
        assert_eq!(landscape.index(), 2);
    }

    #[test]
    fn scrub_fraction_zero_count_is_zero_guard() {
        assert_eq!(scrub_fraction_to_page(0.0, 0, false), 0);
        assert_eq!(scrub_fraction_to_page(0.5, 0, false), 0);
        assert_eq!(scrub_fraction_to_page(1.0, 0, true), 0);
    }

    #[test]
    fn scrub_fraction_ltr_maps_ends_and_midpoint() {
        assert_eq!(scrub_fraction_to_page(0.0, 10, false), 0);
        assert_eq!(scrub_fraction_to_page(1.0, 10, false), 9);
        assert_eq!(scrub_fraction_to_page(0.5, 10, false), 5);
    }

    #[test]
    fn scrub_fraction_rtl_inverts_fraction() {
        assert_eq!(scrub_fraction_to_page(0.0, 10, true), 9);
        assert_eq!(scrub_fraction_to_page(1.0, 10, true), 0);
        assert_eq!(scrub_fraction_to_page(0.5, 10, true), 5);
    }

    #[test]
    fn scrub_fraction_clamps_out_of_range_input() {
        assert_eq!(scrub_fraction_to_page(-0.4, 5, false), 0);
        assert_eq!(scrub_fraction_to_page(1.7, 5, false), 4);
        assert_eq!(scrub_fraction_to_page(-0.4, 5, true), 4);
        assert_eq!(scrub_fraction_to_page(1.7, 5, true), 0);
    }

    #[test]
    fn scrub_fraction_rounds_half_up_at_exact_half_step() {
        assert_eq!(scrub_fraction_to_page(0.125, 5, false), 1);
        assert_eq!(scrub_fraction_to_page(0.375, 5, false), 2);
        assert_eq!(scrub_fraction_to_page(0.124, 5, false), 0);
        assert_eq!(scrub_fraction_to_page(0.875, 5, true), 1);
        assert_eq!(scrub_fraction_to_page(0.625, 5, true), 2);
    }

    #[test]
    fn scrub_fraction_non_finite_single_page_is_zero() {
        assert_eq!(scrub_fraction_to_page(f32::NAN, 1, true), 0);
        assert_eq!(scrub_fraction_to_page(f32::INFINITY, 1, false), 0);
    }

    #[test]
    fn scrub_fraction_odd_span_rtl_half_step_rounds_half_up() {
        assert_eq!(scrub_fraction_to_page(0.5, 4, true), 2);
    }

    #[test]
    fn scrub_fraction_single_page_is_always_zero() {
        assert_eq!(scrub_fraction_to_page(0.0, 1, false), 0);
        assert_eq!(scrub_fraction_to_page(1.0, 1, false), 0);
        assert_eq!(scrub_fraction_to_page(0.3, 1, true), 0);
    }

    #[test]
    fn scrub_fraction_is_total_function_no_nan_panic() {
        let page = scrub_fraction_to_page(f32::NAN, 8, false);
        assert_eq!(page, 0);
        let rtl_page = scrub_fraction_to_page(f32::INFINITY, 8, true);
        assert_eq!(rtl_page, 7);
    }

    #[test]
    fn scrub_commit_path_jumps_via_jump_to() {
        let mut nav = navigation_with(8, SpreadMode::Single, CoverMode::Standalone);
        let page = scrub_fraction_to_page(1.0, nav.total(), false);
        assert_eq!(page, 7);
        assert!(nav.jump_to(page));
        assert_eq!(nav.index(), 7);

        let mut rtl_nav = navigation_with(8, SpreadMode::Single, CoverMode::Standalone);
        let page = scrub_fraction_to_page(0.0, rtl_nav.total(), true);
        assert_eq!(page, 7);
        assert!(rtl_nav.jump_to(page));
        assert_eq!(rtl_nav.index(), 7);
    }
}
