use super::*;
use crate::view_modes::ReadingDirection::{Ltr, Rtl};
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

    for (before_spread, before_cover, book_spread, book_cover, total, resume, expected) in cases {
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
    assert_eq!(scrub_fraction_to_page(0.0, 0, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(0.5, 0, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 0, Rtl), 0);
}

#[test]
fn scrub_fraction_ltr_maps_ends_and_midpoint() {
    assert_eq!(scrub_fraction_to_page(0.0, 10, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 10, Ltr), 9);
    assert_eq!(scrub_fraction_to_page(0.5, 10, Ltr), 5);
}

#[test]
fn scrub_fraction_rtl_inverts_fraction() {
    assert_eq!(scrub_fraction_to_page(0.0, 10, Rtl), 9);
    assert_eq!(scrub_fraction_to_page(1.0, 10, Rtl), 0);
    assert_eq!(scrub_fraction_to_page(0.5, 10, Rtl), 5);
}

#[test]
fn scrub_rtl_is_the_mirror_of_ltr_at_the_ends() {
    for count in [1, 2, 10] {
        assert_eq!(
            scrub_fraction_to_page(0.0, count, Rtl),
            scrub_fraction_to_page(1.0, count, Ltr)
        );
        assert_eq!(
            scrub_fraction_to_page(1.0, count, Rtl),
            scrub_fraction_to_page(0.0, count, Ltr)
        );
    }
}

#[test]
fn scrub_fraction_clamps_out_of_range_input() {
    assert_eq!(scrub_fraction_to_page(-0.4, 5, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(1.7, 5, Ltr), 4);
    assert_eq!(scrub_fraction_to_page(-0.4, 5, Rtl), 4);
    assert_eq!(scrub_fraction_to_page(1.7, 5, Rtl), 0);
}

#[test]
fn scrub_fraction_rounds_half_up_at_exact_half_step() {
    assert_eq!(scrub_fraction_to_page(0.125, 5, Ltr), 1);
    assert_eq!(scrub_fraction_to_page(0.375, 5, Ltr), 2);
    assert_eq!(scrub_fraction_to_page(0.124, 5, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(0.875, 5, Rtl), 1);
    assert_eq!(scrub_fraction_to_page(0.625, 5, Rtl), 2);
}

#[test]
fn scrub_fraction_non_finite_single_page_is_zero() {
    assert_eq!(scrub_fraction_to_page(f32::NAN, 1, Rtl), 0);
    assert_eq!(scrub_fraction_to_page(f32::INFINITY, 1, Ltr), 0);
}

#[test]
fn scrub_fraction_odd_span_rtl_half_step_rounds_half_up() {
    assert_eq!(scrub_fraction_to_page(0.5, 4, Rtl), 2);
}

#[test]
fn scrub_fraction_single_page_is_always_zero() {
    assert_eq!(scrub_fraction_to_page(0.0, 1, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 1, Ltr), 0);
    assert_eq!(scrub_fraction_to_page(0.3, 1, Rtl), 0);
}

#[test]
fn scrub_fraction_is_total_function_no_nan_panic() {
    let page = scrub_fraction_to_page(f32::NAN, 8, Ltr);
    assert_eq!(page, 0);
    let rtl_page = scrub_fraction_to_page(f32::INFINITY, 8, Rtl);
    assert_eq!(rtl_page, 7);
}

#[test]
fn scrub_commit_path_jumps_via_jump_to() {
    let mut nav = navigation_with(8, SpreadMode::Single, CoverMode::Standalone);
    let page = scrub_fraction_to_page(1.0, nav.total(), Ltr);
    assert_eq!(page, 7);
    assert!(nav.jump_to(page));
    assert_eq!(nav.index(), 7);

    let mut rtl_nav = navigation_with(8, SpreadMode::Single, CoverMode::Standalone);
    let page = scrub_fraction_to_page(0.0, rtl_nav.total(), Rtl);
    assert_eq!(page, 7);
    assert!(rtl_nav.jump_to(page));
    assert_eq!(rtl_nav.index(), 7);
}
