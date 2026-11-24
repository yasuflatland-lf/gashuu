//! Pure page-pairing for two-page spread display. Slint/tracing-free and
//! reading-direction-agnostic: this module decides WHICH pages form a spread
//! (in reading order), never how they are placed left/right. Placement (RTL/LTR)
//! and input (which arrow advances) live in the presentation layer.
//!
//! Pairing receives an already-resolved `SpreadLayout` (never `Auto`).
//! `SpreadNavigation` owns `SpreadMode::Auto` resolution from the viewport
//! aspect ratio and supplies that resolved layout to the pairing functions.
//!
//! `scrub_fraction_to_page` uses `ReadingDirection` to map a knob position to a
//! raw page index. Pairing and `SpreadNavigation` are independent of direction.

use crate::view_modes::{CoverMode, ReadingDirection, SpreadLayout, SpreadMode};
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
pub(crate) fn spread_at(
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
    leading: usize,
) -> Spread {
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
pub(crate) fn next_leading(
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
    leading: usize,
) -> usize {
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
pub(crate) fn prev_leading(
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
    leading: usize,
) -> usize {
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
pub(crate) fn normalize_leading(
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
/// `SpreadNavigation` resolves the layout before constructing this context.
/// The free functions implement the pairing rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpreadContext {
    total: usize,
    layout: SpreadLayout,
    cover: CoverMode,
}

impl SpreadContext {
    pub(crate) fn new(total: usize, layout: SpreadLayout, cover: CoverMode) -> Self {
        Self {
            total,
            layout,
            cover,
        }
    }

    pub(crate) fn spread_at(&self, leading: usize) -> Spread {
        spread_at(self.total, self.layout, self.cover, leading)
    }

    pub(crate) fn normalize(&self, index: usize) -> usize {
        normalize_leading(self.total, self.layout, self.cover, index)
    }

    pub(crate) fn next(&self, index: usize) -> usize {
        next_leading(self.total, self.layout, self.cover, index)
    }

    pub(crate) fn prev(&self, index: usize) -> usize {
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
    pub(crate) fn context(&self) -> SpreadContext {
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
/// With `ReadingDirection::Rtl`, the screen-left edge maps to the last page
/// and the screen-right edge to the first. With `ReadingDirection::Ltr`, the
/// screen-left edge maps to the first page. The caller passes the raw index
/// to `SpreadNavigation` to select the spread containing that page.
// Single source of rounding (#71 Part D): the Slint scrubber passes the RAW clamped
// fraction via preview/commit; `handlers/viewer.rs` resolves the page through this helper.
pub fn scrub_fraction_to_page(
    fraction: f32,
    page_count: usize,
    direction: ReadingDirection,
) -> usize {
    if page_count == 0 {
        return 0;
    }
    // Clamp; a non-finite fraction (NaN/inf) collapses to 0 first.
    let f = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let f = match direction {
        ReadingDirection::Ltr => f,
        ReadingDirection::Rtl => 1.0 - f,
    };
    let last = page_count - 1;
    // Round half-up: +0.5 then floor (via `as usize` truncation on a non-negative
    // value). `last` fits f32 exactly for any realistic page count.
    let scaled = (f * last as f32 + 0.5) as usize;
    scaled.min(last)
}

#[cfg(test)]
#[path = "spread/tests.rs"]
mod tests;
